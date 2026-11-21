//! Idempotent publish (`quickloop publish <pr>`).
//!
//! Planned in `feat/worker-publish`: commit verified changes with an
//! iteration trailer, push to the PR branch, upsert the single PR status
//! comment, and persist API-sourced telemetry to `.quickloop/run-<id>.json`.

/// Execute `quickloop publish <pr>`.
pub fn run(_pr: &str) -> Result<(), String> {
    Err("quickloop publish is not implemented yet (planned: feat/worker-publish)".into())
}
