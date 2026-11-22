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

/// Cargo argument scope for the gate: `-p <pkg>` per workspace member,
/// used **only when every changed path maps into some member directory**
/// (completeness-driven). Member directories come from the root
/// `Cargo.toml`; package names from each member's own `[package] name`.
///
/// This direction matters: any lossiness in the mapping — root-package
/// worktrees (root files have no member dir), glob members, truncated
/// trailing comments, unparsable names, root-level files like `README.md`
/// or `Cargo.lock` — leaves a changed path unmapped and forces the
/// `--workspace` fallback. Scoping is an optimization, never a
/// completeness risk.
fn cargo_scope_args(changed: &[String]) -> Vec<String> {
    let Ok(root_manifest) = fs::read_to_string("Cargo.toml") else {
        return vec!["--workspace".to_string()];
    };
    let Some(members) = workspace_members(&root_manifest) else {
        return vec!["--workspace".to_string()];
    };
    if members.is_empty() {
        return vec!["--workspace".to_string()];
    }
    // Glob members (`crates/*`) expand beyond their literal directory text:
    // a file under a glob-expanded sibling package would prefix-match the
    // glob's root member (e.g. `a/b/src` matches member `a`), silently
    // scoping the gate away from the real crate. Unparsable → fallback.
    if members
        .iter()
        .any(|m| m.contains('*') || m.contains('?') || m.contains('['))
    {
        return vec!["--workspace".to_string()];
    }
    let mut packages = Vec::new();
    // Deterministic order: walk members as listed in the manifest.
    for member in &members {
        if !changed
            .iter()
            .any(|path| path.starts_with(format!("{member}/").as_str()))
        {
            continue;
        }
        let Some(name) = package_name(Path::new(member)) else {
            return vec!["--workspace".to_string()];
        };
        if !packages.contains(&name) {
            packages.push(name);
        }
    }
    // Completeness rule: every changed path must have mapped into some
    // member — any unmapped path (root package files, glob members,
    // root-level files like README.md) forces the whole workspace so a
    // regression can never escape a partial scope.
    let all_mapped = changed.iter().all(|path| {
        members
            .iter()
            .any(|m| path.starts_with(format!("{m}/").as_str()))
    });
    if !all_mapped || packages.is_empty() {
        return vec!["--workspace".to_string()];
    }
    let mut args = Vec::with_capacity(packages.len() * 2);
    for name in packages {
        args.push("-p".to_string());
        args.push(name);
    }
    args
}

/// `members = [...]` entries from a workspace `Cargo.toml` — a tiny
/// targeted scan rather than a toml dependency (see §5.3 dependency diet).
///
/// Strict on purpose: entries must be clean double- or single-quoted
/// strings, and any leftover text after a closing quote (beyond `,`, `]`,
/// and whitespace — trailing comments are stripped per line first) aborts
/// the whole parse with `None`. A lossy parse here would let a real member
/// escape a partial `-p` scope, so valid-but-unrepresentable TOML is
/// treated as unparsable, never skipped.
fn workspace_members(manifest: &str) -> Option<Vec<String>> {
    let mut members = Vec::new();
    let mut section = String::new();
    let mut opened = false;
    let mut closed = false;
    for raw in manifest.lines() {
        let line = match raw.split_once('#') {
            Some((code, _)) => code.trim(),
            None => raw.trim(),
        };
        if line.is_empty() {
            continue;
        }
        if !opened {
            if line.starts_with('[') && line.ends_with(']') {
                section = line[1..line.len() - 1].trim().to_string();
            } else if section == "workspace" {
                if let Some(inner) = line.strip_prefix("members").and_then(|rest| {
                    rest.trim_start()
                        .strip_prefix('=')
                        .and_then(|s| s.trim_start().strip_prefix('['))
                }) {
                    opened = true;
                    // A multi-line array legitimately stays open here; the
                    // final `opened && !closed` check covers unterminated
                    // arrays. Only garbage aborts immediately.
                    let (closed_now, ok) = scan_items(inner, &mut members)?;
                    if !ok {
                        return None;
                    }
                    if closed_now {
                        closed = true;
                    }
                }
            }
            continue;
        }
        // Inside the array: this line must either close it or add clean
        // items — a line containing a section header mid-array is garbage.
        let (closed_now, ok) = scan_items(line, &mut members)?;
        if !ok {
            return None;
        }
        if closed_now {
            closed = true;
            break;
        }
    }
    if opened && !closed {
        return None;
    }
    Some(members)
}

/// Scan array items from the text following the opening `[` (comments
/// already stripped). Returns `(closed, ok)` where `closed` means a `]`
/// terminated the array in this text (only whitespace may follow) and `ok`
/// means every item was a clean double- or single-quoted string.
fn scan_items(text: &str, members: &mut Vec<String>) -> Option<(bool, bool)> {
    let mut quote: Option<char> = None;
    let mut current = String::new();
    for ch in text.chars() {
        match quote {
            Some(q) if ch == q => {
                if current.is_empty() {
                    return None;
                }
                members.push(std::mem::take(&mut current));
                quote = None;
            }
            Some(_q) => current.push(ch),
            None => match ch {
                '"' | '\'' => quote = Some(ch),
                ',' | ' ' | '\t' => {}
                ']' => return Some((true, true)),
                _ => return None,
            },
        }
    }
    if quote.is_some() {
        // Unterminated string reaching end-of-line — not a clean entry.
        return None;
    }
    Some((false, true))
}

/// The `[package] name` of a member crate: the first `name = "..."` before
/// any dependency section starts (package metadata always precedes deps).
/// Strict: the value must be a clean quoted string (`split('"')` takes the
/// text between the first two quotes, so a trailing `# comment` parses
/// cleanly; exotic lines yield `None` → the caller falls back to
/// `--workspace` instead of passing a garbage `-p` that would fail the
/// gate forever).
fn package_name(member_dir: &Path) -> Option<String> {
    let manifest = fs::read_to_string(member_dir.join("Cargo.toml")).ok()?;
    let mut section = String::new();
    for raw in manifest.lines() {
        // Strip trailing comments first so `[package] # note` headers and
        // `name = "x" # why` values parse the same as their plain forms.
        let line = match raw.split_once('#') {
            Some((code, _)) => code.trim(),
            None => raw.trim(),
        };
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_string();
            continue;
        }
        if section != "package" {
            // Only `[package] name` is the crate's identity — a `[lib] name`
            // or dependency key is a different thing entirely.
            continue;
        }
        let Some(rest) = line
            .strip_prefix("name = ")
            .or_else(|| line.strip_prefix("name="))
        else {
            continue;
        };
        let value = rest.split('"').nth(1).filter(|v| !v.is_empty())?;
        return Some(value.to_string());
    }
    None
}

/// Run test/lint/security checks concurrently (cargo serializes itself on
/// the target-dir lock, the security scan is pure I/O).
///
/// Cargo scope: only workspace members whose directories intersect the
/// changed paths are re-checked. Sound because an unchanged member cannot
/// produce new lint or test failures; when nothing maps to a member (e.g.
/// a root-level file or a standalone cargo project without a members
/// list), fall back to the whole workspace.
fn run_checks_concurrent(
    changed: &[String],
) -> Result<(CheckResult, CheckResult, CheckResult), String> {
    let changed_owned = changed.to_vec();
    let cargo_scope = cargo_scope_args(&changed_owned);
    thread::scope(
        |s| -> Result<(CheckResult, CheckResult, CheckResult), String> {
            let test_handle = s.spawn(|| run_test_check(&cargo_scope));
            let lint_handle = s.spawn(|| run_lint_check(&cargo_scope));
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

fn run_test_check(scope: &[String]) -> Result<CheckResult, String> {
    let mut args = vec!["test"];
    args.extend(scope.iter().map(String::as_str));
    let output = run_tool("cargo", &args, TOOL_TIMEOUT_SECS)?;
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

fn run_lint_check(scope: &[String]) -> Result<CheckResult, String> {
    let mut args = vec!["clippy"];
    args.extend(scope.iter().map(String::as_str));
    args.push("--");
    args.push("-D");
    args.push("warnings");
    let output = run_tool("cargo", &args, TOOL_TIMEOUT_SECS)?;
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
        cargo_scope_args, changed_since_baseline, evaluate, read_baseline, score_of,
        status_changed_fields, workspace_members, SEC_PATTERNS,
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
    fn cargo_scope_maps_changed_paths_to_member_packages() {
        let manifest = "# comment\n[workspace]\nresolver = \"2\"\nmembers = [\n  \"worker\",\n  \"demo-repo\",\n]\ndefault-members = [\"worker\"]\n";
        assert_eq!(
            workspace_members(manifest),
            Some(vec!["worker".to_string(), "demo-repo".to_string()])
        );
        // Multi-item single line, spacing variants.
        assert_eq!(
            workspace_members("[workspace]\nmembers = [\"a\", \"b\"]"),
            Some(vec!["a".to_string(), "b".to_string()])
        );
        // No members list (standalone project) → Some(empty), not a parse
        // failure — the caller's empty-scope rule handles it.
        assert_eq!(
            workspace_members("[package]\nname = \"x\"\n"),
            Some(Vec::new())
        );
        // Single-quoted literal strings are valid TOML and must parse.
        assert_eq!(
            workspace_members("[workspace]\nmembers = [\"a\", 'b']"),
            Some(vec!["a".to_string(), "b".to_string()])
        );
        // Unclosed array → None (caller falls back to --workspace).
        assert_eq!(workspace_members("[workspace]\nmembers = [\"a\"\n"), None);
    }

    #[test]
    fn cargo_scope_args_fall_back_to_workspace_safely() {
        let root =
            std::env::temp_dir().join(format!("quickloop-scope-empty-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        with_cwd(&root, || {
            // No readable root manifest → whole workspace, never an empty scope.
            assert_eq!(cargo_scope_args(&["src/x.rs".to_string()]), ["--workspace"]);
        });
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cargo_scope_scopes_to_intersecting_members_only() {
        let root = std::env::temp_dir().join(format!("quickloop-scope-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("worker/src")).unwrap();
        fs::create_dir_all(root.join("demo-repo/src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"worker\", \"demo-repo\"]\n",
        )
        .unwrap();
        fs::write(
            root.join("worker/Cargo.toml"),
            "[package]\nname = \"quickloop\"\n[dependencies]\nserde = \"1\"\n",
        )
        .unwrap();
        fs::write(
            root.join("demo-repo/Cargo.toml"),
            "[package]\nname = \"demo_repo\"\n",
        )
        .unwrap();
        with_cwd(&root, || {
            assert_eq!(
                cargo_scope_args(&["demo-repo/src/auth.rs".to_string()]),
                ["-p", "demo_repo"]
            );
            assert_eq!(
                cargo_scope_args(&[
                    "demo-repo/src/auth.rs".to_string(),
                    "worker/src/main.rs".to_string()
                ]),
                ["-p", "quickloop", "-p", "demo_repo"]
            );
            // Completeness rule: ANY changed path that maps to no member
            // (root-level files, unmapped crates) forces the whole workspace —
            // a regression in an unmapped file must never escape the gate.
            assert_eq!(
                cargo_scope_args(&["demo-repo/src/auth.rs".to_string(), "README.md".to_string()]),
                ["--workspace"]
            );
        });
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cargo_scope_lossy_manifests_parse_or_fall_back() {
        let root =
            std::env::temp_dir().join(format!("quickloop-scope-lossy-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("worker/src")).unwrap();
        fs::create_dir_all(root.join("crates/extra/src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            // Trailing inline comment on the members line — comments are
            // stripped per line before scanning, so this parses cleanly.
            "[workspace]\nmembers = [\"worker\", \"crates/extra\", \"demo-repo\"] # note\n",
        )
        .unwrap();
        fs::write(
            root.join("worker/Cargo.toml"),
            // Trailing comment after the quoted name — split('"') takes the
            // text between the quotes, so this parses cleanly too.
            "[package]\nname = \"quickloop\" # the cli\n",
        )
        .unwrap();
        fs::write(
            root.join("crates/extra/Cargo.toml"),
            "[package]\nname=\"extra_no_space\"\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("demo-repo/src")).unwrap();
        fs::write(
            root.join("demo-repo/Cargo.toml"),
            "[package]\nname = \"demo_repo\"\n",
        )
        .unwrap();
        with_cwd(&root, || {
            assert_eq!(
                cargo_scope_args(&[
                    "worker/src/main.rs".to_string(),
                    "demo-repo/src/auth.rs".to_string()
                ]),
                ["-p", "quickloop", "-p", "demo_repo"]
            );
            assert_eq!(
                cargo_scope_args(&["worker/src/main.rs".to_string()]),
                ["-p", "quickloop"]
            );
            assert_eq!(
                cargo_scope_args(&["crates/extra/src/lib.rs".to_string()]),
                ["-p", "extra_no_space"]
            );
        });
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cargo_scope_single_quoted_and_commented_members_parse_fully() {
        // Round-3 review escape shape: `push_member` silently dropped
        // single-quoted entries and corrupted the last entry when a trailing
        // comment followed the closing bracket — in both cases the nested
        // member `a/b` vanished from the parsed list and its regression
        // escaped a scope pinned to `a`. The strict parser accepts both
        // forms exactly, so `bee` is now inside the scope instead of
        // escaping it.
        let root =
            std::env::temp_dir().join(format!("quickloop-scope-nested-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("a/src")).unwrap();
        fs::create_dir_all(root.join("a/b/src")).unwrap();
        fs::write(root.join("a/Cargo.toml"), "[package]\nname = \"apa\"\n").unwrap();
        fs::write(root.join("a/b/Cargo.toml"), "[package]\nname = \"bee\"\n").unwrap();
        for (tag, manifest) in [
            (
                "double-quoted + trailing comment",
                "[workspace]\nmembers = [\"a\", \"a/b\"] # note\n",
            ),
            (
                "single-quoted literal string",
                "[workspace]\nmembers = [\"a\", 'a/b']\n",
            ),
        ] {
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("a/b/src")).unwrap();
            fs::write(root.join("Cargo.toml"), manifest).unwrap();
            fs::write(root.join("a/Cargo.toml"), "[package]\nname = \"apa\"\n").unwrap();
            fs::write(root.join("a/b/Cargo.toml"), "[package]\nname = \"bee\"\n").unwrap();
            with_cwd(&root, || {
                // Member `a` also prefix-matches (superset is safe); what the
                // fix guarantees is that `bee` is INSIDE the scope, not
                // silently escaped from it.
                assert_eq!(
                    cargo_scope_args(&["a/b/src/lib.rs".to_string()]),
                    ["-p", "apa", "-p", "bee"],
                    "{tag}: the nested member must be in scope, not escaped"
                );
            });
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cargo_scope_garbage_entries_abort_the_scope() {
        // The one direction the strict parser must keep fail-closed: an
        // entry it cannot represent exactly (bare word, unclosed quote)
        // aborts the whole parse → whole workspace, never a partial scope.
        let root =
            std::env::temp_dir().join(format!("quickloop-scope-garbage-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("a/src")).unwrap();
        fs::create_dir_all(root.join("a/b/src")).unwrap();
        fs::write(root.join("a/Cargo.toml"), "[package]\nname = \"apa\"\n").unwrap();
        fs::write(root.join("a/b/Cargo.toml"), "[package]\nname = \"bee\"\n").unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"a\", bare-word]\n",
        )
        .unwrap();
        with_cwd(&root, || {
            assert_eq!(
                cargo_scope_args(&["a/b/src/lib.rs".to_string()]),
                ["--workspace"]
            );
        });
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cargo_scope_root_package_files_fall_back_to_workspace() {
        // Root-package workspace shape: members list only names `crates/sub`,
        // while the root package owns `src/` — a root regression must not
        // escape a `crates/sub`-only scope.
        let root =
            std::env::temp_dir().join(format!("quickloop-scope-rootpkg-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("crates/sub/src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"root-pkg\"\n\n[workspace]\nmembers = [\"crates/sub\"]\n",
        )
        .unwrap();
        fs::write(
            root.join("crates/sub/Cargo.toml"),
            "[package]\nname = \"sub\"\n",
        )
        .unwrap();
        with_cwd(&root, || {
            assert_eq!(
                cargo_scope_args(&["src/lib.rs".to_string()]),
                ["--workspace"]
            );
            assert_eq!(
                cargo_scope_args(&[
                    "src/lib.rs".to_string(),
                    "crates/sub/src/lib.rs".to_string()
                ]),
                ["--workspace"]
            );
        });
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cargo_scope_glob_member_rooted_at_literal_member_falls_back() {
        // `members = ["a", "a/*"]`: a file under the glob-expanded sibling
        // `a/b` prefix-matches the literal member `a`, so the completeness
        // rule alone would wrongly scope the gate to `a`'s package while a
        // regression in `a/b`'s package escapes. Glob metacharacters must
        // force the `--workspace` fallback outright.
        let root =
            std::env::temp_dir().join(format!("quickloop-scope-glob-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("a/src")).unwrap();
        fs::create_dir_all(root.join("a/b/src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"a\", \"a/*\"]\n",
        )
        .unwrap();
        fs::write(root.join("a/Cargo.toml"), "[package]\nname = \"apa\"\n").unwrap();
        fs::write(root.join("a/b/Cargo.toml"), "[package]\nname = \"bee\"\n").unwrap();
        with_cwd(&root, || {
            assert_eq!(
                cargo_scope_args(&["a/b/src/lib.rs".to_string()]),
                ["--workspace"]
            );
            assert_eq!(
                cargo_scope_args(&["a/src/lib.rs".to_string()]),
                ["--workspace"]
            );
        });
        let _ = fs::remove_dir_all(root);
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
