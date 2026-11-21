//! Idempotent publish (`quickloop publish <pr>`).
//!
//! Re-runs the deterministic gate first and refuses to publish anything
//! that has not verifiably reached `score = 100` — the model never decides
//! readiness, not even by asking the worker to publish. Then: commit the
//! verified working tree with a `QuickLoop-Iteration` trailer, push to the
//! PR branch, upsert the single PR status comment (marker-based, never
//! spammed), and persist an honest telemetry record. `--dry-run` performs
//! the gate re-check and telemetry write but guarantees zero remote
//! writes. Protected-path enforcement lives in `verify::evaluate` and is
//! active in every publish path.

use crate::diff::pr_number;
use crate::gh;
use crate::git;
use crate::telemetry::{record_run, RunRecord};
use crate::verify;
use serde_json::json;
use std::fs;
use std::path::Path;
use std::time::Instant;

/// Hidden marker that makes the PR status comment idempotent.
const COMMENT_MARKER: &str = "<!-- quickloop:status -->";

/// Execute `quickloop publish <pr>`.
pub fn run(pr_ref: &str, dry_run: bool, iteration: u32) -> Result<(), String> {
    let started = Instant::now();
    let pr = pr_number(pr_ref)?;

    // Deterministic gate re-check — publish refuses unverified state.
    let verify_started = Instant::now();
    let gate = verify::evaluate()?;
    let verify_wall_s = verify_started.elapsed().as_secs_f64();
    if gate.blocked || gate.score != 100 {
        return Err(format!(
            "publish refused: gate not passed (score {}, blocked {})",
            gate.score, gate.blocked
        ));
    }

    let mut commit_sha = git::run(&["rev-parse", "HEAD"])?.trim().to_string();
    let mut committed = false;
    if !dry_run {
        let dirty = !git::run(&["status", "--porcelain"])?.trim().is_empty();
        if dirty {
            git::run(&["add", "-A"])?;
            let message = format!(
                "fix: apply QuickLoop patch (verified score 100)\n\nQuickLoop-Iteration: {iteration}"
            );
            git::run(&["commit", "-q", "-m", &message])?;
            committed = true;
        }
        git::run(&["push", "origin", "HEAD"])?;
        commit_sha = git::run(&["rev-parse", "HEAD"])?.trim().to_string();
    }

    let comment_url = if dry_run {
        None
    } else {
        Some(upsert_comment(pr, iteration, gate.score, &commit_sha)?)
    };

    let record = RunRecord {
        pr,
        iteration,
        score: gate.score,
        blocked: gate.blocked,
        dry_run,
        commit_sha: commit_sha.clone(),
        comment_url: comment_url.clone(),
        committed,
        verify_wall_s,
        publish_wall_s: started.elapsed().as_secs_f64(),
    };
    record_run(&record)?;

    let output = json!({
        "ok": true,
        "pr": pr,
        "commit_sha": commit_sha,
        "comment_url": comment_url,
        "committed": committed,
        "dry_run": dry_run,
    });
    println!("{output}");
    Ok(())
}

/// Create or update the single marker-bearing PR status comment and return
/// its URL.
fn upsert_comment(pr: u64, iteration: u32, score: u32, commit_sha: &str) -> Result<String, String> {
    let slug = gh::run(&[
        "repo",
        "view",
        "--json",
        "nameWithOwner",
        "-q",
        ".nameWithOwner",
    ])?
    .trim()
    .to_string();
    let comments = gh::run(&["api", &format!("repos/{slug}/issues/{pr}/comments")])?;
    let existing = find_marked_comment_id(&comments, COMMENT_MARKER);
    let body = build_report(iteration, score, commit_sha);
    let body_path = write_comment_body(&body)?;

    let response = match existing {
        Some(id) => gh::run(&[
            "api",
            "-X",
            "PATCH",
            &format!("repos/{slug}/issues/comments/{id}"),
            "--input",
            &body_path,
        ])?,
        None => gh::run(&[
            "api",
            "-X",
            "POST",
            &format!("repos/{slug}/issues/{pr}/comments"),
            "--input",
            &body_path,
        ])?,
    };
    let response: serde_json::Value = serde_json::from_str(response.trim())
        .map_err(|e| format!("gh api comment response invalid: {e}"))?;
    response["html_url"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "gh api comment response missing html_url".to_string())
}

/// Find the id of the comment carrying the QuickLoop status marker.
fn find_marked_comment_id(comments_json: &str, marker: &str) -> Option<u64> {
    let value: serde_json::Value = serde_json::from_str(comments_json.trim()).ok()?;
    value.as_array()?.iter().find_map(|comment| {
        let body = comment.get("body")?.as_str()?;
        if body.contains(marker) {
            comment.get("id")?.as_u64()
        } else {
            None
        }
    })
}

/// Build the markdown status report body (starts with the idempotency
/// marker; stays concise to keep the PR thread clean).
fn build_report(iteration: u32, score: u32, commit_sha: &str) -> String {
    format!(
        "{COMMENT_MARKER}\n\
         ## QuickLoop · READY_TO_MERGE\n\
         - score: {score}/100 (40·test + 30·lint + 20·sec + 10·scope, all binary)\n\
         - iteration: {iteration}\n\
         - commit: `{commit_sha}`\n\
         - gate: deterministic (`quickloop verify`); the model never decides readiness\n\
         - telemetry: `.quickloop/run-*.json` (worker-measured, API-sourced metrics via OpenClaw)"
    )
}

/// Write the comment body as JSON for `gh api --input` (avoids arg-length
/// and quoting pitfalls). The file lives under gitignored `.quickloop/`.
fn write_comment_body(body: &str) -> Result<String, String> {
    fs::create_dir_all(Path::new(".quickloop")).map_err(|e| format!("create .quickloop: {e}"))?;
    let path = Path::new(".quickloop").join("comment-body.json");
    let payload = json!({"body": body}).to_string();
    fs::write(&path, payload).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(path.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::{build_report, find_marked_comment_id, run, write_comment_body, COMMENT_MARKER};
    use crate::testutil::with_cwd;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn git_in(root: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("git should spawn");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).expect("utf8")
    }

    /// Committed git repo fixture with a baseline but NO passing gate
    /// (there is no cargo project here, so `cargo test` fails by default).
    /// All git calls run with `current_dir(root)` — never the test process
    /// CWD, which belongs to the real repository.
    fn failing_gate_fixture(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("quickloop-publish-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/lib.rs"), "pub fn ok() -> u32 { 1 }\n").unwrap();
        git_in(&root, &["init", "-q", "-b", "main"]);
        git_in(&root, &["add", "-A"]);
        git_in(
            &root,
            &[
                "-c",
                "user.email=quickloop@test",
                "-c",
                "user.name=quickloop",
                "commit",
                "-q",
                "-m",
                "fixture",
            ],
        );
        let head = git_in(&root, &["rev-parse", "HEAD"]).trim().to_string();
        fs::create_dir_all(root.join(".quickloop")).unwrap();
        fs::write(
            root.join(".quickloop/baseline.json"),
            format!(
                r#"{{"pr":42,"base_sha":"{head}","head_sha":"{head}","test_file_count":0,"changed_paths":["src/lib.rs"]}}"#
            ),
        )
        .unwrap();
        root
    }

    #[test]
    fn find_marked_comment_picks_only_the_marker_comment() {
        let fixture = format!(
            r#"[{{"id": 11, "body": "unrelated review note"}},
                {{"id": 22, "body": "a plain note without any marker"}},
                {{"id": 33, "body": "{COMMENT_MARKER}\n## QuickLoop · READY_TO_MERGE"}}]"#
        );
        assert_eq!(find_marked_comment_id(&fixture, COMMENT_MARKER), Some(33));
        assert_eq!(find_marked_comment_id("[]", COMMENT_MARKER), None);
        assert_eq!(find_marked_comment_id("not json", COMMENT_MARKER), None);
    }

    #[test]
    fn report_carries_marker_score_iteration_and_commit() {
        let body = build_report(2, 100, "abc1234");
        assert!(
            body.starts_with(COMMENT_MARKER),
            "marker must be first for idempotency"
        );
        assert!(body.contains("100/100"));
        assert!(body.contains("iteration: 2"));
        assert!(body.contains("`abc1234`"));
    }

    #[test]
    fn comment_body_payload_is_valid_json_under_quickloop() {
        let root =
            std::env::temp_dir().join(format!("quickloop-publish-body-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        with_cwd(&root, || {
            let path = write_comment_body("hello").expect("write must succeed");
            let data = fs::read_to_string(&path).unwrap();
            let value: serde_json::Value = serde_json::from_str(&data).expect("valid JSON");
            assert_eq!(value["body"], "hello");
            assert!(path.contains(".quickloop"));
        });
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn publish_refuses_unverified_state_even_dry_run_and_never_commits() {
        let root = failing_gate_fixture("refused");
        with_cwd(&root, || {
            let commits_before = git_in(&root, &["rev-list", "--count", "HEAD"])
                .trim()
                .to_string();
            let err = run("42", true, 1).expect_err("gate not passed must refuse");
            assert!(err.contains("publish refused"), "got: {err}");
            assert!(
                err.contains("score 0") || err.contains("score "),
                "error names the score: {err}"
            );
            assert_eq!(
                git_in(&root, &["rev-list", "--count", "HEAD"]).trim(),
                commits_before,
                "no commit may be created on refusal"
            );
        });
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn publish_refuses_invalid_pr_reference_before_anything_else() {
        let err = run("-R evil/repo", false, 0).expect_err("flag injection must be rejected");
        assert!(err.contains("invalid PR reference"));
    }
}
