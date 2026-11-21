//! `quickloop` — deterministic PR review, patch, and verification worker.
//!
//! The worker owns every side effect (filesystem, git, gh, verification).
//! Model output never decides merge readiness; only `quickloop verify`
//! returning score = 100 does. A failing score is structured loop data, so
//! `verify` must never treat it as an execution failure.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod diff;
mod gh;
mod git;
mod patch;
mod publish;
mod telemetry;
mod verify;

/// Deterministic worker for the QuickLoop closed-loop review agent.
#[derive(Parser)]
#[command(name = "quickloop", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Emit the filtered PR diff as compact single-line JSON.
    Diff {
        /// PR number, `#N`, or full PR URL.
        pr: String,
    },
    /// Apply SEARCH/REPLACE blocks atomically (exit 2 if any block is rejected).
    Patch {
        /// Path to the file containing fenced SEARCH/REPLACE blocks.
        file: PathBuf,
    },
    /// Run test/lint/security/scope checks concurrently and emit the score.
    Verify,
    /// Commit verified changes and upsert the single PR status comment.
    Publish {
        /// PR number, `#N`, or full PR URL.
        pr: String,
        /// Re-check the gate only — zero remote writes, no commit, no comment.
        #[arg(long)]
        dry_run: bool,
        /// Loop iteration number recorded in the commit trailer and telemetry.
        #[arg(long, default_value_t = 0)]
        iteration: u32,
    },
}

fn main() {
    let cli = Cli::parse();
    let code = match cli.command {
        Command::Diff { pr } => diff::run(&pr).map(|_| 0),
        Command::Patch { file } => patch::run(&file).map(|outcome| outcome.exit_code()),
        Command::Verify => verify::run().map(|_| 0),
        Command::Publish {
            pr,
            dry_run,
            iteration,
        } => publish::run(&pr, dry_run, iteration).map(|_| 0),
    };
    match code {
        Ok(0) => {}
        Ok(code) => std::process::exit(code),
        Err(err) => {
            // Worker execution failures exit non-zero on stderr; compact JSON
            // on stdout is reserved for structured loop data.
            eprintln!("quickloop: {err}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::{CommandFactory, Parser};

    #[test]
    fn cli_parses_all_four_subcommands() {
        Cli::try_parse_from(["quickloop", "diff", "42"]).expect("diff should parse");
        Cli::try_parse_from(["quickloop", "patch", "out.patch"]).expect("patch should parse");
        Cli::try_parse_from(["quickloop", "verify"]).expect("verify should parse");
        Cli::try_parse_from(["quickloop", "publish", "#42"]).expect("publish should parse");
        Cli::try_parse_from(["quickloop", "publish", "42", "--dry-run"])
            .expect("publish --dry-run");
        Cli::try_parse_from(["quickloop", "publish", "42", "--iteration", "3"])
            .expect("publish --iteration");
    }

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }
}

/// Test-only helper shared by every module that runs git fixtures: tests
/// execute in parallel threads of ONE process, and `set_current_dir` is
/// process-global — a single shared lock is what makes the fixtures safe.
#[cfg(test)]
pub(crate) mod testutil {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    static CWD_LOCK: Mutex<()> = Mutex::new(());

    pub(crate) fn with_cwd<F: FnOnce()>(root: &Path, f: F) {
        let lock = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(root).unwrap();
        let _restore = CwdRestore(original);
        f();
        drop(_restore);
        drop(lock);
    }

    struct CwdRestore(PathBuf);

    impl Drop for CwdRestore {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.0);
        }
    }
}
