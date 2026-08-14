#![cfg(feature = "bin")]

use std::{
    io::{IsTerminal, Read, Write},
    path::PathBuf,
    process::ExitCode,
    sync::Mutex,
};

use clap::Parser;
use zhfmt::{
    config::{self, Config},
    process::{self, Mode, RunOptions},
};

#[derive(Parser)]
#[command(version, about, long_about = None, after_help = r#"Examples:
  zhfmt                     # format all doc files under the current directory
  zhfmt docs/ README.md     # format specific paths
  zhfmt --check             # CI mode: report files that would change, exit 1
  zhfmt --diff              # print a unified diff without writing
  cat a.md | zhfmt          # stdin -> stdout
"#)]
struct Cli {
    /// Files or directories to process (defaults to the current directory)
    paths: Vec<PathBuf>,

    /// Only report files that would change; exit code 1 if any
    #[arg(long)]
    check: bool,

    /// Print a unified diff instead of writing
    #[arg(long, conflicts_with = "check")]
    diff: bool,

    /// Path to a config file (zhfmt.json)
    #[arg(long)]
    config: Option<PathBuf>,

    /// Override the file extensions to process (comma separated)
    #[arg(long, value_delimiter = ',')]
    ext: Option<Vec<String>>,

    /// Number of walker threads (0 = auto)
    #[arg(short, long)]
    jobs: Option<usize>,
}

/// Load the config from `--config`, or discover it; `None` if not found.
fn resolve_config(cli: &Cli) -> Result<Config, String> {
    if let Some(path) = &cli.config {
        return config::load_file(path).map_err(|e| e.to_string());
    }
    // Discover from the real working directory: `Path::new(".").ancestors()`
    // only yields `[".", ""]`, which would never leave the cwd.
    let discovered = std::env::current_dir()
        .ok()
        .and_then(|cwd| config::discover(&cwd));
    match discovered.map(|p| config::load_file(&p)) {
        Some(Ok(c)) => Ok(c),
        Some(Err(e)) => Err(e.to_string()),
        None => Ok(Config::default()),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // stdin -> stdout pipeline mode.
    if cli.paths.is_empty() && !std::io::stdin().is_terminal() {
        if cli.check || cli.diff {
            eprintln!("error: --check/--diff cannot be combined with stdin input");
            return ExitCode::from(2);
        }
        return run_stdin();
    }

    let mut config = match resolve_config(&cli) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        },
    };
    if let Some(ext) = cli.ext {
        config.extensions = ext;
    }
    if let Some(jobs) = cli.jobs {
        config.jobs = jobs;
    }

    let mode = if cli.check {
        Mode::Check
    } else if cli.diff {
        Mode::Diff
    } else {
        Mode::Write
    };
    let opts = RunOptions {
        mode,
        config,
        print_lock: Mutex::new(()),
    };

    let paths = if cli.paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        cli.paths
    };

    match process::process_paths(&paths, &opts) {
        Ok(summary) => {
            eprintln!(
                "{} {} files, {} changed, {} errors.",
                match mode {
                    Mode::Check => "Checked",
                    Mode::Diff => "Diffed",
                    Mode::Write => "Processed",
                },
                summary.processed,
                summary.changed,
                summary.errors,
            );
            if summary.errors > 0 {
                ExitCode::from(2)
            } else if mode != Mode::Write && summary.changed > 0 {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            }
        },
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        },
    }
}

fn run_stdin() -> ExitCode {
    let mut buf = Vec::new();
    if let Err(e) = std::io::stdin().read_to_end(&mut buf) {
        eprintln!("error: failed to read stdin: {e}");
        return ExitCode::from(2);
    }
    let out = match zhfmt::format(&buf) {
        Some(out) => out,
        None => buf,
    };
    let mut stdout = std::io::stdout().lock();
    if let Err(e) = stdout.write_all(&out).and_then(|()| stdout.flush()) {
        eprintln!("error: failed to write stdout: {e}");
        return ExitCode::from(2);
    }
    ExitCode::SUCCESS
}
