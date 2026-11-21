//! Atomic SEARCH/REPLACE application (`quickloop patch <file>`).
//!
//! Planned in `feat/worker-patch`: every SEARCH block must match the target
//! file byte-for-byte exactly once; zero matches or an ambiguous match aborts
//! the entire patch set (exit code 2). No file creation or deletion.

use std::path::Path;

/// Execute `quickloop patch <file>`.
pub fn run(_file: &Path) -> Result<(), String> {
    Err("quickloop patch is not implemented yet (planned: feat/worker-patch)".into())
}
