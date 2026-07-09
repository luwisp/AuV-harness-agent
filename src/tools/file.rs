use crate::error::{HarnessError, Result};
use crate::tools::context::ToolContext;
use crate::tools::Tool;
use crate::types::ToolResult;
use serde_json::{json, Value};

/// Tool that reads the content of a file with optional line-range filtering.
///
/// Returns the file content with line numbers. Supports `offset` (1-indexed)
/// and `limit` parameters to read a subset of lines.
pub struct ReadFileTool;

impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Reads a file from the workspace. Returns content with line numbers. \
         Supports optional 'offset' (1-indexed start line) and 'limit' (max lines) parameters."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file, relative to workspace root."
                },
                "offset": {
                    "type": "integer",
                    "description": "1-indexed line number to start reading from (default: 1).",
                    "minimum": 1
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to return (default: read all).",
                    "minimum": 1
                }
            },
            "required": ["path"]
        })
    }

    fn execute(&self, params: &Value, ctx: &ToolContext) -> Result<ToolResult> {
        let path_str = params["path"]
            .as_str()
            .ok_or_else(|| HarnessError::ToolExecution("Missing 'path' parameter".to_string()))?;

        let file_path = ctx.workspace_root.join(path_str);

        if !file_path.exists() {
            return Err(HarnessError::ToolExecution(format!(
                "File not found: {}",
                file_path.display()
            )));
        }

        if !file_path.is_file() {
            return Err(HarnessError::ToolExecution(format!(
                "Path is not a file: {}",
                file_path.display()
            )));
        }

        let content = std::fs::read_to_string(&file_path)?;
        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        let offset = params["offset"]
            .as_u64()
            .map(|o| o as usize)
            .unwrap_or(1);
        let limit = params["limit"].as_u64().map(|l| l as usize);

        if offset < 1 {
            return Err(HarnessError::ToolExecution(
                "Offset must be >= 1".to_string(),
            ));
        }

        let start = offset - 1; // convert to 0-indexed
        if start >= total_lines {
            return Ok(ToolResult {
                success: true,
                content: format!(
                    "Offset {} exceeds file length ({} lines). File is empty from that offset.",
                    offset, total_lines
                ),
                structured: Some(json!({
                    "total_lines": total_lines,
                    "offset": offset,
                    "limit": limit,
                    "returned_lines": 0
                })),
                artifacts: vec![],
            });
        }

        let end = match limit {
            Some(lim) => std::cmp::min(start + lim, total_lines),
            None => total_lines,
        };

        let selected = &lines[start..end];
        let returned_count = selected.len();

        // Format with right-aligned 6-digit line numbers
        let output: String = selected
            .iter()
            .enumerate()
            .map(|(i, line)| {
                let line_num = start + i + 1;
                format!("{:>6}\t{}", line_num, line)
            })
            .collect::<Vec<_>>()
            .join("\n");

        Ok(ToolResult {
            success: true,
            content: output,
            structured: Some(json!({
                "total_lines": total_lines,
                "offset": offset,
                "limit": limit,
                "returned_lines": returned_count
            })),
            artifacts: vec![],
        })
    }
}

/// Tool that writes content to a file, creating parent directories as needed.
///
/// Returns the number of bytes written.
pub struct WriteFileTool;

impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Writes content to a file in the workspace. Creates parent directories if they don't exist. \
         Returns the number of bytes written."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file, relative to workspace root."
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file."
                }
            },
            "required": ["path", "content"]
        })
    }

    fn execute(&self, params: &Value, ctx: &ToolContext) -> Result<ToolResult> {
        let path_str = params["path"]
            .as_str()
            .ok_or_else(|| HarnessError::ToolExecution("Missing 'path' parameter".to_string()))?;

        let content = params["content"]
            .as_str()
            .ok_or_else(|| HarnessError::ToolExecution("Missing 'content' parameter".to_string()))?;

        let file_path = ctx.workspace_root.join(path_str);

        // Create parent directories if they don't exist
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(&file_path, content)?;

        let byte_count = content.len();

        Ok(ToolResult {
            success: true,
            content: format!("Wrote {} bytes to {}", byte_count, file_path.display()),
            structured: Some(json!({
                "path": file_path.to_string_lossy(),
                "bytes_written": byte_count
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
    // ReadFileTool tests
    // ------------------------------------------------------------------

    #[test]
    fn test_read_file() {
        let (_dir, ctx) = setup_workspace();
        let content = "line one\nline two\nline three\n";
        std::fs::write(ctx.workspace_root.join("test.txt"), content)
            .expect("failed to write test file");

        let tool = ReadFileTool;
        let result = tool
            .execute(&json!({"path": "test.txt"}), &ctx)
            .expect("read_file should succeed");

        assert!(result.success);
        assert!(result.content.contains("line one"));
        assert!(result.content.contains("line two"));
        assert!(result.content.contains("line three"));

        // Verify line numbers are present
        assert!(result.content.contains("     1\t"));
        assert!(result.content.contains("     2\t"));
        assert!(result.content.contains("     3\t"));

        // Verify structured output
        let structured = result.structured.expect("should have structured data");
        assert_eq!(structured["total_lines"], 3);
        assert_eq!(structured["offset"], 1);
        assert_eq!(structured["limit"], Value::Null);
        assert_eq!(structured["returned_lines"], 3);
    }

    #[test]
    fn test_read_file_with_line_range() {
        let (_dir, ctx) = setup_workspace();
        let content = "line 1\nline 2\nline 3\nline 4\nline 5\n";
        std::fs::write(ctx.workspace_root.join("test.txt"), content)
            .expect("failed to write test file");

        let tool = ReadFileTool;
        let result = tool
            .execute(&json!({"path": "test.txt", "offset": 2, "limit": 2}), &ctx)
            .expect("read_file should succeed");

        assert!(result.success);

        // Should contain lines 2 and 3 but not 1, 4, or 5
        assert!(!result.content.contains("line 1\n"));
        // line 1 might still appear if line 10 had "line 1" in it, but we have 5 lines
        // so check line numbers instead
        assert!(result.content.contains("     2\tline 2"));
        assert!(result.content.contains("     3\tline 3"));
        assert!(!result.content.contains("     4\t"));
        assert!(!result.content.contains("     1\t"));

        let structured = result.structured.expect("should have structured data");
        assert_eq!(structured["total_lines"], 5);
        assert_eq!(structured["offset"], 2);
        assert_eq!(structured["limit"], 2);
        assert_eq!(structured["returned_lines"], 2);
    }

    #[test]
    fn test_read_file_offset_only() {
        let (_dir, ctx) = setup_workspace();
        let content = "a\nb\nc\nd\ne\n";
        std::fs::write(ctx.workspace_root.join("test.txt"), content)
            .expect("failed to write test file");

        let tool = ReadFileTool;
        let result = tool
            .execute(&json!({"path": "test.txt", "offset": 3}), &ctx)
            .expect("read_file should succeed");

        assert!(result.success);
        assert!(result.content.contains("     3\tc"));
        assert!(result.content.contains("     4\td"));
        assert!(result.content.contains("     5\te"));
        assert!(!result.content.contains("     1\t"));
        assert!(!result.content.contains("     2\t"));

        let structured = result.structured.expect("should have structured data");
        assert_eq!(structured["returned_lines"], 3);
    }

    #[test]
    fn test_read_file_limit_only() {
        let (_dir, ctx) = setup_workspace();
        let content = "a\nb\nc\nd\ne\n";
        std::fs::write(ctx.workspace_root.join("test.txt"), content)
            .expect("failed to write test file");

        let tool = ReadFileTool;
        let result = tool
            .execute(&json!({"path": "test.txt", "limit": 2}), &ctx)
            .expect("read_file should succeed");

        assert!(result.success);
        assert!(result.content.contains("     1\ta"));
        assert!(result.content.contains("     2\tb"));
        assert!(!result.content.contains("     3\t"));

        let structured = result.structured.expect("should have structured data");
        assert_eq!(structured["returned_lines"], 2);
    }

    #[test]
    fn test_read_file_nonexistent() {
        let (_dir, ctx) = setup_workspace();
        let tool = ReadFileTool;
        let result = tool.execute(&json!({"path": "nonexistent.txt"}), &ctx);
        assert!(result.is_err());
        match result {
            Err(HarnessError::ToolExecution(msg)) => {
                assert!(msg.contains("File not found"));
            }
            other => panic!("expected ToolExecution error, got {:?}", other),
        }
    }

    #[test]
    fn test_read_file_path_is_directory() {
        let (_dir, ctx) = setup_workspace();
        let subdir = ctx.workspace_root.join("subdir");
        std::fs::create_dir(&subdir).expect("failed to create subdir");

        let tool = ReadFileTool;
        let result = tool.execute(&json!({"path": "subdir"}), &ctx);
        assert!(result.is_err());
        match result {
            Err(HarnessError::ToolExecution(msg)) => {
                assert!(msg.contains("not a file"));
            }
            other => panic!("expected ToolExecution error, got {:?}", other),
        }
    }

    #[test]
    fn test_read_file_offset_beyond_eof() {
        let (_dir, ctx) = setup_workspace();
        let content = "only one line\n";
        std::fs::write(ctx.workspace_root.join("test.txt"), content)
            .expect("failed to write test file");

        let tool = ReadFileTool;
        let result = tool
            .execute(&json!({"path": "test.txt", "offset": 10}), &ctx)
            .expect("read_file should succeed");

        assert!(result.success);
        assert!(result.content.contains("exceeds file length"));
        let structured = result.structured.expect("should have structured data");
        assert_eq!(structured["returned_lines"], 0);
    }

    // ------------------------------------------------------------------
    // WriteFileTool tests
    // ------------------------------------------------------------------

    #[test]
    fn test_write_file() {
        let (_dir, ctx) = setup_workspace();
        let tool = WriteFileTool;
        let result = tool
            .execute(
                &json!({"path": "output.txt", "content": "hello world"}),
                &ctx,
            )
            .expect("write_file should succeed");

        assert!(result.success);
        assert!(result.content.contains("11 bytes"));
        assert!(result.content.contains("output.txt"));

        let written =
            std::fs::read_to_string(ctx.workspace_root.join("output.txt"))
                .expect("output file should exist");
        assert_eq!(written, "hello world");

        let structured = result.structured.expect("should have structured data");
        assert_eq!(structured["bytes_written"], 11);
    }

    #[test]
    fn test_write_file_creates_parent_dirs() {
        let (_dir, ctx) = setup_workspace();
        let tool = WriteFileTool;
        let result = tool
            .execute(
                &json!({"path": "sub/dir/output.txt", "content": "nested content"}),
                &ctx,
            )
            .expect("write_file should succeed");

        assert!(result.success);
        assert!(result.content.contains("14 bytes"));

        let written =
            std::fs::read_to_string(ctx.workspace_root.join("sub/dir/output.txt"))
                .expect("nested file should exist");
        assert_eq!(written, "nested content");
    }

    #[test]
    fn test_write_file_overwrite() {
        let (_dir, ctx) = setup_workspace();
        let file_path = ctx.workspace_root.join("overwrite.txt");
        std::fs::write(&file_path, "original").expect("failed to write original");

        let tool = WriteFileTool;
        let result = tool
            .execute(
                &json!({"path": "overwrite.txt", "content": "replaced"}),
                &ctx,
            )
            .expect("write_file should succeed");

        assert!(result.success);
        let written =
            std::fs::read_to_string(&file_path).expect("file should exist");
        assert_eq!(written, "replaced");
    }

    #[test]
    fn test_write_file_empty_content() {
        let (_dir, ctx) = setup_workspace();
        let tool = WriteFileTool;
        let result = tool
            .execute(
                &json!({"path": "empty.txt", "content": ""}),
                &ctx,
            )
            .expect("write_file should succeed");

        assert!(result.success);
        assert!(result.content.contains("0 bytes"));

        let written =
            std::fs::read_to_string(ctx.workspace_root.join("empty.txt"))
                .expect("empty file should exist");
        assert_eq!(written, "");
    }

    #[test]
    fn test_write_file_missing_path() {
        let (_dir, ctx) = setup_workspace();
        let tool = WriteFileTool;
        let result = tool.execute(&json!({"content": "no path"}), &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn test_write_file_missing_content() {
        let (_dir, ctx) = setup_workspace();
        let tool = WriteFileTool;
        let result = tool.execute(&json!({"path": "f.txt"}), &ctx);
        assert!(result.is_err());
    }
}