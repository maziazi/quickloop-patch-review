//! Concurrent verification and deterministic scoring (`quickloop verify`).
//!
//! Runs the test/lint/security/scope checks and computes
//! `score = 40·S_test + 30·S_lint + 20·S_sec + 10·S_scope` where every
//! sub-score is strictly binary. This command ALWAYS exits 0 when the
//! verifier itself runs — a score < 100 is structured loop data, never an
//! execution failure. Any protected-path edit zeroes `S_scope`, reverts the
//! iteration patch immediately, and reports `BLOCKED` (enforced in every
//! mode; there is no bypass flag).
//!
//! S_sec semantics: the security scan covers the full current content of
//! every PR-changed file (the spec's "zero new issues" is interpreted as
//! zero issues on the PR's changed surface), excluding Phase-1 noise paths.
//! The scope check runs before the concurrent phase; on its failure the
//! other three checks never run — BLOCKED short-circuits by design.

use serde::Serialize;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::diff::{count_protected_files, Baseline};
use crate::git;

/// Wall-clock cap per spawned tool run (cargo cold builds can be slow).
const TOOL_TIMEOUT_SECS: u64 = 600;

/// Max failure lines reported per check, keeping the replaceable loop tail
/// small enough to stay within the KV-cache-friendly budget.
const MAX_FAILURE_LINES: usize = 20;
const MAX_FAILURE_LINE_CHARS: usize = 300;

/// Static security patterns (lowercase, substring match) scanned over the
/// files changed by this PR. Deliberately narrow to avoid false positives
/// on legitimate constants (e.g. a `STORED_PASSWORD` const does not match
/// `password = "` and carries no `backdoor` keyword).
const SEC_PATTERNS: [&str; 10] = [
    "backdoor",
    "begin rsa private key",
    "begin private key",
    "akia",
    "password = \"",
    "passwd = \"",
    "api_key = \"",
    "apikey = \"",
    "secret = \"",
    "token = \"",
];

#[derive(Debug, Serialize)]
pub struct Checks {
    pub test: u8,
    pub lint: u8,
    pub sec: u8,
    pub scope: u8,
}

#[derive(Debug, Serialize)]
pub struct VerifyOutput {
    pub score: u32,
    pub checks: Checks,
    pub blocked: bool,
    pub failures: Vec<String>,
}

struct CheckResult {
    passed: bool,
    failures: Vec<String>,
}

struct ToolOutput {
    success: bool,
    timed_out: bool,
    stdout: String,
    stderr: String,
}

/// Protected test-path rule matching `tests/**`, `spec/**`, and `**/*_test.*`
/// (plus common test-name conventions). A directory segment named `tests`,
/// `spec`, or `__tests__` protects everything below it.
pub fn is_protected_path(path: &str) -> bool {
    let p = path.trim_start_matches("./");
    let segments: Vec<&str> = p.split('/').collect();
    let dir_hit = segments
        .iter()
        .take(segments.len().saturating_sub(1))
        .any(|seg| *seg == "tests" || *seg == "spec" || *seg == "__tests__");
    if dir_hit {
        return true;
    }
    let name = segments.last().copied().unwrap_or("");
    name == "conftest.py"
        || name.starts_with("test_")
        || name.contains("_test.")
        || name.contains(".test.")
}

/// Protected paths among the files changed by the current iteration — the
/// input to the `S_scope` anti-cheat check.
fn scope_violations(changed: &[String]) -> Vec<String> {
    changed
        .iter()
        .filter(|p| is_protected_path(p))
        .cloned()
        .collect()
}

/// Execute `quickloop verify`: print compact JSON, always exit 0.
pub fn run() -> Result<(), String> {
    let output = evaluate()?;
    println!(
        "{}",
        serde_json::to_string(&output).map_err(|e| format!("serialize verify: {e}"))?
    );
    Ok(())
}

/// Evaluate the four checks and score them.
pub fn evaluate() -> Result<VerifyOutput, String> {
    let baseline = read_baseline()?;
    let changed = changed_since_baseline(&baseline.head_sha)?;
    let scope_hits = scope_violations(&changed);
    let test_file_count = count_protected_files(Path::new("."))?;
    let count_drift = test_file_count != baseline.test_file_count;

    if !scope_hits.is_empty() || count_drift {
        // Anti-cheat: revert the whole iteration immediately. This runs in
        // every mode — there is deliberately no flag to disable it.
        revert_working_tree()?;
        let mut failures = scope_hits;
        if count_drift {
            failures.push(format!(
                "protected test file count changed: baseline {}, now {test_file_count}",
                baseline.test_file_count
            ));
        }
        failures.push("working tree reverted; halting with BLOCKED".to_string());
        return Ok(VerifyOutput {
            score: 0,
            checks: Checks {
                test: 0,
                lint: 0,
                sec: 0,
                scope: 0,
            },
            blocked: true,
            failures: bounded(failures),
        });
    }

    let (test, lint, sec) = run_checks_concurrent(&changed)?;
    let checks = Checks {
        test: u8::from(test.passed),
        lint: u8::from(lint.passed),
        sec: u8::from(sec.passed),
        scope: 1,
    };
    let score = score_of(&checks);
    let mut failures = Vec::new();
    failures.extend(test.failures);
    failures.extend(lint.failures);
    failures.extend(sec.failures);
    Ok(VerifyOutput {
        score,
        checks,
        blocked: false,
        failures: bounded(failures),
    })
}

/// `40·S_test + 30·S_lint + 20·S_sec + 10·S_scope` over binary sub-scores.
fn score_of(checks: &Checks) -> u32 {
    40 * u32::from(checks.test)
        + 30 * u32::from(checks.lint)
        + 20 * u32::from(checks.sec)
        + 10 * u32::from(checks.scope)
}

/// Run test/lint/security checks concurrently (cargo serializes itself on
/// the target-dir lock, the security scan is pure I/O).
fn run_checks_concurrent(
    changed: &[String],
) -> Result<(CheckResult, CheckResult, CheckResult), String> {
    let changed_owned = changed.to_vec();
    thread::scope(
        |s| -> Result<(CheckResult, CheckResult, CheckResult), String> {
            let test_handle = s.spawn(run_test_check);
            let lint_handle = s.spawn(run_lint_check);
            let sec_handle = s.spawn(|| run_sec_check(&changed_owned));
            let join = |handle: thread::ScopedJoinHandle<'_, Result<CheckResult, String>>| {
                handle.join().unwrap_or_else(|_| {
                    Ok(CheckResult {
                        passed: false,
                        failures: vec!["check thread panicked".to_string()],
                    })
                })
            };
            Ok((join(test_handle)?, join(lint_handle)?, join(sec_handle)?))
        },
    )
}

fn run_test_check() -> Result<CheckResult, String> {
    let output = run_tool("cargo", &["test", "--workspace"], TOOL_TIMEOUT_SECS)?;
    if output.timed_out {
        return Ok(failed(vec![format!(
            "cargo test timed out after {TOOL_TIMEOUT_SECS}s"
        )]));
    }
    if output.success {
        return Ok(passed());
    }
    Ok(failed(concise_failures(&output.stdout, &output.stderr)))
}

fn run_lint_check() -> Result<CheckResult, String> {
    let output = run_tool(
        "cargo",
        &["clippy", "--workspace", "--", "-D", "warnings"],
        TOOL_TIMEOUT_SECS,
    )?;
    if output.timed_out {
        return Ok(failed(vec![format!(
            "cargo clippy timed out after {TOOL_TIMEOUT_SECS}s"
        )]));
    }
    if output.success {
        return Ok(passed());
    }
    Ok(failed(concise_failures(&output.stdout, &output.stderr)))
}

fn run_sec_check(changed: &[String]) -> Result<CheckResult, String> {
    let mut failures = Vec::new();
    for path in changed {
        // Deleted or non-UTF-8 files carry no scannable content.
        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };
        for (idx, line) in content.lines().enumerate() {
            let lower = line.to_lowercase();
            for pattern in SEC_PATTERNS {
                if lower.contains(pattern) {
                    failures.push(format!("{path}:{}: matched '{pattern}'", idx + 1));
                }
            }
        }
    }
    Ok(failed_or_pass(failures))
}

/// Files changed since the baseline head: committed work on the branch plus
/// everything in the working tree (the loop's uncommitted patches). Both
/// git queries use `-z` (NUL-delimited, never quoted), so non-ASCII and
/// special-character paths survive verbatim for downstream I/O — the
/// line-based `core.quotePath` output would C-quote them into unusable
/// names. Phase-1 noise paths (lockfiles, `dist/`, ...) are filtered with
/// the same rule as the diff intake.
fn changed_since_baseline(head_sha: &str) -> Result<Vec<String>, String> {
    let diff_raw = git::run(&["diff", "--name-only", "-z", head_sha]).map_err(|e| {
        format!("baseline head {head_sha} not resolvable locally (is this the PR repository?): {e}")
    })?;
    let mut changed: Vec<String> = diff_raw
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect();
    changed.extend(status_changed_fields(&git::run(&[
        "status",
        "--porcelain",
        "-z",
    ])?));
    changed.retain(|path| !crate::diff::is_excluded(path));
    changed.sort();
    changed.dedup();
    Ok(changed)
}

/// Parse `git status --porcelain -z` fields. Each entry is `XY <path>`;
/// rename/copy entries are followed by a second NUL field carrying the
/// original path, which is skipped (the destination is what exists on disk).
fn status_changed_fields(raw: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut fields = raw.split('\0').filter(|field| !field.is_empty());
    while let Some(field) = fields.next() {
        if field.len() < 4 {
            continue;
        }
        let status = field.as_bytes()[0];
        paths.push(field[3..].to_string());
        if status == b'R' || status == b'C' {
            fields.next();
        }
    }
    paths
}

pub(crate) fn read_baseline() -> Result<Baseline, String> {
    let data = fs::read_to_string(Path::new(".quickloop").join("baseline.json"))
        .map_err(|_| "no baseline found — run `quickloop diff <pr>` first".to_string())?;
    serde_json::from_str(&data).map_err(|e| format!("corrupt baseline.json: {e}"))
}

/// Revert every working-tree and index change to HEAD (anti-cheat revert).
/// Untracked files are removed as well — an injected protected test must
/// not survive the halt. Gitignored paths (incl. `.quickloop/`, which is
/// also excluded explicitly) survive because `clean` runs without `-x`.
fn revert_working_tree() -> Result<(), String> {
    git::run(&["restore", "--staged", "--worktree", "."])?;
    git::run(&["clean", "-fd", "-e", ".quickloop"])?;
    Ok(())
}

/// Spawn a tool, drain its pipes on reader threads (preventing the classic
/// full-pipe deadlock), and kill it when the wall-clock budget expires.
/// The child runs in its own process group (Unix), so a timeout kill also
/// reaps cargo's `rustc`/build-script grandchildren — otherwise they keep
/// the pipes open and the reader joins would hang past the deadline.
fn run_tool(program: &str, args: &[&str], timeout_secs: u64) -> Result<ToolOutput, String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("spawn {program}: {e}"))?;
    let mut stdout_pipe = child
        .stdout
        .take()
        .ok_or_else(|| format!("{program}: no stdout pipe"))?;
    let mut stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| format!("{program}: no stderr pipe"))?;
    let stdout_thread = thread::spawn(move || {
        let mut buf = String::new();
        let _ = stdout_pipe.read_to_string(&mut buf);
        buf
    });
    let stderr_thread = thread::spawn(move || {
        let mut buf = String::new();
        let _ = stderr_pipe.read_to_string(&mut buf);
        buf
    });
    let started = Instant::now();
    let timeout = Duration::from_secs(timeout_secs);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if started.elapsed() > timeout {
                    // Kill the whole process group so grandchildren (rustc,
                    // build scripts) release the pipes and the reader joins
                    // below cannot hang past the deadline.
                    let _ = child.kill();
                    let _ = child.wait();
                    #[cfg(unix)]
                    {
                        let _ = Command::new("pkill")
                            .args(["-KILL", "-g", &child.id().to_string()])
                            .output();
                    }
                    break None;
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(format!("wait {program}: {e}")),
        }
    };
    let stdout = stdout_thread.join().unwrap_or_default();
    let stderr = stderr_thread.join().unwrap_or_default();
    Ok(match status {
        Some(status) => ToolOutput {
            success: status.success(),
            timed_out: false,
            stdout,
            stderr,
        },
        None => ToolOutput {
            success: false,
            timed_out: true,
            stdout,
            stderr,
        },
    })
}

fn passed() -> CheckResult {
    CheckResult {
        passed: true,
        failures: Vec::new(),
    }
}

fn failed(failures: Vec<String>) -> CheckResult {
    CheckResult {
        passed: false,
        failures,
    }
}

fn failed_or_pass(failures: Vec<String>) -> CheckResult {
    if failures.is_empty() {
        passed()
    } else {
        failed(failures)
    }
}

/// Keep failure output bounded so the loop's replaceable tail stays small.
fn bounded(failures: Vec<String>) -> Vec<String> {
    failures
        .into_iter()
        .take(MAX_FAILURE_LINES)
        .map(|line| {
            let truncated: String = line.chars().take(MAX_FAILURE_LINE_CHARS).collect();
            if truncated.len() < line.len() {
                format!("{truncated}...")
            } else {
                truncated
            }
        })
        .collect()
}

/// Distill raw tool output into concise, actionable failure lines.
fn concise_failures(stdout: &str, stderr: &str) -> Vec<String> {
    let mut failures: Vec<String> = stdout
        .lines()
        .chain(stderr.lines())
        .map(str::trim_start)
        .filter(|line| {
            line.contains("FAILED")
                || line.starts_with("thread '")
                || line.contains("panicked at")
                || line.starts_with("error")
                || line.starts_with("warning: unused")
        })
        .map(str::to_string)
        .collect();
    if failures.is_empty() {
        // No recognizable markers — fall back to the last lines of output.
        let tail: Vec<&str> = stderr.lines().rev().take(5).collect();
        failures.extend(tail.into_iter().rev().map(str::to_string));
    }
    if failures.is_empty() {
        let tail: Vec<&str> = stdout.lines().rev().take(5).collect();
        failures.extend(tail.into_iter().rev().map(str::to_string));
    }
    bounded(failures)
}

#[cfg(test)]
mod tests {
    use super::{
        changed_since_baseline, evaluate, read_baseline, score_of, status_changed_fields,
        SEC_PATTERNS,
    };
    use crate::testutil::with_cwd;
    use crate::verify::Checks;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn git(args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .output()
            .expect("git should spawn");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).expect("utf8")
    }

    /// Build a committed git repo fixture: `src/lib.rs` + one protected
    /// test file, both committed, clean working tree.
    fn git_fixture(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("quickloop-verify-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::write(root.join("src/lib.rs"), "pub fn ok() -> u32 { 1 }\n").unwrap();
        fs::write(
            root.join("tests/t.rs"),
            "#[test]\nfn t() { assert_eq!(1, 1); }\n",
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
        root
    }

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

    #[test]
    fn scoring_matches_the_documented_weights() {
        let all_pass = Checks {
            test: 1,
            lint: 1,
            sec: 1,
            scope: 1,
        };
        assert_eq!(score_of(&all_pass), 100);
        let no_tests = Checks {
            test: 0,
            lint: 1,
            sec: 1,
            scope: 1,
        };
        assert_eq!(score_of(&no_tests), 60);
        let only_scope = Checks {
            test: 0,
            lint: 0,
            sec: 0,
            scope: 1,
        };
        assert_eq!(score_of(&only_scope), 10);
    }

    #[test]
    fn protected_test_edit_zeroes_scope_and_reverts() {
        let root = git_fixture("blocked");
        with_cwd(&root, || {
            let head = git(&["rev-parse", "HEAD"]).trim().to_string();
            fs::create_dir_all(".quickloop").unwrap();
            fs::write(
                ".quickloop/baseline.json",
                format!(
                    r#"{{"pr":42,"base_sha":"{head}","head_sha":"{head}","test_file_count":1,"changed_paths":["src/lib.rs"]}}"#
                ),
            )
            .unwrap();
            assert!(read_baseline().is_ok(), "baseline should parse");
            // The "model" edits a protected test file and a normal source file.
            fs::write("tests/t.rs", "#[test]\nfn t() { assert_eq!(1, 2); }\n").unwrap();
            fs::write("src/lib.rs", "pub fn ok() -> u32 { 2 }\n").unwrap();

            let out = evaluate().expect("verify must not crash");
            assert!(out.blocked, "protected edit must yield BLOCKED");
            assert_eq!(out.score, 0);
            assert_eq!(out.checks.scope, 0);
            assert!(
                out.failures.iter().any(|f| f.contains("tests/t.rs")),
                "failures must name the violating path: {:?}",
                out.failures
            );
            // Anti-cheat revert: the working tree is back to HEAD.
            assert_eq!(
                fs::read_to_string("src/lib.rs").unwrap(),
                "pub fn ok() -> u32 { 1 }\n",
                "legit patch must also be reverted on BLOCKED"
            );
            assert_eq!(
                fs::read_to_string("tests/t.rs").unwrap(),
                "#[test]\nfn t() { assert_eq!(1, 1); }\n",
                "protected file restored to HEAD"
            );
        });
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn legit_patch_keeps_scope_passing() {
        let root = git_fixture("scope-ok");
        with_cwd(&root, || {
            let head = git(&["rev-parse", "HEAD"]).trim().to_string();
            fs::create_dir_all(".quickloop").unwrap();
            fs::write(
                ".quickloop/baseline.json",
                format!(
                    r#"{{"pr":42,"base_sha":"{head}","head_sha":"{head}","test_file_count":1,"changed_paths":["src/lib.rs"]}}"#
                ),
            )
            .unwrap();
            fs::write("src/lib.rs", "pub fn ok() -> u32 { 2 }\n").unwrap();
            let out = evaluate().expect("verify must not crash");
            assert!(!out.blocked);
            assert_eq!(out.checks.scope, 1);
        });
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_baseline_is_an_execution_error() {
        let root = git_fixture("no-baseline");
        with_cwd(&root, || {
            let err = evaluate().expect_err("missing baseline must fail execution");
            assert!(err.contains("run `quickloop diff <pr>` first"));
        });
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn changed_since_baseline_includes_working_tree_and_status() {
        let root = git_fixture("changed");
        with_cwd(&root, || {
            let head = git(&["rev-parse", "HEAD"]).trim().to_string();
            fs::write("src/lib.rs", "pub fn ok() -> u32 { 2 }\n").unwrap();
            fs::write("untracked_new.rs", "new\n").unwrap();
            let changed = changed_since_baseline(&head).unwrap();
            assert!(changed.contains(&"src/lib.rs".to_string()));
            assert!(
                changed.contains(&"untracked_new.rs".to_string()),
                "porcelain ?? entries count as changes: {changed:?}"
            );
            assert!(
                !changed.contains(&"tests/t.rs".to_string()),
                "unmodified protected file must not appear as changed"
            );
        });
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn status_z_fields_handle_renames_and_skips_original_paths() {
        assert_eq!(
            status_changed_fields("M  src/lib.rs\0?? new.rs\0"),
            vec!["src/lib.rs", "new.rs"]
        );
        assert_eq!(
            status_changed_fields("R  new/name.rs\0old/name.rs\0M  a.rs\0"),
            vec!["new/name.rs", "a.rs"],
            "rename destination kept, original path skipped"
        );
        assert_eq!(status_changed_fields(""), Vec::<String>::new());
        assert_eq!(status_changed_fields("\0\0"), Vec::<String>::new());
    }

    #[test]
    fn changed_paths_survive_special_characters_via_z_output() {
        let root = git_fixture("special-names");
        with_cwd(&root, || {
            let head = git(&["rev-parse", "HEAD"]).trim().to_string();
            // git would C-quote this path in line mode; -z keeps it verbatim.
            fs::create_dir_all("src").unwrap();
            fs::write("src/café.rs", "pub fn x() {}\n").unwrap();
            let changed = changed_since_baseline(&head).unwrap();
            assert!(
                changed.contains(&"src/café.rs".to_string()),
                "non-ASCII path must appear unquoted: {changed:?}"
            );
        });
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn blocked_revert_removes_injected_untracked_files() {
        let root = git_fixture("untracked-blocked");
        with_cwd(&root, || {
            let head = git(&["rev-parse", "HEAD"]).trim().to_string();
            fs::create_dir_all(".quickloop").unwrap();
            fs::write(
                ".quickloop/baseline.json",
                format!(
                    r#"{{"pr":42,"base_sha":"{head}","head_sha":"{head}","test_file_count":1,"changed_paths":["src/lib.rs"]}}"#
                ),
            )
            .unwrap();
            // Inject an untracked protected test file and a scratch file.
            fs::write("tests/injected_test.rs", "#[test]\nfn injected() {}\n").unwrap();
            fs::write("scratch.txt", "tmp\n").unwrap();
            let out = evaluate().expect("verify must not crash");
            assert!(out.blocked, "count drift must yield BLOCKED");
            assert!(
                !Path::new("tests/injected_test.rs").exists(),
                "injected untracked protected file must be removed"
            );
            assert!(
                !Path::new("scratch.txt").exists(),
                "untracked scratch file is removed by the revert"
            );
            assert!(
                Path::new(".quickloop/baseline.json").exists(),
                ".quickloop survives"
            );
        });
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sec_patterns_flag_backdoor_but_not_stored_password_consts() {
        assert!(SEC_PATTERNS.contains(&"backdoor"));
        let backdoor_line = "const BACKDOOR_PASSWORD: &str = \"open-sesame-42\";".to_lowercase();
        let stored_line = "const STORED_PASSWORD: &str = \"correct-horse-battery\";".to_lowercase();
        assert!(
            SEC_PATTERNS.iter().any(|p| backdoor_line.contains(p)),
            "planted backdoor must be flagged"
        );
        assert!(
            !SEC_PATTERNS.iter().any(|p| stored_line.contains(p)),
            "legitimate stored-password const must not be flagged"
        );
    }
}
