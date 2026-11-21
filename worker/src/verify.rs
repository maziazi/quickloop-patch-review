//! Concurrent verification and deterministic scoring (`quickloop verify`).
//!
//! Planned in `feat/worker-verify`: run test/lint/security/scope checks
//! concurrently via `std::process::Command`, compute
//! `score = 40·S_test + 30·S_lint + 20·S_sec + 10·S_scope`, and emit compact
//! JSON. This command must ALWAYS exit 0 when the verifier itself runs — a
//! score < 100 is structured loop data, never an execution failure. Any
//! protected-path edit zeroes `S_scope`, reverts the iteration patch, and
//! reports `BLOCKED` (enforced in every mode, including `--dry-run`).

/// Protected test-path rule matching `tests/**`, `spec/**`, and `**/*_test.*`
/// (plus common test-name conventions). A directory segment named `tests`,
/// `spec`, or `__tests__` protects everything below it.
pub fn is_protected_path(path: &str) -> bool {
    let p = path.trim_start_matches("./");
    let segments: Vec<&str> = p.split('/').collect();
    let dir_hit = segments
        .iter()
        .take(segments.len().saturating_sub(1))
        .any(|seg| *seg == "tests" || *seg == "spec" || *seg == "__tests__");
    if dir_hit {
        return true;
    }
    let name = segments.last().copied().unwrap_or("");
    name == "conftest.py"
        || name.starts_with("test_")
        || name.contains("_test.")
        || name.contains(".test.")
}

/// Protected paths among the files changed by the current iteration — the
/// input to the `S_scope` anti-cheat check.
// Consumed by `feat/worker-verify` when the scope check lands.
#[allow(dead_code)]
pub(crate) fn scope_violations(changed: &[String]) -> Vec<String> {
    changed
        .iter()
        .filter(|p| is_protected_path(p))
        .cloned()
        .collect()
}

/// Execute `quickloop verify`.
pub fn run() -> Result<(), String> {
    Err("quickloop verify is not implemented yet (planned: feat/worker-verify)".into())
}

#[cfg(test)]
mod tests {
    use super::{is_protected_path, scope_violations};

    #[test]
    fn protected_paths_cover_test_dirs_and_test_names() {
        assert!(is_protected_path("demo-repo/tests/regression_test.rs"));
        assert!(is_protected_path("tests/util.rs"));
        assert!(is_protected_path("a/b/spec/models.rb"));
        assert!(is_protected_path("web/src/__tests__/app.jsx"));
        assert!(is_protected_path("src/parser_test.rs"));
        assert!(is_protected_path("js/app.test.tsx"));
        assert!(is_protected_path("tests/conftest.py"));
        assert!(!is_protected_path("src/lib.rs"));
        assert!(
            !is_protected_path("contest/entry.rs"),
            "'contest' is not 'tests'"
        );
        assert!(
            !is_protected_path("src/testing_helper.rs"),
            "substring hits must not match"
        );
    }

    #[test]
    fn scope_violations_report_only_protected_hits() {
        let changed = vec![
            "demo-repo/src/scoring.rs".to_string(),
            "demo-repo/tests/regression_test.rs".to_string(),
        ];
        let hits = scope_violations(&changed);
        assert_eq!(hits, vec!["demo-repo/tests/regression_test.rs"]);
    }
}
