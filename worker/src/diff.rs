//! PR diff extraction and noise filtering (`quickloop diff <pr>`).
//!
//! Planned in `feat/worker-diff`: strip lockfiles, `dist/`, `vendor/`,
//! generated paths, and binary hunks; bound hunk context to 3 lines; emit
//! compact single-line JSON. Target latency < 400 ms (network-bound on `gh`).

/// Execute `quickloop diff <pr>`.
pub fn run(_pr: &str) -> Result<(), String> {
    Err("quickloop diff is not implemented yet (planned: feat/worker-diff)".into())
}
