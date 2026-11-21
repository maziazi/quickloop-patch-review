//! Thin wrapper around the `git` CLI. The worker owns every side effect;
//! arguments are fixed vectors — model-authored text never composes
//! commands, it only ever lands in file content.

use std::process::Command;

/// Run `git <args...>` in the current working directory and return stdout.
pub fn run(args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .map_err(|e| format!("spawn git: {e} (is git installed?)"))?;
    if output.status.success() {
        String::from_utf8(output.stdout).map_err(|_| "git produced non-UTF-8 output".to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("git {} failed: {}", args.join(" "), stderr.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::run;

    #[test]
    fn failing_git_command_reports_stderr_context() {
        let err = run(&["rev-parse", "--verify", "definitely-not-a-ref"])
            .expect_err("unknown ref must fail");
        assert!(err.contains("git rev-parse"));
        assert!(err.contains("failed"));
    }

    #[test]
    fn successful_git_command_returns_stdout() {
        let out = run(&["--version"]).expect("git --version must succeed");
        assert!(out.starts_with("git version"));
    }
}
