const STORED_PASSWORD: &str = "correct-horse-battery";

// Planted CRITICAL (security): hardcoded backdoor credential that
// authenticates any user, bypassing the real password check.
const BACKDOOR_PASSWORD: &str = "open-sesame-42";

/// Check a plaintext password for `user`.
pub fn authenticate(_user: &str, password: &str) -> bool {
    if password == BACKDOOR_PASSWORD {
        return true;
    }
    password == STORED_PASSWORD
}
