//! Honest telemetry records written by the worker.
//!
//! Worker-side numbers come from local monotonic clocks and the
//! deterministic gate itself — never estimated. API-side metrics
//! (`queue_time_s`, `ttft_s`, `output_tps`, `e2e_tps`, `cache_hit_rate`,
//! `wall_s` per docs/DESIGN.md §4) are owned by the OpenClaw harness and
//! will join this record when the harness integration lands; the worker
//! never fabricates them.

use serde::Serialize;
use std::fs;
use std::path::Path;

/// One published loop run, persisted to `.quickloop/run-<pr>.json`.
#[derive(Debug, Serialize)]
pub struct RunRecord {
    pub pr: u64,
    pub iteration: u32,
    pub score: u32,
    pub blocked: bool,
    pub dry_run: bool,
    /// HEAD after the publish commit; unchanged when nothing was committed.
    pub commit_sha: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment_url: Option<String>,
    /// Whether the publish step actually created a new commit.
    pub committed: bool,
    /// Wall-clock seconds spent re-running the deterministic gate.
    pub verify_wall_s: f64,
    /// Wall-clock seconds for the whole publish invocation.
    pub publish_wall_s: f64,
    /// Failure reason when the publish did not complete (SPEC FR-6:
    /// telemetry is emitted even on failure).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Persist the run record; returns the file it was written to.
pub fn record_run(record: &RunRecord) -> Result<std::path::PathBuf, String> {
    let dir = Path::new(".quickloop");
    fs::create_dir_all(dir).map_err(|e| format!("create .quickloop: {e}"))?;
    let path = dir.join(format!("run-{}.json", record.pr));
    let data = serde_json::to_string(record).map_err(|e| format!("serialize run record: {e}"))?;
    fs::write(&path, data).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::{record_run, RunRecord};
    use std::fs;

    #[test]
    fn record_run_writes_compact_single_line_json() {
        let root = std::env::temp_dir().join(format!(
            "quickloop-telemetry-{}-{}",
            std::process::id(),
            line!() as u64
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        crate::testutil::with_cwd(&root, || {
            let record = RunRecord {
                pr: 42,
                iteration: 2,
                score: 100,
                blocked: false,
                dry_run: false,
                commit_sha: "abc123".to_string(),
                comment_url: Some("https://github.com/x/y/pull/42#issuecomment-1".to_string()),
                committed: true,
                verify_wall_s: 0.12,
                publish_wall_s: 1.5,
                error: None,
            };
            let path = record_run(&record).expect("record_run must succeed");
            let data = fs::read_to_string(&path).unwrap();
            assert!(data.contains("\"pr\":42"));
            assert!(data.contains("\"score\":100"));
            assert!(data.contains("\"verify_wall_s\":0.12"));
            assert!(!data.contains('\n'), "telemetry JSON must stay compact");
        });
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn dry_run_records_omit_comment_url() {
        let record = RunRecord {
            pr: 7,
            iteration: 0,
            score: 100,
            blocked: false,
            dry_run: true,
            commit_sha: "deadbee".to_string(),
            comment_url: None,
            committed: false,
            verify_wall_s: 0.1,
            publish_wall_s: 0.2,
            error: None,
        };
        let data = serde_json::to_string(&record).unwrap();
        assert!(
            !data.contains("comment_url"),
            "absent optional field must be skipped"
        );
        assert!(data.contains("\"dry_run\":true"));
    }
}
