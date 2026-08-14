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
}

/// Raw config file schema.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ConfigFile {
    /// File extensions to process (without dot), replacing the defaults.
    pub extensions: Option<Vec<String>>,
    /// Additional glob patterns to include (even if the extension does not
    /// match), relative to the current working directory.
    pub include: Vec<String>,
    /// Glob patterns to exclude, relative to the current working directory.
    pub exclude: Vec<String>,
    /// Number of walker threads; 0 or absent means auto.
    pub jobs: Option<usize>,
}

/// Resolved configuration used by the runner.
#[derive(Debug)]
pub struct Config {
    /// Extensions to process (without dot).
    pub extensions: Vec<String>,
    /// Extra whitelist globs (matched against paths as walked).
    pub include: Vec<String>,
    /// Blacklist globs (matched against paths relative to the walk root).
    pub exclude: Vec<String>,
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
            jobs: 0,
        }
    }
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
