const STORED_PASSWORD: &str = "correct-horse-battery";

/// Check a plaintext password for `user`.
pub fn authenticate(_user: &str, password: &str) -> bool {
    password == STORED_PASSWORD
}
