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

use ignore::WalkState;
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
        source: globset::Error,
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

        // Glob sets are built once; patterns are matched against the path
        // relative to its walk root (see `matches_relative`), so they work
        // with absolute roots as well.
        let exclude_set = build_glob_set(&opts.config.exclude)
            .map_err(|(pattern, source)| ProcessError::ExcludeGlob { pattern, source })?;
        let include_set = build_glob_set(&opts.config.include)
            .map_err(|(pattern, source)| ProcessError::IncludeGlob { pattern, source })?;

        let roots: Vec<PathBuf> = dirs.iter().map(|p| (*p).clone()).collect();
        let has_explicit = !files.is_empty();
        let ext_filter = |path: &Path| {
            has_target_extension(path, &opts.config.extensions)
                || matches_relative(&include_set, path, &roots)
        };

        builder.build_parallel().run(|| {
            let run_one = &run_one;
            let explicit = &explicit;
            let ext_filter = &ext_filter;
            let errors = &errors;
            let exclude_set = &exclude_set;
            let roots = &roots;
            Box::new(move |entry| {
                let entry = match entry {
                    Ok(entry) => entry,
                    // A walk error (e.g. a non-existent root path) must not
                    // be silently swallowed: CI typos would pass unnoticed.
                    Err(e) => {
                        errors.fetch_add(1, Ordering::Relaxed);
                        eprintln!("error: {e}");
                        return WalkState::Continue;
                    },
                };
                let path = entry.path();
                let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
                // Excluded: files are skipped, directories are pruned.
                if matches_relative(exclude_set, path, roots) {
                    return if is_dir {
                        WalkState::Skip
                    } else {
                        WalkState::Continue
                    };
                }
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    return WalkState::Continue;
                }
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

/// Build a glob set from config patterns; on failure returns the offending
/// pattern together with the error.
fn build_glob_set(patterns: &[String]) -> Result<globset::GlobSet, (String, globset::Error)> {
    let mut builder = globset::GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(globset::Glob::new(pattern).map_err(|e| (pattern.clone(), e))?);
    }
    builder.build().map_err(|e| (String::new(), e))
}

/// Whether `path` (as walked, possibly absolute) matches a glob set whose
/// patterns are expressed relative to one of the walk `roots`. Falls back
/// to matching the raw path, so relative roots keep working.
fn matches_relative(set: &globset::GlobSet, path: &Path, roots: &[PathBuf]) -> bool {
    roots
        .iter()
        .any(|r| path.strip_prefix(r).is_ok_and(|rel| set.is_match(rel)))
        || set.is_match(path)
}

/// Format `contents` and act according to the mode. Returns whether the
/// content needs changes. `contents` is the pre-format input, required
/// only by `Mode::Diff`; the mmap path passes `None` after dropping the
/// mapping (Windows refuses to replace a file with a live mapping).
fn finish(
    path: &Path,
    opts: &RunOptions,
    contents: Option<&[u8]>,
    formatted: Option<Vec<u8>>,
    meta: &fs::Metadata,
) -> Result<bool, ProcessError> {
    let Some(formatted) = formatted else {
        return Ok(false);
    };
    match opts.mode {
        Mode::Check => Ok(true),
        Mode::Diff => {
            let old = String::from_utf8_lossy(contents.unwrap_or_default());
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
        finish(path, opts, Some(&buf), formatted, &meta)
    } else {
        // SAFETY: the file is opened read-only and not modified by us while
        // mapped; see memmap2 docs for the external-modification caveats.
        let mmap = unsafe { MmapOptions::new().map(&file).context(ReadSnafu { path })? };
        #[cfg(unix)]
        let _ = mmap.advise(memmap2::Advice::Sequential);
        let formatted = crate::format(&mmap);
        if formatted.is_some() && opts.mode == Mode::Write {
            // On Windows, renaming over a file with a live mapping fails
            // (Access denied), so release the mapping and handle before
            // the write path runs. Diff mode does not write and keeps the
            // mapping as the old contents.
            drop(mmap);
            drop(file);
            return finish(path, opts, None, formatted, &meta);
        }
        finish(path, opts, Some(&mmap), formatted, &meta)
    }
}
