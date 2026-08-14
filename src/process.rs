//! Parallel file discovery and processing.
//!
//! - Files are discovered with the `ignore` crate's parallel walker (respects gitignore by default,
//!   but *does* scan dotfiles).
//! - Small files are read into memory; large files are processed through a read-only memory map.
//! - Unchanged files are never written. Changed files are written to a temp file in the same
//!   directory and atomically renamed over the original.

use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use ignore::{WalkState, overrides::OverrideBuilder};
use memmap2::MmapOptions;
use snafu::{ResultExt, Snafu};

use crate::config::Config;

const SMALL_FILE_THRESHOLD: u64 = 256 * 1024;

#[derive(Debug, Snafu)]
pub enum ProcessError {
    #[snafu(display("failed to read {}", path.display()))]
    Read {
        source: std::io::Error,
        path: PathBuf,
    },
    #[snafu(display("failed to write {}", path.display()))]
    Write {
        source: std::io::Error,
        path: PathBuf,
    },
    #[snafu(display("failed to create temp file in {}", dir.display()))]
    CreateTemp {
        source: std::io::Error,
        dir: PathBuf,
    },
    #[snafu(display("failed to persist temp file to {}", path.display()))]
    Persist {
        source: tempfile::PersistError,
        path: PathBuf,
    },
    #[snafu(display("invalid exclude glob {pattern:?}"))]
    ExcludeGlob {
        source: ignore::Error,
        pattern: String,
    },
    #[snafu(display("invalid include glob {pattern:?}"))]
    IncludeGlob {
        source: globset::Error,
        pattern: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Overwrite changed files (via temp file + atomic rename).
    Write,
    /// Report files that would change; caller exits with code 1 if any.
    Check,
    /// Print a unified diff of the changes.
    Diff,
}

pub struct RunOptions {
    pub mode: Mode,
    pub config: Config,
    /// Serializes diff output from worker threads.
    pub print_lock: Mutex<()>,
}

impl RunOptions {
    /// Check mode with default config (used by benches and tests).
    #[must_use]
    pub fn check() -> Self {
        Self {
            mode: Mode::Check,
            config: Config::default(),
            print_lock: Mutex::new(()),
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Summary {
    pub processed: usize,
    pub changed: usize,
    pub errors: usize,
}

/// Process all given paths (files or directories) according to `opts`.
pub fn process_paths(paths: &[PathBuf], opts: &RunOptions) -> Result<Summary, ProcessError> {
    let (files, dirs): (Vec<_>, Vec<_>) = paths.iter().partition(|p| p.is_file());

    let processed = AtomicUsize::new(0);
    let changed = AtomicUsize::new(0);
    let errors = AtomicUsize::new(0);
    // Canonical paths of explicitly passed files, to avoid double processing
    // when they are also discovered by directory walking.
    let explicit: Mutex<std::collections::HashSet<PathBuf>> =
        Mutex::new(files.iter().filter_map(|p| p.canonicalize().ok()).collect());

    let run_one = |path: &Path| match process_file(path, opts) {
        Ok(did_change) => {
            processed.fetch_add(1, Ordering::Relaxed);
            if did_change {
                changed.fetch_add(1, Ordering::Relaxed);
                if opts.mode == Mode::Check {
                    eprintln!("would reformat: {}", path.display());
                }
            }
        },
        Err(e) => {
            errors.fetch_add(1, Ordering::Relaxed);
            eprintln!("error: {e}");
        },
    };

    // Explicitly passed files are always processed, regardless of extension.
    for f in &files {
        run_one(f);
    }

    if let Some((first, rest)) = dirs.split_first() {
        let mut builder = ignore::WalkBuilder::new(first);
        for d in rest {
            builder.add(d);
        }
        builder.hidden(false).threads(opts.config.jobs);
        if !opts.config.exclude.is_empty() {
            let mut ob = OverrideBuilder::new(".");
            for pattern in &opts.config.exclude {
                ob.add(&format!("!{pattern}")).context(ExcludeGlobSnafu {
                    pattern: pattern.clone(),
                })?;
            }
            let o = ob.build().context(ExcludeGlobSnafu {
                pattern: String::new(),
            })?;
            builder.overrides(o);
        }

        let include_set = {
            let mut b = globset::GlobSetBuilder::new();
            for pattern in &opts.config.include {
                b.add(globset::Glob::new(pattern).context(IncludeGlobSnafu {
                    pattern: pattern.clone(),
                })?);
            }
            b.build().context(IncludeGlobSnafu {
                pattern: String::new(),
            })?
        };

        let has_explicit = !files.is_empty();
        let ext_filter = |path: &Path| {
            has_target_extension(path, &opts.config.extensions) || include_set.is_match(path)
        };

        builder.build_parallel().run(|| {
            let run_one = &run_one;
            let explicit = &explicit;
            let ext_filter = &ext_filter;
            Box::new(move |entry| {
                let Ok(entry) = entry else {
                    return WalkState::Continue
                };
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    return WalkState::Continue;
                }
                let path = entry.path();
                if !ext_filter(path) {
                    return WalkState::Continue;
                }
                if has_explicit && let Ok(canon) = path.canonicalize() {
                    let mut set = explicit.lock().unwrap();
                    if !set.insert(canon) {
                        // Already processed as an explicit file.
                        return WalkState::Continue;
                    }
                }
                run_one(path);
                WalkState::Continue
            })
        });
    }

    Ok(Summary {
        processed: processed.load(Ordering::Relaxed),
        changed: changed.load(Ordering::Relaxed),
        errors: errors.load(Ordering::Relaxed),
    })
}

fn has_target_extension(path: &Path, extensions: &[String]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| extensions.iter().any(|t| t.eq_ignore_ascii_case(e)))
}

/// Format `contents` and act according to the mode. Returns whether the
/// content needs changes.
fn finish(
    path: &Path,
    opts: &RunOptions,
    contents: &[u8],
    formatted: Option<Vec<u8>>,
    meta: &fs::Metadata,
) -> Result<bool, ProcessError> {
    let Some(formatted) = formatted else {
        return Ok(false);
    };
    match opts.mode {
        Mode::Check => Ok(true),
        Mode::Diff => {
            let old = String::from_utf8_lossy(contents);
            let new = String::from_utf8_lossy(&formatted);
            let diff = similar::TextDiff::from_lines(old.as_ref(), new.as_ref());
            let _guard = opts.print_lock.lock().unwrap();
            println!("--- {}\n+++ {}", path.display(), path.display());
            print!(
                "{}",
                diff.unified_diff()
                    .context_radius(3)
                    .header("original", "formatted")
            );
            Ok(true)
        },
        Mode::Write => {
            let parent = path.parent().unwrap_or_else(|| Path::new("."));
            let mut tmp =
                tempfile::NamedTempFile::new_in(parent).context(CreateTempSnafu { dir: parent })?;
            tmp.write_all(&formatted).context(WriteSnafu { path })?;
            // No fsync before persist: rename is atomic and the tool is
            // idempotent, so a crash just means re-running.
            let _ = tmp.as_file().set_permissions(meta.permissions());
            tmp.persist(path).context(PersistSnafu { path })?;
            Ok(true)
        },
    }
}

/// Process a single file. Returns whether the content needs changes.
pub fn process_file(path: &Path, opts: &RunOptions) -> Result<bool, ProcessError> {
    // Open once and stat the handle: avoids a second path resolution per file.
    let mut file = fs::File::open(path).context(ReadSnafu { path })?;
    let meta = file.metadata().context(ReadSnafu { path })?;
    if meta.len() == 0 {
        return Ok(false);
    }
    if meta.len() < SMALL_FILE_THRESHOLD {
        let mut buf = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(usize::MAX));
        file.read_to_end(&mut buf).context(ReadSnafu { path })?;
        // Close the handle before finish(): on Windows, writing back via
        // rename would fail while our own handle is still open.
        drop(file);
        let formatted = crate::format(&buf);
        finish(path, opts, &buf, formatted, &meta)
    } else {
        // SAFETY: the file is opened read-only and not modified by us while
        // mapped; see memmap2 docs for the external-modification caveats.
        let mmap = unsafe { MmapOptions::new().map(&file).context(ReadSnafu { path })? };
        #[cfg(unix)]
        let _ = mmap.advise(memmap2::Advice::Sequential);
        let formatted = crate::format(&mmap);
        finish(path, opts, &mmap, formatted, &meta)
    }
}
