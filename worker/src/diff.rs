//! PR diff extraction and noise filtering (`quickloop diff <pr>`).
//!
//! Fetches the PR diff via `gh`, strips noise (lockfiles, `dist/`,
//! `vendor/`, generated paths, binaries), and emits compact single-line
//! JSON. It also persists the loop baseline to `.quickloop/baseline.json`,
//! which `quickloop verify` later consumes for scope anti-cheat and the
//! security scan. Target latency < 400 ms (network-bound on `gh`).

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::gh;
use crate::verify::is_protected_path;

/// File names always stripped from the diff before it reaches the model.
const EXCLUDED_FILES: [&str; 6] = [
    "Cargo.lock",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "poetry.lock",
    "composer.lock",
];

/// Path segments (directory names) always stripped from the diff.
const EXCLUDED_SEGMENTS: [&str; 3] = ["dist", "vendor", "generated"];

/// Directories never walked when counting protected test files.
pub(crate) const SKIP_DIRS: [&str; 11] = [
    ".git",
    "target",
    "node_modules",
    ".quickloop",
    "dist",
    "vendor",
    "build",
    "generated",
    ".venv",
    "venv",
    "__pycache__",
];

#[derive(Debug, Serialize)]
pub struct Hunk {
    pub old_start: u32,
    pub new_start: u32,
    pub patch: String,
}

#[derive(Debug, Serialize)]
pub struct DiffFile {
    pub path: String,
    pub hunks: Vec<Hunk>,
}

#[derive(Debug, Serialize)]
pub struct DiffOutput {
    pub pr: u64,
    pub base_sha: String,
    pub head_sha: String,
    pub test_file_count: usize,
    pub files: Vec<DiffFile>,
}

/// Loop baseline persisted by `quickloop diff` and consumed by
/// `quickloop verify` for scope anti-cheat and the security scan.
#[derive(Debug, Serialize, Deserialize)]
pub struct Baseline {
    pub pr: u64,
    pub base_sha: String,
    pub head_sha: String,
    pub test_file_count: usize,
    pub changed_paths: Vec<String>,
}

struct PendingFile {
    path: String,
    hunks: Vec<Hunk>,
    current: Option<Hunk>,
}

/// Validate a PR reference (42, #42, or a GitHub PR URL) before it reaches
/// `gh`, so a leading `-` can never be interpreted as a gh flag.
fn validate_pr_ref(pr_ref: &str) -> Result<(), String> {
    let digits = pr_ref.strip_prefix('#').unwrap_or(pr_ref);
    let numeric = !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit());
    let github_pr_url = pr_ref.starts_with("https://")
        && pr_ref.contains("github.com/")
        && pr_ref.contains("/pull/");
    if numeric || github_pr_url {
        return Ok(());
    }
    Err(format!(
        "invalid PR reference '{pr_ref}' (expected 42, #42, or a GitHub PR URL)"
    ))
}

/// Execute `quickloop diff <pr>`.
pub fn run(pr_ref: &str) -> Result<(), String> {
    validate_pr_ref(pr_ref)?;
    let meta = gh::run(&[
        "pr",
        "view",
        pr_ref,
        "--json",
        "number,baseRefOid,headRefOid",
    ])?;
    let meta: serde_json::Value = serde_json::from_str(meta.trim())
        .map_err(|e| format!("gh pr view returned invalid JSON: {e}"))?;
    let pr = meta["number"]
        .as_u64()
        .ok_or("gh pr view: missing field 'number'")?;
    let base_sha = meta["baseRefOid"]
        .as_str()
        .ok_or("gh pr view: missing field 'baseRefOid'")?
        .to_string();
    let head_sha = meta["headRefOid"]
        .as_str()
        .ok_or("gh pr view: missing field 'headRefOid'")?
        .to_string();

    let raw = gh::run(&["pr", "diff", pr_ref])?;
    let files = parse_and_filter(&raw);
    let changed_paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
    let test_file_count = count_protected_files(Path::new("."))?;

    let baseline = Baseline {
        pr,
        base_sha,
        head_sha,
        test_file_count,
        changed_paths,
    };
    // Persist the baseline before anything reaches stdout: stdout carries
    // loop data only, so a failed run must not leave valid JSON behind.
    write_baseline(&baseline)?;

    let output = DiffOutput {
        pr: baseline.pr,
        base_sha: baseline.base_sha.clone(),
        head_sha: baseline.head_sha.clone(),
        test_file_count: baseline.test_file_count,
        files,
    };
    println!(
        "{}",
        serde_json::to_string(&output).map_err(|e| format!("serialize diff: {e}"))?
    );
    Ok(())
}

/// Parse a unified diff and keep only noise-free hunks. Binary sections,
/// mode-only changes, and excluded paths carry no hunks and are dropped.
/// Lines are split on `\n` only (never `.lines()`), so `\r` from CRLF files
/// survives into the emitted `patch` text — the model transcribes SEARCH
/// blocks from this JSON and `quickloop patch` matches byte-for-byte, so
/// any normalization here would break the patch contract upstream.
/// Context size relies on `gh`/git emitting the default 3 context lines.
pub fn parse_and_filter(raw: &str) -> Vec<DiffFile> {
    let mut files: Vec<DiffFile> = Vec::new();
    let mut pending: Option<PendingFile> = None;

    let lines: Vec<&str> = raw.split('\n').collect();
    let end = if lines.last().is_some_and(|l| l.is_empty()) {
        lines.len() - 1
    } else {
        lines.len()
    };
    for line in &lines[..end] {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            flush_pending(&mut pending, &mut files);
            pending = Some(PendingFile {
                path: extract_b_path(rest),
                hunks: Vec::new(),
                current: None,
            });
            continue;
        }
        if let Some(rest) = line.strip_prefix("+++ b/") {
            // The `+++` header is the authoritative destination path (it
            // handles quoted and space-containing names). Only trust it
            // outside hunk bodies: a pure addition line whose content is
            // `++ b/x` renders as `+++ b/x` inside a hunk.
            if let Some(p) = pending.as_mut() {
                if p.current.is_none() {
                    let candidate = rest
                        .split('\t')
                        .next()
                        .unwrap_or("")
                        .trim_matches('"')
                        .trim_end_matches('\r')
                        .to_string();
                    if !candidate.is_empty() {
                        p.path = candidate;
                        continue;
                    }
                }
            }
        }
        if line.starts_with("@@ ") {
            if let Some(p) = pending.as_mut() {
                if let Some(current) = p.current.take() {
                    p.hunks.push(current);
                }
                let (old_start, new_start) = hunk_starts(line);
                p.current = Some(Hunk {
                    old_start,
                    new_start,
                    patch: line.to_string(),
                });
            }
            continue;
        }
        if line.starts_with("\\ No newline at end of file") {
            continue;
        }
        if let Some(p) = pending.as_mut() {
            if let Some(current) = p.current.as_mut() {
                current.patch.push('\n');
                current.patch.push_str(line);
            }
        }
    }
    flush_pending(&mut pending, &mut files);
    files
}

fn flush_pending(pending: &mut Option<PendingFile>, files: &mut Vec<DiffFile>) {
    if let Some(mut p) = pending.take() {
        if let Some(current) = p.current.take() {
            p.hunks.push(current);
        }
        if !p.hunks.is_empty() && !is_excluded(&p.path) {
            files.push(DiffFile {
                path: p.path,
                hunks: p.hunks,
            });
        }
    }
}

/// Extract the destination path from a `diff --git a/<old> b/<new>` tail.
/// A trailing `\r` (CRLF files) is line metadata, not path content, so it
/// is stripped here — hunk text keeps its `\r` untouched.
fn extract_b_path(rest: &str) -> String {
    let strip = |s: &str| s.trim_matches('"').trim_end_matches('\r').to_string();
    if let Some(idx) = rest.find(" b/") {
        strip(&rest[idx + 3..])
    } else {
        let candidate = rest.rsplit(' ').next().unwrap_or("");
        strip(candidate.strip_prefix("a/").unwrap_or(candidate))
    }
}

/// Parse `@@ -a,b +c,d @@` starts. Only the range tokens directly after the
/// leading `@@` are considered; header context text (e.g. `->`) must never
/// influence the result. New-file hunks (`-0,0`) report old_start 0.
fn hunk_starts(line: &str) -> (u32, u32) {
    let mut old_start = 1;
    let mut new_start = 1;
    let mut ranges = line
        .split_whitespace()
        .skip_while(|token| *token != "@@")
        .skip(1)
        .take(2);
    if let Some(token) = ranges.next() {
        if let Some(num) = token.strip_prefix('-') {
            old_start = num.split(',').next().unwrap_or("1").parse().unwrap_or(1);
        }
    }
    if let Some(token) = ranges.next() {
        if let Some(num) = token.strip_prefix('+') {
            new_start = num.split(',').next().unwrap_or("1").parse().unwrap_or(1);
        }
    }
    (old_start, new_start)
}

/// Decide whether a diff path is noise that must never reach the model.
pub fn is_excluded(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    EXCLUDED_FILES.contains(&name)
        || path.ends_with(".lock")
        || EXCLUDED_SEGMENTS
            .iter()
            .any(|seg| path.split('/').any(|part| part == *seg))
}

/// Count repository files matching the protected test-path globs, skipping
/// build/vendor trees. Used as the anti-cheat baseline.
pub fn count_protected_files(root: &Path) -> Result<usize, String> {
    let mut count = 0usize;
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("dir entry in {}: {e}", dir.display()))?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            // file_type() never follows symlinks, so symlinked directories
            // cannot trap the walk in a cycle.
            let file_type = entry
                .file_type()
                .map_err(|e| format!("file_type in {}: {e}", dir.display()))?;
            if file_type.is_dir() {
                if !SKIP_DIRS.contains(&name.as_str()) {
                    stack.push(path);
                }
            } else {
                let rel = path
                    .strip_prefix(root)
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .replace('\\', "/");
                if is_protected_path(&rel) {
                    count += 1;
                }
            }
        }
    }
    Ok(count)
}

/// Persist the loop baseline for `quickloop verify`.
fn write_baseline(baseline: &Baseline) -> Result<(), String> {
    let dir = Path::new(".quickloop");
    fs::create_dir_all(dir).map_err(|e| format!("create .quickloop: {e}"))?;
    let data = serde_json::to_string(baseline).map_err(|e| e.to_string())?;
    fs::write(dir.join("baseline.json"), data).map_err(|e| format!("write baseline.json: {e}"))
}

#[cfg(test)]
mod tests {
    use super::{extract_b_path, hunk_starts, is_excluded, parse_and_filter, validate_pr_ref};
    use std::fs;
    use std::path::PathBuf;

    const FIXTURE: &str = "\
diff --git a/Cargo.lock b/Cargo.lock
index 1111111..2222222 100644
--- a/Cargo.lock
+++ b/Cargo.lock
@@ -1,5 +1,6 @@
[[package]]
-name = \"old\"
+name = \"new\"

diff --git a/dist/bundle.js b/dist/bundle.js
new file mode 100644
--- /dev/null
+++ b/dist/bundle.js
@@ -0,0 +1,2 @@
+bundle

diff --git a/demo-repo/src/scoring.rs b/demo-repo/src/scoring.rs
index aaaaaaa..bbbbbbb 100644
--- a/demo-repo/src/scoring.rs
+++ b/demo-repo/src/scoring.rs
@@ -8,7 +8,7 @@ fn decay_divisor() -> u32 {
    let context = kept();
-    let old = line();
+    let new = line();
    let tail = kept();

diff --git a/assets/logo.png b/assets/logo.png
index 0000000..1111111 100644
Binary files a/assets/logo.png and b/assets/logo.png differ
";

    #[test]
    fn keeps_only_noise_free_files_with_hunks() {
        let files = parse_and_filter(FIXTURE);
        assert_eq!(files.len(), 1, "lockfile, dist/, and binaries are dropped");
        let f = &files[0];
        assert_eq!(f.path, "demo-repo/src/scoring.rs");
        assert_eq!(f.hunks.len(), 1);
        assert_eq!(f.hunks[0].old_start, 8);
        assert_eq!(f.hunks[0].new_start, 8);
        assert!(f.hunks[0].patch.contains("-    let old = line();"));
        assert!(f.hunks[0].patch.contains("+    let new = line();"));
        assert!(
            f.hunks[0].patch.contains("    let context = kept();"),
            "context lines are preserved verbatim"
        );
    }

    #[test]
    fn excluded_paths_cover_lockfiles_and_noise_dirs() {
        assert!(is_excluded("worker/Cargo.lock"));
        assert!(is_excluded("web/pnpm-lock.yaml"));
        assert!(is_excluded("app/dist/bundle.js"));
        assert!(is_excluded("third_party/vendor/lib.rs"));
        assert!(is_excluded("src/generated/api.ts"));
        assert!(!is_excluded("worker/src/diff.rs"));
        assert!(!is_excluded("demo-repo/src/scoring.rs"));
    }

    #[test]
    fn hunk_starts_handle_single_line_and_new_files() {
        assert_eq!(hunk_starts("@@ -18 +18,2 @@ ctx"), (18, 18));
        assert_eq!(hunk_starts("@@ -0,0 +1,5 @@ new file"), (0, 1));
        assert_eq!(hunk_starts("garbage"), (1, 1));
        assert_eq!(
            hunk_starts("@@ -8,7 +8,7 @@ fn decay_divisor() -> u32 {"),
            (8, 8),
            "header context text must not affect parsing"
        );
    }

    #[test]
    fn extract_b_path_prefers_destination_side() {
        assert_eq!(extract_b_path("a/old.txt b/new.txt"), "new.txt");
        assert_eq!(extract_b_path("a/only.txt"), "only.txt");
    }

    #[test]
    fn crlf_hunks_preserve_carriage_returns() {
        let fixture = "diff --git a/win.ps1 b/win.ps1\r\nindex aaaaaaa..bbbbbbb 100644\r\n--- a/win.ps1\r\n+++ b/win.ps1\r\n@@ -1,2 +1,2 @@\r\n-old\r\n+new\r\n";
        let files = parse_and_filter(fixture);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "win.ps1");
        assert!(
            files[0].hunks[0].patch.contains("-old\r"),
            "CR must survive into the patch text for byte-exact SEARCH matching"
        );
        assert!(files[0].hunks[0].patch.contains("+new\r"));
    }

    #[test]
    fn plus_plus_plus_header_overrides_path_for_quoted_names() {
        let fixture = "diff --git a/\"weird name.txt\" b/\"weird name.txt\"\nsimilarity index 100%\n--- a/\"weird name.txt\"\n+++ b/\"weird name.txt\"\n@@ -1 +1 @@\n-x\n+y\n";
        let files = parse_and_filter(fixture);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "weird name.txt");
    }

    #[test]
    fn pr_refs_accept_numbers_hash_and_url_only() {
        assert!(validate_pr_ref("42").is_ok());
        assert!(validate_pr_ref("#42").is_ok());
        assert!(
            validate_pr_ref("https://github.com/maziazi/quickloop-patch-review/pull/1").is_ok()
        );
        assert!(validate_pr_ref("").is_err());
        assert!(validate_pr_ref("abc").is_err());
        assert!(validate_pr_ref("42x").is_err());
        assert!(
            validate_pr_ref("-R owner/repo").is_err(),
            "gh flag injection must be rejected"
        );
        assert!(validate_pr_ref("https://example.com/pull/1").is_err());
    }

    #[test]
    fn count_protected_files_walks_tests_and_skips_target() {
        let root = temp_root("protected-count");
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("target/debug")).unwrap();
        fs::write(root.join("tests/regression.rs"), "").unwrap();
        fs::write(root.join("src/lib.rs"), "").unwrap();
        fs::write(root.join("target/debug/lookup_test.rs"), "").unwrap();
        let count = super::count_protected_files(&root).unwrap();
        assert_eq!(
            count, 1,
            "only tests/regression.rs counts; target/ is skipped"
        );
        let _ = fs::remove_dir_all(root);
    }

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("quickloop-diff-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
