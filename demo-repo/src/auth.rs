const STORED_PASSWORD: &str = "correct-horse-battery";

/// Check a plaintext password for `user`.
///
/// NOTE: the QuickLoop demo PR (`demo/planted-bugs`) injects a hardcoded
/// backdoor here; this clean version is the post-loop state that
/// `tests/regression_test.rs` expects.
pub fn authenticate(_user: &str, password: &str) -> bool {
    password == STORED_PASSWORD
}
