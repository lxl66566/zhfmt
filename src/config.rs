//! Configuration file support (`zhfmt.json` / `.zhfmt.json`).
//!
//! Lookup order: the nearest config file walking up from the current
//! directory, then the global config (`$XDG_CONFIG_HOME/zhfmt/zhfmt.json` or
//! platform equivalent). Missing config is not an error; defaults apply.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use snafu::{ResultExt, Snafu};

use crate::whitelist::Whitelist;

pub const CONFIG_NAMES: [&str; 2] = ["zhfmt.json", ".zhfmt.json"];
pub const DEFAULT_EXTENSIONS: [&str; 5] = ["md", "markdown", "mdx", "txt", "rst"];

#[derive(Debug, Snafu)]
pub enum ConfigError {
    #[snafu(display("failed to read config file {}", path.display()))]
    Read {
        source: std::io::Error,
        path: PathBuf,
    },
    #[snafu(display("failed to parse config file {}", path.display()))]
    Parse {
        source: serde_json::Error,
        path: PathBuf,
    },
    #[snafu(display("invalid glob pattern {pattern:?} in {}", path.display()))]
    Glob {
        source: globset::Error,
        pattern: String,
        path: PathBuf,
    },
    #[snafu(display("whitelist entry {entry:?} must be exactly two characters in {}", path.display()))]
    WhitelistEntry { entry: String, path: PathBuf },
}

/// Raw config file schema.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ConfigFile {
    /// File extensions to process (without dot), replacing the defaults.
    pub extensions: Option<Vec<String>>,
    /// Additional glob patterns to include (even if the extension does not
    /// match), relative to the walk root.
    pub include: Vec<String>,
    /// Glob patterns to exclude, relative to the walk root.
    pub exclude: Vec<String>,
    /// Character pairs between which no space is inserted; each entry is
    /// exactly two characters (e.g. `"B站"` keeps `B站` unspaced).
    pub whitelist: Vec<String>,
    /// Number of walker threads; 0 or absent means auto.
    pub jobs: Option<usize>,
}

/// Resolved configuration used by the runner.
#[derive(Debug)]
pub struct Config {
    /// Extensions to process (without dot).
    pub extensions: Vec<String>,
    /// Extra whitelist globs, matched against paths relative to the walk root.
    pub include: Vec<String>,
    /// Blacklist globs, matched against paths relative to the walk root.
    pub exclude: Vec<String>,
    /// Compiled pair whitelist; empty means disabled.
    pub whitelist: Whitelist,
    /// Walker threads; 0 means auto.
    pub jobs: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            extensions: DEFAULT_EXTENSIONS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            include: Vec::new(),
            exclude: Vec::new(),
            whitelist: Whitelist::default(),
            jobs: 0,
        }
    }
}

/// Compile whitelist entries (each exactly two characters) into a
/// [`Whitelist`], validating length eagerly so config errors surface at
/// startup.
fn compile_whitelist(entries: &[String], path: &Path) -> Result<Whitelist, ConfigError> {
    let mut pairs = Vec::with_capacity(entries.len());
    for entry in entries {
        let mut chars = entry.chars();
        match (chars.next(), chars.next(), chars.next()) {
            (Some(left), Some(right), None) => pairs.push((left as u32, right as u32)),
            _ => {
                return Err(ConfigError::WhitelistEntry {
                    entry: entry.clone(),
                    path: path.to_path_buf(),
                });
            },
        }
    }
    Ok(Whitelist::new(pairs))
}

impl Config {
    pub fn from_file(file: ConfigFile, path: &Path) -> Result<Self, ConfigError> {
        // Validate glob patterns eagerly so config errors surface at startup.
        let validate = |patterns: &[String]| -> Result<(), ConfigError> {
            for pattern in patterns {
                globset::GlobBuilder::new(pattern)
                    .literal_separator(false)
                    .build()
                    .context(GlobSnafu {
                        pattern: pattern.clone(),
                        path,
                    })?;
            }
            Ok(())
        };
        validate(&file.include)?;
        validate(&file.exclude)?;
        Ok(Self {
            extensions: file
                .extensions
                .unwrap_or_else(|| Config::default().extensions),
            include: file.include,
            exclude: file.exclude,
            whitelist: compile_whitelist(&file.whitelist, path)?,
            jobs: file.jobs.unwrap_or(0),
        })
    }
}

/// Parse a config file from disk.
pub fn load_file(path: &Path) -> Result<Config, ConfigError> {
    let content = fs::read_to_string(path).context(ReadSnafu { path })?;
    let file: ConfigFile = serde_json::from_str(&content).context(ParseSnafu { path })?;
    Config::from_file(file, path)
}

/// Find the nearest local config walking up from `start`, or the global one.
#[must_use]
pub fn discover(start: &Path) -> Option<PathBuf> {
    for dir in start.ancestors() {
        for name in CONFIG_NAMES {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let config_dir = dirs::config_dir()?;
    let global = config_dir.join("zhfmt").join(CONFIG_NAMES[0]);
    global.is_file().then_some(global)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_walks_up_to_parent_dirs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join(".zhfmt.json");
        fs::write(&cfg, "{}").unwrap();
        // A nested dir with no config of its own finds the parent's config.
        let deep = tmp.path().join("a").join("b");
        fs::create_dir_all(&deep).unwrap();
        assert_eq!(discover(&deep), Some(cfg.clone()));
        assert_eq!(discover(tmp.path()), Some(cfg));
    }

    #[test]
    fn discover_missing_returns_none() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert_eq!(discover(tmp.path()), None);
    }

    #[test]
    fn whitelist_compiles_pairs() {
        let cfg = ConfigFile {
            whitelist: vec!["B站".to_string(), "中a".to_string()],
            ..Default::default()
        };
        let c = Config::from_file(cfg, Path::new("zhfmt.json")).unwrap();
        assert!(c.whitelist.contains('B' as u32, '站' as u32));
        assert!(c.whitelist.contains('中' as u32, 'a' as u32));
        assert!(!c.whitelist.contains('中' as u32, 'b' as u32));
    }

    #[test]
    fn whitelist_rejects_bad_lengths() {
        for bad in ["", "B", "B站x"] {
            let cfg = ConfigFile {
                whitelist: vec![bad.to_string()],
                ..Default::default()
            };
            let err = Config::from_file(cfg, Path::new("zhfmt.json")).unwrap_err();
            assert!(err.to_string().contains("exactly two characters"), "{err}");
        }
    }
}
