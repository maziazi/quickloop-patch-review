//! Thin wrapper around the `gh` CLI. The worker owns every side effect;
//! model-authored text never composes shell commands — it only ever ends up
//! as arguments to these fixed invocations.

use std::process::Command;

/// Run `gh <args...>` in the current working directory and return stdout.
pub fn run(args: &[&str]) -> Result<String, String> {
    let output = Command::new("gh")
        .args(args)
        .output()
        .map_err(|e| format!("spawn gh: {e} (is gh installed and authenticated?)"))?;
    if output.status.success() {
        String::from_utf8(output.stdout).map_err(|_| "gh produced non-UTF-8 output".to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("gh {} failed: {}", args.join(" "), stderr.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::run;

    #[test]
    fn missing_gh_subcommand_reports_failure_with_context() {
        // `gh` is installed in CI/dev environments; an invalid subcommand
        // must surface stderr context instead of panicking.
        let err =
            run(&["definitely-not-a-subcommand"]).expect_err("invalid gh subcommand must fail");
        assert!(err.contains("gh definitely-not-a-subcommand failed"));
    }
}
