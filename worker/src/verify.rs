//! Concurrent verification and deterministic scoring (`quickloop verify`).
//!
//! Planned in `feat/worker-verify`: run test/lint/security/scope checks
//! concurrently via `std::process::Command`, compute
//! `score = 40·S_test + 30·S_lint + 20·S_sec + 10·S_scope`, and emit compact
//! JSON. This command must ALWAYS exit 0 when the verifier itself runs — a
//! score < 100 is structured loop data, never an execution failure. Any
//! protected-path edit zeroes `S_scope`, reverts the iteration patch, and
//! reports `BLOCKED` (enforced in every mode, including `--dry-run`).

/// Execute `quickloop verify`.
pub fn run() -> Result<(), String> {
    Err("quickloop verify is not implemented yet (planned: feat/worker-verify)".into())
}
