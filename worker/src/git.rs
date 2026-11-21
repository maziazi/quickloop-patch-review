//! Thin wrapper around the `git` CLI. The worker owns every side effect;
//! arguments are fixed vectors — model-authored text never composes
//! commands, it only ever lands in file content.

use std::fs;
use std::path::Path;
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

/// Register `pattern` in `.git/info/exclude` — worker-owned, never
/// committed, and effective in every target repository regardless of its
/// own `.gitignore`. Keeps runtime artifacts (`.quickloop/`) out of user
/// commits even when the worker stages with `git add -A`.
pub fn ensure_local_exclude(pattern: &str) -> Result<(), String> {
    let exclude_path = Path::new(".git").join("info").join("exclude");
    fs::create_dir_all(
        exclude_path
            .parent()
            .ok_or(".git/info path has no parent")?,
    )
    .map_err(|e| format!("create .git/info: {e}"))?;
    let existing = fs::read_to_string(&exclude_path).unwrap_or_default();
    if existing.lines().any(|line| line.trim() == pattern) {
        return Ok(());
    }
    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(pattern);
    content.push('\n');
    fs::write(&exclude_path, content).map_err(|e| format!("write {}: {e}", exclude_path.display()))
}

#[cfg(test)]
mod tests {
    use super::{ensure_local_exclude, run};
    use crate::testutil::with_cwd;
    use std::fs;
    use std::path::PathBuf;

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

    #[test]
    fn local_exclude_is_idempotent_and_does_not_duplicate() {
        let root =
            std::env::temp_dir().join(format!("quickloop-git-exclude-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".git/info")).unwrap();
        fs::write(root.join(".git/info/exclude"), "target/\n").unwrap();
        with_cwd(&root, || {
            ensure_local_exclude(".quickloop/").expect("first write must succeed");
            ensure_local_exclude(".quickloop/").expect("second write must be a no-op");
            let content = fs::read_to_string(".git/info/exclude").unwrap();
            assert_eq!(
                content
                    .lines()
                    .filter(|l| l.trim() == ".quickloop/")
                    .count(),
                1
            );
            assert!(
                content.contains("target/"),
                "existing entries are preserved"
            );
        });
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn local_exclude_creates_missing_info_dir() {
        let root = std::env::temp_dir().join(format!(
            "quickloop-git-exclude-missing-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".git")).unwrap();
        with_cwd(&root, || {
            ensure_local_exclude(".quickloop/").expect("must create .git/info/exclude");
            assert!(PathBuf::from(".git/info/exclude").exists());
        });
        let _ = fs::remove_dir_all(root);
    }
}
