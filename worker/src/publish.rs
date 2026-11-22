//! Idempotent publish (`quickloop publish <pr>`).
//!
//! Re-runs the deterministic gate first and refuses to publish anything
//! that has not verifiably reached `score = 100` — the model never decides
//! readiness, not even by asking the worker to publish. The checkout is
//! cross-checked against the PR reference before any public write, and the
//! push targets the PR's head branch explicitly. Then: commit the verified
//! working tree (staged via `git add -u`, so worker-owned files —
//! `.quickloop/` telemetry, `target/` build output — can never enter the
//! commit regardless of the target repo's ignore rules) with a
//! `QuickLoop-Iteration` trailer, push, upsert the single PR status
//! comment (marker-based, never spammed), and persist an honest telemetry
//! record — also on failures. `--dry-run` performs the gate re-check and
//! telemetry write but guarantees zero remote writes. Protected-path
//! enforcement lives in `verify::evaluate` and is active in every publish
//! path.

use crate::diff::pr_number;
use crate::gh;
use crate::git;
use crate::telemetry::{record_run, RunRecord};
use crate::verify;
use serde_json::json;
use std::fs;
use std::path::Path;
use std::time::Instant;

/// Hidden marker that makes the PR status comment idempotent. A hidden
/// HTML comment is deliberately obscure; a human quoting it verbatim in
/// their own comment would be adopted by the upsert — acceptable for a
/// worker-owned status thread.
const COMMENT_MARKER: &str = "<!-- quickloop:status -->";

/// Outcome of the side-effect phase, recorded even on failure.
struct PublishOutcome {
    commit_sha: String,
    comment_url: Option<String>,
    committed: bool,
}

/// Execute `quickloop publish <pr>`.
pub fn run(pr_ref: &str, dry_run: bool, iteration: u32) -> Result<(), String> {
    let started = Instant::now();
    let pr = pr_number(pr_ref)?;
    // Runtime artifacts must never leak into the user's repository — not
    // via staging (`git add -u`), not via untracked-file dirty detection.
    git::ensure_local_exclude(".quickloop/")?;

    // Deterministic gate re-check — publish refuses unverified state.
    let verify_started = Instant::now();
    let gate = verify::evaluate()?;
    let verify_wall_s = verify_started.elapsed().as_secs_f64();
    if gate.blocked || gate.score != 100 {
        let reason = format!(
            "publish refused: gate not passed (score {}, blocked {})",
            gate.score, gate.blocked
        );
        record_run(&failure_record(
            started,
            verify_wall_s,
            pr,
            iteration,
            &gate,
            &reason,
        ))
        .ok();
        return Err(reason);
    }
    // The checkout must be the PR this invocation names — `git push` is a
    // public write and must never land on the wrong branch.
    let baseline = verify::read_baseline()?;
    if baseline.pr != pr {
        let reason = format!(
            "checkout baseline is for PR {} but publish targets PR {pr} — run `quickloop diff {pr_ref}` first",
            baseline.pr
        );
        record_run(&failure_record(
            started,
            verify_wall_s,
            pr,
            iteration,
            &gate,
            &reason,
        ))
        .ok();
        return Err(reason);
    }

    let outcome = apply_publish(pr_ref, pr, dry_run, iteration, gate.score);
    if let Err(err) = &outcome {
        // SPEC FR-6: telemetry is emitted even on failure.
        record_run(&failure_record(
            started,
            verify_wall_s,
            pr,
            iteration,
            &gate,
            err,
        ))
        .ok();
        return Err(err.clone());
    }
    let outcome = outcome?;

    let record = RunRecord {
        pr,
        iteration,
        score: gate.score,
        blocked: gate.blocked,
        dry_run,
        commit_sha: outcome.commit_sha.clone(),
        comment_url: outcome.comment_url.clone(),
        committed: outcome.committed,
        verify_wall_s,
        publish_wall_s: started.elapsed().as_secs_f64(),
        error: None,
    };
    record_run(&record)?;

    let output = json!({
        "ok": true,
        "pr": pr,
        "commit_sha": outcome.commit_sha,
        "comment_url": outcome.comment_url,
        "committed": outcome.committed,
        "dry_run": dry_run,
    });
    println!("{output}");
    Ok(())
}

/// Commit/push/comment phase, separated so failures are recorded.
fn apply_publish(
    pr_ref: &str,
    pr: u64,
    dry_run: bool,
    iteration: u32,
    score: u32,
) -> Result<PublishOutcome, String> {
    let mut commit_sha = git::run(&["rev-parse", "HEAD"])?.trim().to_string();
    let mut committed = false;
    if !dry_run {
        let dirty = !git::run(&["status", "--porcelain", "--", ":(exclude).quickloop"])?
            .trim()
            .is_empty();
        if dirty {
            // `git add -u` stages modifications and deletions of *tracked*
            // files only — the loop's patch phase can never create files
            // (patch.rs), so this covers every fix it can emit, and
            // worker-generated untracked artifacts (`target/`, `Cargo.lock`,
            // `.quickloop/` telemetry) cannot leak into the commit even on
            // target repos with no committed ignore rules. Naming
            // `.quickloop` via an `:(exclude)` pathspec here would make git
            // treat the ignored directory as explicitly requested and abort
            // with "paths are ignored" — caught by the end-to-end loop
            // proof; and `git add -A` sweeps untracked build output into
            // the PR (review of this exact line).
            git::run(&["add", "-u"])?;
            let message = format!(
                "fix: apply QuickLoop patch (verified score 100)\n\nQuickLoop-Iteration: {iteration}"
            );
            git::run(&["commit", "-q", "-m", &message])?;
            committed = true;
        }
        // Push explicitly to the PR's head branch: whatever branch is
        // checked out must never silently land somewhere else.
        let head_ref = gh::run(&[
            "pr",
            "view",
            pr_ref,
            "--json",
            "headRefName",
            "-q",
            ".headRefName",
        ])?
        .trim()
        .to_string();
        git::run(&["push", "origin", &format!("HEAD:{head_ref}")])?;
        commit_sha = git::run(&["rev-parse", "HEAD"])?.trim().to_string();
    }

    let comment_url = if dry_run {
        None
    } else {
        Some(upsert_comment(pr, iteration, score, &commit_sha)?)
    };
    Ok(PublishOutcome {
        commit_sha,
        comment_url,
        committed,
    })
}

/// Best-effort telemetry record for a failed or refused publish.
fn failure_record(
    started: Instant,
    verify_wall_s: f64,
    pr: u64,
    iteration: u32,
    gate: &verify::VerifyOutput,
    error: &str,
) -> RunRecord {
    RunRecord {
        pr,
        iteration,
        score: gate.score,
        blocked: gate.blocked,
        dry_run: false,
        commit_sha: git::run(&["rev-parse", "HEAD"])
            .map(|s| s.trim().to_string())
            .unwrap_or_default(),
        comment_url: None,
        committed: false,
        verify_wall_s,
        publish_wall_s: started.elapsed().as_secs_f64(),
        error: Some(error.to_string()),
    }
}

/// Create or update the single marker-bearing PR status comment and return
/// its URL. Fetches up to 100 comments (the status comment sits at the
/// tail of the thread; the loop is sequential so later pages only matter
/// on pathologically long PRs).
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
    let comments = gh::run(&[
        "api",
        &format!("repos/{slug}/issues/{pr}/comments?per_page=100"),
    ])?;
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
            // SPEC FR-6: telemetry is emitted even on failure.
            let record = fs::read_to_string(root.join(".quickloop/run-42.json")).unwrap();
            assert!(
                record.contains("\"error\""),
                "refusal must be recorded: {record}"
            );
        });
        let _ = fs::remove_dir_all(root);
    }

    /// A minimal cargo project whose gate PASSES (no dependencies, one
    /// inline passing test), with a fake `gh` shim on PATH that records
    /// every invocation — proving the --dry-run zero-remote-writes
    /// guarantee offline.
    #[cfg(unix)]
    fn passing_gate_fixture(tag: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let root =
            std::env::temp_dir().join(format!("quickloop-publish-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"fixture-ok\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::write(
            root.join("src/lib.rs"),
            "pub fn one() -> u32 { 1 }\n\n#[test]\nfn one_is_one() {\n    assert_eq!(one(), 1);\n}\n",
        )
        .unwrap();
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
        // Fake gh: records its argv, never touches the network.
        let shim = root.join("bin/gh");
        fs::write(
            &shim,
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$GH_CALLS\"\necho '[]'\n",
        )
        .unwrap();
        let mut perms = fs::metadata(&shim).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&shim, perms).unwrap();
        root
    }

    /// Passing-gate fixture extended for the REAL publish path: repo-local
    /// git identity (the production commit runs without `-c`, so an
    /// identity-less machine must still work), a local bare remote as
    /// `origin` (so `git push` works offline), and a smarter `gh` shim
    /// answering each call shape the publish flow makes.
    #[cfg(unix)]
    fn real_publish_fixture(tag: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let root = passing_gate_fixture(tag);
        git_in(&root, &["config", "user.email", "quickloop@test"]);
        git_in(&root, &["config", "user.name", "quickloop"]);
        let origin = std::env::temp_dir().join(format!(
            "quickloop-publish-{tag}-{}-origin.git",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&origin);
        let origin_arg = origin.to_string_lossy().to_string();
        git_in(&root, &["init", "--bare", "-q", &origin_arg]);
        git_in(&root, &["remote", "add", "origin", &origin_arg]);
        git_in(&root, &["push", "-q", "origin", "main"]);
        let shim = root.join("bin/gh");
        fs::write(
            &shim,
            "#!/bin/sh\n\
             printf '%s\\n' \"$*\" >> \"$GH_CALLS\"\n\
             case \"$*\" in\n\
             *headRefName*) echo main ;;\n\
             *nameWithOwner*) echo test/fixture ;;\n\
             *--input*) echo '{\"html_url\":\"https://example.invalid/comments/9\"}' ;;\n\
             *) echo '[]' ;;\n\
             esac\n",
        )
        .unwrap();
        let mut perms = fs::metadata(&shim).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&shim, perms).unwrap();
        root
    }

    /// Failure-proof restore of the process env a test mutated: runs on
    /// Drop, so an assertion failure can no longer leak the shim PATH or a
    /// stale `GH_CALLS` into the remainder of the test process.
    #[cfg(unix)]
    struct EnvGuard {
        original_path: String,
    }

    #[cfg(unix)]
    impl EnvGuard {
        fn install(shim_dir: &Path, gh_calls: &Path) -> Self {
            let original_path = std::env::var("PATH").unwrap_or_default();
            // Prepend the shim dir: PATH is searched in order, and the
            // developer's real `gh` lives earlier in the original PATH.
            std::env::set_var("PATH", format!("{}:{}", shim_dir.display(), original_path));
            std::env::set_var("GH_CALLS", gh_calls);
            Self { original_path }
        }
    }

    #[cfg(unix)]
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            std::env::remove_var("GH_CALLS");
            std::env::set_var("PATH", &self.original_path);
        }
    }

    #[cfg(unix)]
    #[test]
    fn real_publish_commits_pushes_and_upserts_comment_offline() {
        let root = real_publish_fixture("real-ok");
        // The "model patch": a dirty-but-valid source tweak that keeps the
        // gate green — and `.quickloop/` telemetry present to prove it is
        // never staged.
        fs::create_dir_all(root.join(".quickloop")).unwrap();
        fs::write(root.join(".quickloop/artifact.txt"), "telemetry\n").unwrap();
        fs::write(root.join("src/lib.rs"), "pub fn one() -> u32 { 1 } // touched\n\n#[test]\nfn one_is_one() {\n    assert_eq!(one(), 1);\n}\n").unwrap();
        let gh_calls = root.join("gh-calls.log");
        let shim_dir = root.join("bin");
        with_cwd(&root, || {
            let _env = EnvGuard::install(&shim_dir, &gh_calls);
            let commits_before = git_in(&root, &["rev-list", "--count", "HEAD"])
                .trim()
                .to_string();

            run("42", false, 1).expect("real publish on a passing gate must succeed");

            assert_eq!(
                git_in(&root, &["rev-list", "--count", "HEAD"]).trim(),
                (commits_before.parse::<u32>().unwrap() + 1).to_string(),
                "real publish must commit exactly once"
            );
            let message = git_in(&root, &["log", "-1", "--format=%B"]).to_lowercase();
            assert!(message.contains("score 100"), "message: {message}");
            assert!(
                message.contains("quickloop-iteration: 1"),
                "trailer missing: {message}"
            );
            let files = git_in(&root, &["show", "--name-only", "--format=", "HEAD"]);
            assert!(!files.contains(".quickloop"), "telemetry leaked: {files}");
            // The gate's own cargo run leaves untracked `target/` and
            // `Cargo.lock` behind (this fixture has no committed
            // .gitignore) — staging must never sweep them into the PR.
            assert!(!files.contains("target/"), "build output leaked: {files}");
            assert!(!files.contains("Cargo.lock"), "lockfile leaked: {files}");
            assert_eq!(
                files.trim(),
                "src/lib.rs",
                "published commit must touch exactly the patched file: {files}"
            );
            let pushed = git_in(&root, &["ls-remote", "origin", "refs/heads/main"]);
            let local = git_in(&root, &["rev-parse", "HEAD"]);
            assert_eq!(
                pushed.trim().split('\t').next().unwrap_or_default(),
                local.trim(),
                "origin/main must match the published commit"
            );
            let calls = fs::read_to_string(&gh_calls).unwrap();
            assert!(calls.contains("headRefName"), "calls: {calls}");
            assert!(calls.contains("comments"), "calls: {calls}");
            let record = fs::read_to_string(root.join(".quickloop/run-42.json")).unwrap();
            assert!(record.contains("\"dry_run\":false"), "record: {record}");
            assert!(record.contains("\"committed\":true"), "record: {record}");
            assert!(record.contains("example.invalid"), "record: {record}");
            assert!(
                !record.contains("\"error\""),
                "successful publish must not carry an error field: {record}"
            );
        });
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(std::env::temp_dir().join(format!(
            "quickloop-publish-real-ok-{}-origin.git",
            std::process::id()
        )));
    }

    #[cfg(unix)]
    #[test]
    fn dry_run_happy_path_makes_zero_gh_calls_and_no_commit() {
        let root = passing_gate_fixture("dry-run-ok");
        // The "model patch": a dirty-but-valid source tweak that keeps the gate green.
        fs::write(root.join("src/lib.rs"), "pub fn one() -> u32 { 1 } // touched\n\n#[test]\nfn one_is_one() {\n    assert_eq!(one(), 1);\n}\n").unwrap();
        let gh_calls = root.join("gh-calls.log");
        let shim_dir = root.join("bin");
        with_cwd(&root, || {
            let _env = EnvGuard::install(&shim_dir, &gh_calls);
            let commits_before = git_in(&root, &["rev-list", "--count", "HEAD"])
                .trim()
                .to_string();

            run("42", true, 1).expect("dry-run on a passing gate must succeed");

            assert!(
                !gh_calls.exists(),
                "dry-run must never invoke gh, saw: {:?}",
                fs::read_to_string(&gh_calls)
            );
            assert_eq!(
                git_in(&root, &["rev-list", "--count", "HEAD"]).trim(),
                commits_before,
                "dry-run must not commit"
            );
            let record = fs::read_to_string(root.join(".quickloop/run-42.json")).unwrap();
            assert!(record.contains("\"dry_run\":true"), "record: {record}");
            assert!(
                !record.contains("\"error\""),
                "successful dry-run must not carry an error field: {record}"
            );
            assert!(record.contains("\"score\":100"));
        });
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn publish_refuses_invalid_pr_reference_before_anything_else() {
        let err = run("-R evil/repo", false, 0).expect_err("flag injection must be rejected");
        assert!(err.contains("invalid PR reference"));
    }
}
