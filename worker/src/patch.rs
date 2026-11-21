//! Atomic SEARCH/REPLACE application (`quickloop patch <file>`).
//!
//! Every SEARCH block must match the target file byte-for-byte exactly
//! once; zero matches or an ambiguous match rejects the block and aborts
//! the entire patch set atomically (exit code 2, no partial writes).
//! Files are never created or deleted.

use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Debug)]
struct Block {
    path: String,
    search: String,
    replace: String,
}

#[derive(Debug, Serialize)]
pub struct Rejected {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct Outcome {
    pub ok: bool,
    pub applied: Vec<String>,
    pub rejected: Vec<Rejected>,
}

impl Outcome {
    /// `0` when every block applied; `2` when the set was rejected.
    pub fn exit_code(&self) -> i32 {
        if self.ok {
            0
        } else {
            2
        }
    }
}

/// Execute `quickloop patch <file>`.
pub fn run(patch_file: &Path) -> Result<Outcome, String> {
    let text = fs::read_to_string(patch_file)
        .map_err(|e| format!("read {}: {e}", patch_file.display()))?;
    let blocks = parse_blocks(&text)?;
    if blocks.is_empty() {
        return Err(format!(
            "no ```patch blocks found in {} (protocol violation)",
            patch_file.display()
        ));
    }

    // Validate every block against in-memory content before writing anything;
    // the first rejection aborts the whole set atomically.
    let mut contents: BTreeMap<String, String> = BTreeMap::new();
    let mut applied_paths: Vec<String> = Vec::new();
    let mut rejected: Vec<Rejected> = Vec::new();

    for block in &blocks {
        if !rejected.is_empty() {
            break;
        }
        let content = match contents.get(&block.path) {
            Some(current) => current.clone(),
            None => {
                let target = Path::new(&block.path);
                if !target.exists() {
                    rejected.push(Rejected {
                        path: block.path.clone(),
                        reason: "file_not_found".to_string(),
                    });
                    break;
                }
                fs::read_to_string(target).map_err(|e| format!("read {}: {e}", block.path))?
            }
        };
        let matches = content.matches(&block.search).count();
        if matches == 1 {
            let updated = content.replacen(&block.search, &block.replace, 1);
            contents.insert(block.path.clone(), updated);
            if !applied_paths.contains(&block.path) {
                applied_paths.push(block.path.clone());
            }
        } else {
            rejected.push(Rejected {
                path: block.path.clone(),
                reason: if matches == 0 {
                    "search_not_found".to_string()
                } else {
                    format!("ambiguous_match:{matches}")
                },
            });
        }
    }

    let outcome = if rejected.is_empty() {
        for (path, content) in &contents {
            fs::write(path, content).map_err(|e| format!("write {path}: {e}"))?;
        }
        Outcome {
            ok: true,
            applied: applied_paths,
            rejected: Vec::new(),
        }
    } else {
        Outcome {
            ok: false,
            applied: Vec::new(),
            rejected,
        }
    };
    println!(
        "{}",
        serde_json::to_string(&outcome).map_err(|e| format!("serialize patch outcome: {e}"))?
    );
    Ok(outcome)
}

/// Parse fenced ```patch path=<file> blocks with SEARCH/REPLACE markers.
fn parse_blocks(text: &str) -> Result<Vec<Block>, String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut blocks = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        if let Some(rest) = trimmed.strip_prefix("```patch") {
            let path = rest
                .trim()
                .strip_prefix("path=")
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .ok_or_else(|| format!("line {}: patch fence is missing path=<file>", i + 1))?
                .to_string();
            i += 1;
            if i < lines.len() && lines[i].trim_end() == "<<<<<<< SEARCH" {
                i += 1;
            }
            let mut search: Vec<&str> = Vec::new();
            while i < lines.len() && lines[i].trim_end() != "=======" {
                search.push(lines[i]);
                i += 1;
            }
            if i >= lines.len() {
                return Err(format!(
                    "patch block for {path}: unterminated SEARCH section"
                ));
            }
            i += 1; // skip =======
            let mut replace: Vec<&str> = Vec::new();
            while i < lines.len() && lines[i].trim_end() != ">>>>>>> REPLACE" {
                replace.push(lines[i]);
                i += 1;
            }
            if i >= lines.len() {
                return Err(format!(
                    "patch block for {path}: unterminated REPLACE section"
                ));
            }
            i += 1; // skip >>>>>>> REPLACE
            if i < lines.len() && lines[i].trim_start().starts_with("```") {
                i += 1;
            }
            blocks.push(Block {
                path,
                search: search.join("\n"),
                replace: replace.join("\n"),
            });
        } else {
            i += 1;
        }
    }
    Ok(blocks)
}

#[cfg(test)]
mod tests {
    use super::{parse_blocks, run, Outcome};
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn parses_fenced_blocks_with_path_attribute() {
        let text = "prose before is ignored\n```patch path=src/a.rs\n<<<<<<< SEARCH\nold()\n=======\nnew()\n>>>>>>> REPLACE\n```\ntrailing prose\n";
        let blocks = parse_blocks(text).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].path, "src/a.rs");
        assert_eq!(blocks[0].search, "old()");
        assert_eq!(blocks[0].replace, "new()");
    }

    #[test]
    fn missing_path_attribute_is_a_protocol_error() {
        let text = "```patch\n<<<<<<< SEARCH\nx\n=======\ny\n>>>>>>> REPLACE\n```\n";
        let err = parse_blocks(text).expect_err("missing path must fail");
        assert!(err.contains("missing path=<file>"));
    }

    #[test]
    fn unterminated_search_is_a_protocol_error() {
        let text = "```patch path=a.rs\n<<<<<<< SEARCH\nx\n";
        let err = parse_blocks(text).expect_err("unterminated SEARCH must fail");
        assert!(err.contains("unterminated SEARCH"));
    }

    #[test]
    fn applies_single_block_and_reports_exit_code_zero() {
        let root = temp_root("apply");
        let target = root.join("score.rs");
        fs::write(&target, "fn a() {}\nfn b() {}\n").unwrap();
        let patch_file = root.join("fix.patch");
        fs::write(
            &patch_file,
            format!(
                "```patch path={}\n<<<<<<< SEARCH\nfn b() {{}}\n=======\nfn b() {{ /* fixed */ }}\n>>>>>>> REPLACE\n```\n",
                target.display()
            ),
        )
        .unwrap();
        let outcome = run(&patch_file).unwrap();
        assert!(outcome.ok);
        assert_eq!(outcome.exit_code(), 0);
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "fn a() {}\nfn b() { /* fixed */ }\n"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn applies_sequential_blocks_to_the_same_file() {
        let root = temp_root("sequential");
        let target = root.join("score.rs");
        fs::write(&target, "let a = 1;\nlet b = 2;\n").unwrap();
        let patch_file = root.join("fix.patch");
        fs::write(
            &patch_file,
            format!(
                "```patch path={}\n<<<<<<< SEARCH\nlet a = 1;\n=======\nlet a = 10;\n>>>>>>> REPLACE\n```\n```patch path={}\n<<<<<<< SEARCH\nlet b = 2;\n=======\nlet b = 20;\n>>>>>>> REPLACE\n```\n",
                target.display(),
                target.display()
            ),
        )
        .unwrap();
        let outcome = run(&patch_file).unwrap();
        assert!(outcome.ok);
        assert_eq!(outcome.applied.len(), 1, "same file is reported once");
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "let a = 10;\nlet b = 20;\n"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ambiguous_match_aborts_atomically_without_writing() {
        let root = temp_root("ambiguous");
        let target = root.join("dup.rs");
        let original = "dup();\ndup();\n";
        fs::write(&target, original).unwrap();
        let patch_file = root.join("fix.patch");
        fs::write(
            &patch_file,
            format!(
                "```patch path={}\n<<<<<<< SEARCH\ndup();\n=======\nboom();\n>>>>>>> REPLACE\n```\n",
                target.display()
            ),
        )
        .unwrap();
        let outcome = run(&patch_file).unwrap();
        assert!(!outcome.ok);
        assert_eq!(outcome.exit_code(), 2);
        assert_eq!(outcome.applied.len(), 0);
        assert_eq!(outcome.rejected[0].reason, "ambiguous_match:2");
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            original,
            "no partial writes"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_target_file_is_rejected_not_a_crash() {
        let root = temp_root("missing");
        let patch_file = root.join("fix.patch");
        fs::write(
            &patch_file,
            format!(
                "```patch path={}\n<<<<<<< SEARCH\nx\n=======\ny\n>>>>>>> REPLACE\n```\n",
                root.join("nope.rs").display()
            ),
        )
        .unwrap();
        let outcome = run(&patch_file).unwrap();
        assert!(!outcome.ok);
        assert_eq!(outcome.rejected[0].reason, "file_not_found");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn empty_patch_file_is_an_execution_error() {
        let root = temp_root("empty");
        let patch_file = root.join("fix.patch");
        fs::write(&patch_file, "no fences here\n").unwrap();
        let err = run(&patch_file).expect_err("no blocks must be a protocol error");
        assert!(err.contains("no ```patch blocks"));
        let _ = fs::remove_dir_all(root);
    }

    fn temp_root(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("quickloop-patch-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
