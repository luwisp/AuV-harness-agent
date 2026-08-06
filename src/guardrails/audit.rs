// ============================================================================
// Audit Log
// ============================================================================
//
// Provides `AuditLog` and `AuditEntry` for recording guardrail actions to a
// JSONL (JSON Lines) file.  Each entry is appended to the file immediately
// when `record()` is called, ensuring durability even if the process crashes.

use chrono::Utc;
use serde::Serialize;
use std::io::Write;
use crate::error::Result;

// ============================================================================
// AuditEntry
// ============================================================================

/// A single entry in the audit log, recording a guardrail decision.
///
/// Each entry captures the timestamp, session context, action summary,
/// risk level, final decision, who approved it (if anyone), and the
/// reasons that contributed to the decision.
#[derive(Debug, Clone, Serialize)]
pub struct AuditEntry {
    /// When the audit entry was created (UTC).
    pub timestamp: chrono::DateTime<Utc>,
    /// The session in which the action occurred.
    pub session_id: String,
    /// A human-readable summary of the action (e.g. "bash: cargo build").
    pub action_summary: String,
    /// The risk level assigned by the risk assessment layer.
    pub risk_level: String,
    /// The final decision (e.g. "Allowed", "Denied", "Timeout").
    pub decision: String,
    /// Who or what approved the action, if applicable.
    pub approver: Option<String>,
    /// The reasons that contributed to the decision.
    pub reasons: Vec<String>,
}

// ============================================================================
// AuditLog
// ============================================================================

/// A JSONL-based audit log that writes each entry to disk immediately.
///
/// # Examples
///
/// ```
/// use harness_agent::guardrails::audit::{AuditLog, AuditEntry};
/// use chrono::Utc;
///
/// let mut log = AuditLog::new("/tmp/audit.jsonl".into());
/// let entry = AuditEntry {
///     timestamp: Utc::now(),
///     session_id: "s1".into(),
///     action_summary: "bash: cargo test".into(),
///     risk_level: "Low".into(),
///     decision: "Allowed".into(),
///     approver: None,
///     reasons: vec![],
/// };
/// log.record(entry).unwrap();
/// assert_eq!(log.get_entries().len(), 1);
/// ```
pub struct AuditLog {
    /// Path to the JSONL output file.
    output: std::path::PathBuf,
    /// In-memory copy of all recorded entries.
    entries: Vec<AuditEntry>,
}

impl AuditLog {
    /// Create a new audit log that writes to the given file path.
    ///
    /// The file is created (or truncated) when the log is first created.
    pub fn new(output: std::path::PathBuf) -> Self {
        Self {
            output,
            entries: Vec::new(),
        }
    }

    /// Record an audit entry.
    ///
    /// The entry is appended to the in-memory list and immediately written
    /// to the JSONL file.  Returns an error if the write fails.
    pub fn record(&mut self, entry: AuditEntry) -> Result<()> {
        // Serialize to JSON line
        let line = serde_json::to_string(&entry).unwrap_or_else(|_| {
            r#"{"error":"failed to serialize audit entry"}"#.to_string()
        });

        // Ensure parent directory exists
        if let Some(parent) = self.output.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }

        // Append to file immediately
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.output)?;
        writeln!(file, "{line}")?;
        file.flush()?;

        self.entries.push(entry);
        Ok(())
    }

    /// Return a reference to all recorded entries (in order).
    pub fn get_entries(&self) -> &[AuditEntry] {
        &self.entries
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    /// Helper: create a sample audit entry.
    fn sample_entry(session_id: &str, action_summary: &str) -> AuditEntry {
        AuditEntry {
            timestamp: Utc::now(),
            session_id: session_id.to_string(),
            action_summary: action_summary.to_string(),
            risk_level: "Medium".to_string(),
            decision: "Allowed".to_string(),
            approver: None,
            reasons: vec!["Test reason".to_string()],
        }
    }

    // -----------------------------------------------------------------------
    // test_audit_log_writes_to_file
    // -----------------------------------------------------------------------

    #[test]
    fn test_audit_log_writes_to_file() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let file_path = dir.path().join("audit.jsonl");

        let mut log = AuditLog::new(file_path.clone());
        let entry = sample_entry("s1", "bash: cargo test");
        log.record(entry).unwrap();

        // Verify in-memory entries
        assert_eq!(log.get_entries().len(), 1);

        // Verify file contains the JSON line
        let content = std::fs::read_to_string(&file_path).expect("read audit file");
        assert!(
            content.contains("s1"),
            "file should contain session_id 's1', got: {content}"
        );
        assert!(
            content.contains("bash: cargo test"),
            "file should contain action summary, got: {content}"
        );

        // Verify the content is valid JSON
        let parsed: serde_json::Value =
            serde_json::from_str(content.trim()).expect("file content should be valid JSON");
        assert_eq!(parsed["session_id"], "s1");
        assert_eq!(parsed["action_summary"], "bash: cargo test");
        assert_eq!(parsed["risk_level"], "Medium");
        assert_eq!(parsed["decision"], "Allowed");
    }

    // -----------------------------------------------------------------------
    // test_audit_log_multiple_entries
    // -----------------------------------------------------------------------

    #[test]
    fn test_audit_log_multiple_entries() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let file_path = dir.path().join("audit.jsonl");

        let mut log = AuditLog::new(file_path.clone());

        let summaries = [
            "bash: cargo build",
            "bash: cargo test",
            "write_file: src/main.rs",
        ];

        for (i, summary) in summaries.iter().enumerate() {
            let entry = sample_entry(&format!("s{i}"), summary);
            log.record(entry).unwrap();
        }

        // Verify in-memory entries
        assert_eq!(log.get_entries().len(), 3);

        // Verify file contains all 3 entries
        let content = std::fs::read_to_string(&file_path).expect("read audit file");
        let lines: Vec<&str> = content.trim().lines().collect();
        assert_eq!(
            lines.len(),
            3,
            "expected 3 JSONL lines, got {}",
            lines.len()
        );

        // Each line should be valid JSON
        for line in &lines {
            let parsed: serde_json::Value =
                serde_json::from_str(line).expect("each line should be valid JSON");
            assert!(parsed.is_object(), "each line should be a JSON object");
        }

        // Verify specific entries
        for (i, summary) in summaries.iter().enumerate() {
            let parsed: serde_json::Value = serde_json::from_str(lines[i])
                .expect("line should parse");
            assert_eq!(parsed["session_id"], format!("s{i}"));
            assert_eq!(parsed["action_summary"], *summary);
        }
    }

    // -----------------------------------------------------------------------
    // test_audit_log_entries_include_timestamp
    // -----------------------------------------------------------------------

    #[test]
    fn test_audit_log_entries_include_timestamp() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let file_path = dir.path().join("audit.jsonl");

        let before = Utc::now();
        let mut log = AuditLog::new(file_path.clone());
        let entry = sample_entry("s1", "bash: echo hello");
        log.record(entry).unwrap();
        let after = Utc::now();

        // Verify in-memory entry has timestamp
        let recorded = &log.get_entries()[0];
        assert!(
            recorded.timestamp >= before,
            "timestamp should be >= before, got {:?} vs {:?}",
            recorded.timestamp,
            before
        );
        assert!(
            recorded.timestamp <= after,
            "timestamp should be <= after, got {:?} vs {:?}",
            recorded.timestamp,
            after
        );

        // Verify file contains a timestamp field
        let content = std::fs::read_to_string(&file_path).expect("read audit file");
        let parsed: serde_json::Value =
            serde_json::from_str(content.trim()).expect("file content should be valid JSON");
        let ts_field = parsed["timestamp"]
            .as_str()
            .expect("timestamp should be a string");
        assert!(
            !ts_field.is_empty(),
            "timestamp field should not be empty"
        );

        // The timestamp should be parseable as an ISO 8601 datetime
        let _parsed_ts: chrono::DateTime<Utc> = chrono::DateTime::parse_from_rfc3339(ts_field)
            .expect("timestamp should be valid RFC 3339")
            .with_timezone(&Utc);
    }
}