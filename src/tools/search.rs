use crate::error::{HarnessError, Result};
use crate::tools::context::ToolContext;
use crate::tools::Tool;
use crate::types::ToolResult;
use regex::Regex;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

/// Tool that searches file contents in the workspace using a regular expression.
///
/// Walks files recursively under the given `path` (defaults to workspace root),
/// reads each file, and returns lines that match the `pattern` as `file:line: content`.
/// Files that cannot be read as UTF-8 are silently skipped.
pub struct GrepTool;

impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Searches file contents in the workspace using a regular expression. \
         Returns matching lines with file:line prefix. Supports an optional \
         'path' parameter to restrict the search to a subdirectory."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regular expression pattern to search for."
                },
                "path": {
                    "type": "string",
                    "description": "Optional subdirectory to search in (default: workspace root)."
                }
            },
            "required": ["pattern"]
        })
    }

    fn execute(&self, params: &Value, ctx: &ToolContext) -> Result<ToolResult> {
        let pattern_str = params["pattern"].as_str().ok_or_else(|| {
            HarnessError::ToolExecution("Missing 'pattern' parameter".to_string())
        })?;

        let re = Regex::new(pattern_str).map_err(|e| {
            HarnessError::ToolExecution(format!("Invalid regex pattern: {}", e))
        })?;

        let subpath = params["path"].as_str().unwrap_or(".");
        let search_root = ctx.workspace_root.join(subpath);

        if !search_root.exists() {
            return Err(HarnessError::ToolExecution(format!(
                "Path not found: {}",
                search_root.display()
            )));
        }

        let mut matches: Vec<String> = Vec::new();
        let mut files_searched: u64 = 0;
        walk_and_search(&search_root, &ctx.workspace_root, &re, &mut matches, &mut files_searched)?;

        let content = if matches.is_empty() {
            "No matches found.".to_string()
        } else {
            matches.join("\n")
        };

        Ok(ToolResult {
            success: true,
            content,
            structured: Some(json!({
                "match_count": matches.len(),
                "files_searched": files_searched,
            })),
            artifacts: vec![],
        })
    }
}

/// Recursively walk `dir`, read each file, and append matching lines to `matches`.
///
/// Each match is formatted as `relative_path:line_num: content`.
/// `workspace_root` is used to compute relative paths.
fn walk_and_search(
    dir: &Path,
    workspace_root: &Path,
    re: &Regex,
    matches: &mut Vec<String>,
    files_searched: &mut u64,
) -> Result<()> {
    let entries = fs::read_dir(dir).map_err(|e| {
        HarnessError::ToolExecution(format!("Failed to read directory {}: {}", dir.display(), e))
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| {
            HarnessError::ToolExecution(format!("Failed to read directory entry: {}", e))
        })?;
        let path = entry.path();

        if path.is_dir() {
            walk_and_search(&path, workspace_root, re, matches, files_searched)?;
        } else if path.is_file() {
            *files_searched += 1;
            search_file(&path, workspace_root, re, matches);
        }
    }
    Ok(())
}

/// Search a single file for regex matches. Silently skips files that cannot
/// be read as UTF-8.
fn search_file(file_path: &Path, workspace_root: &Path, re: &Regex, matches: &mut Vec<String>) {
    let content = match fs::read_to_string(file_path) {
        Ok(c) => c,
        Err(_) => return, // skip binary / unreadable files
    };

    let relative = file_path
        .strip_prefix(workspace_root)
        .unwrap_or(file_path);

    for (i, line) in content.lines().enumerate() {
        if re.is_match(line) {
            matches.push(format!("{}:{}: {}", relative.display(), i + 1, line));
        }
    }
}

// ---------------------------------------------------------------------------
// GlobTool
// ---------------------------------------------------------------------------

/// Tool that finds files matching a glob pattern in the workspace.
///
/// Uses the `glob` crate to match files relative to the workspace root.
/// Returns a list of matching file paths, one per line.
pub struct GlobTool;

impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Finds files matching a glob pattern in the workspace. \
         Returns a newline-separated list of matching file paths."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The glob pattern to match files against (e.g. '**/*.rs')."
                }
            },
            "required": ["pattern"]
        })
    }

    fn execute(&self, params: &Value, ctx: &ToolContext) -> Result<ToolResult> {
        let pattern_str = params["pattern"].as_str().ok_or_else(|| {
            HarnessError::ToolExecution("Missing 'pattern' parameter".to_string())
        })?;

        // Build the full glob pattern relative to the workspace root.
        let full_pattern = ctx.workspace_root.join(pattern_str);
        let pattern_str = full_pattern.to_string_lossy().to_string();

        let paths: Vec<String> = glob::glob(&pattern_str)
            .map_err(|e| {
                HarnessError::ToolExecution(format!("Invalid glob pattern: {}", e))
            })?
            .filter_map(|entry| match entry {
                Ok(path) => {
                    let relative = path
                        .strip_prefix(&ctx.workspace_root)
                        .unwrap_or(&path);
                    Some(relative.display().to_string())
                }
                Err(e) => {
                    // Log and skip entries that can't be read.
                    tracing::warn!("Glob entry error: {}", e);
                    None
                }
            })
            .collect();

        let content = if paths.is_empty() {
            "No files matched.".to_string()
        } else {
            paths.join("\n")
        };

        Ok(ToolResult {
            success: true,
            content,
            structured: Some(json!({
                "match_count": paths.len(),
            })),
            artifacts: vec![],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_workspace() -> (TempDir, ToolContext) {
        let dir = TempDir::new().expect("failed to create temp dir");
        let ctx = ToolContext {
            workspace_root: dir.path().to_path_buf(),
            ..ToolContext::default()
        };
        (dir, ctx)
    }

    // ------------------------------------------------------------------
    // GrepTool tests
    // ------------------------------------------------------------------

    #[test]
    fn test_grep_finds_matches() {
        let (_dir, ctx) = setup_workspace();
        std::fs::write(
            ctx.workspace_root.join("a.txt"),
            "hello world\nfoo bar\nbaz\n",
        )
        .expect("write a.txt");
        std::fs::write(
            ctx.workspace_root.join("b.txt"),
            "line one\nhello again\nline three\n",
        )
        .expect("write b.txt");

        let tool = GrepTool;
        let result = tool
            .execute(&json!({"pattern": "hello"}), &ctx)
            .expect("grep should succeed");

        assert!(result.success);
        // Should match "hello world" in a.txt and "hello again" in b.txt
        assert!(result.content.contains("a.txt:1: hello world"));
        assert!(result.content.contains("b.txt:2: hello again"));
        // Should not contain non-matching lines
        assert!(!result.content.contains("foo bar"));
        assert!(!result.content.contains("baz"));

        let structured = result.structured.expect("should have structured data");
        assert_eq!(structured["match_count"], 2);
        assert!(structured["files_searched"].as_u64().unwrap() >= 2);
    }

    #[test]
    fn test_grep_no_matches() {
        let (_dir, ctx) = setup_workspace();
        std::fs::write(
            ctx.workspace_root.join("a.txt"),
            "foo\nbar\nbaz\n",
        )
        .expect("write a.txt");

        let tool = GrepTool;
        let result = tool
            .execute(&json!({"pattern": "nomatch"}), &ctx)
            .expect("grep should succeed");

        assert!(result.success);
        assert!(result.content.contains("No matches found"));

        let structured = result.structured.expect("should have structured data");
        assert_eq!(structured["match_count"], 0);
    }

    #[test]
    fn test_grep_with_path_filter() {
        let (_dir, ctx) = setup_workspace();
        let sub = ctx.workspace_root.join("subdir");
        std::fs::create_dir(&sub).expect("create subdir");
        std::fs::write(ctx.workspace_root.join("root.txt"), "needle\n").expect("write root");
        std::fs::write(sub.join("sub.txt"), "needle in sub\n").expect("write sub");

        let tool = GrepTool;
        let result = tool
            .execute(&json!({"pattern": "needle", "path": "subdir"}), &ctx)
            .expect("grep should succeed");

        assert!(result.success);
        assert!(result.content.contains("sub.txt:1: needle in sub"));
        assert!(!result.content.contains("root.txt"));
    }

    #[test]
    fn test_grep_invalid_regex() {
        let (_dir, ctx) = setup_workspace();
        let tool = GrepTool;
        let result = tool.execute(&json!({"pattern": "["}), &ctx);
        assert!(result.is_err());
        match result {
            Err(HarnessError::ToolExecution(msg)) => {
                assert!(msg.contains("Invalid regex"));
            }
            other => panic!("expected ToolExecution error, got {:?}", other),
        }
    }

    #[test]
    fn test_grep_missing_pattern() {
        let (_dir, ctx) = setup_workspace();
        let tool = GrepTool;
        let result = tool.execute(&json!({}), &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn test_grep_nonexistent_path() {
        let (_dir, ctx) = setup_workspace();
        let tool = GrepTool;
        let result = tool.execute(&json!({"pattern": "foo", "path": "nonexistent"}), &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn test_grep_skips_binary_files() {
        let (_dir, ctx) = setup_workspace();
        // Write a file with non-UTF-8 bytes
        std::fs::write(ctx.workspace_root.join("bin.bin"), [0x00, 0xFF, 0xFE, 0xFD])
            .expect("write binary");
        std::fs::write(ctx.workspace_root.join("text.txt"), "hello\n").expect("write text");

        let tool = GrepTool;
        let result = tool
            .execute(&json!({"pattern": ".*"}), &ctx)
            .expect("grep should succeed");

        assert!(result.success);
        // Binary file should be skipped, text file should be searched
        assert!(result.content.contains("text.txt"));
        assert!(!result.content.contains("bin.bin"));
    }

    #[test]
    fn test_grep_recursive_search() {
        let (_dir, ctx) = setup_workspace();
        let nested = ctx.workspace_root.join("a").join("b");
        std::fs::create_dir_all(&nested).expect("create nested dirs");
        std::fs::write(nested.join("deep.txt"), "found in deep\n").expect("write deep");

        let tool = GrepTool;
        let result = tool
            .execute(&json!({"pattern": "found"}), &ctx)
            .expect("grep should succeed");

        assert!(result.success);
        assert!(result.content.contains("deep.txt:1: found in deep"));
    }

    // ------------------------------------------------------------------
    // GlobTool tests
    // ------------------------------------------------------------------

    #[test]
    fn test_glob_finds_files() {
        let (_dir, ctx) = setup_workspace();
        std::fs::write(ctx.workspace_root.join("a.rs"), "// a").expect("write a.rs");
        std::fs::write(ctx.workspace_root.join("b.rs"), "// b").expect("write b.rs");
        std::fs::write(ctx.workspace_root.join("c.txt"), "c").expect("write c.txt");

        let tool = GlobTool;
        let result = tool
            .execute(&json!({"pattern": "*.rs"}), &ctx)
            .expect("glob should succeed");

        assert!(result.success);
        assert!(result.content.contains("a.rs"));
        assert!(result.content.contains("b.rs"));
        assert!(!result.content.contains("c.txt"));
    }

    #[test]
    fn test_glob_no_matches() {
        let (_dir, ctx) = setup_workspace();
        std::fs::write(ctx.workspace_root.join("a.txt"), "a").expect("write a.txt");

        let tool = GlobTool;
        let result = tool
            .execute(&json!({"pattern": "*.rs"}), &ctx)
            .expect("glob should succeed");

        assert!(result.success);
        assert!(result.content.contains("No files matched"));
    }

    #[test]
    fn test_glob_recursive_pattern() {
        let (_dir, ctx) = setup_workspace();
        let sub = ctx.workspace_root.join("subdir");
        std::fs::create_dir(&sub).expect("create subdir");
        std::fs::write(ctx.workspace_root.join("root.rs"), "root").expect("write root");
        std::fs::write(sub.join("sub.rs"), "sub").expect("write sub");

        let tool = GlobTool;
        let result = tool
            .execute(&json!({"pattern": "**/*.rs"}), &ctx)
            .expect("glob should succeed");

        assert!(result.success);
        assert!(result.content.contains("root.rs"));
        assert!(result.content.contains("subdir/sub.rs"));
    }

    #[test]
    fn test_glob_missing_pattern() {
        let (_dir, ctx) = setup_workspace();
        let tool = GlobTool;
        let result = tool.execute(&json!({}), &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn test_glob_returns_relative_paths() {
        let (_dir, ctx) = setup_workspace();
        let sub = ctx.workspace_root.join("src");
        std::fs::create_dir(&sub).expect("create src");
        std::fs::write(sub.join("main.rs"), "fn main() {}").expect("write main.rs");

        let tool = GlobTool;
        let result = tool
            .execute(&json!({"pattern": "src/*.rs"}), &ctx)
            .expect("glob should succeed");

        assert!(result.success);
        assert!(result.content.contains("src/main.rs"));
        // Should not contain absolute paths
        assert!(!result.content.contains(ctx.workspace_root.to_string_lossy().as_ref()));
    }
}