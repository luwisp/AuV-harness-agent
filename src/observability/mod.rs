use crate::types::{Action, FeedbackResult, GuardResult, Message, ToolResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEntry {
    pub turn: usize,
    pub timestamp: DateTime<Utc>,
    pub messages_snapshot: Vec<Message>,
    pub llm_response: String,
    pub parsed_action: Action,
    pub guard_result: GuardResult,
    pub tool_result: Option<ToolResult>,
    pub feedback_results: Vec<FeedbackResult>,
}

#[derive(Debug)]
pub struct TraceLog {
    entries: Vec<TraceEntry>,
    output: PathBuf,
}

impl TraceLog {
    pub fn new(output: PathBuf) -> Self {
        Self {
            entries: Vec::new(),
            output,
        }
    }

    pub fn record(&mut self, entry: TraceEntry) {
        self.entries.push(entry.clone());
        let json_line =
            serde_json::to_string(&entry).expect("TraceEntry should serialize to JSON");
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.output)
            .expect("should open trace log file for append");
        writeln!(file, "{json_line}").expect("should write JSONL line to trace log");
    }

    pub fn replay(&self) -> impl Iterator<Item = &TraceEntry> {
        self.entries.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Action, FeedbackResult, GuardResult, Message, Role};
    use std::io::BufRead;
    use tempfile::NamedTempFile;

    fn make_sample_entry(turn: usize) -> TraceEntry {
        TraceEntry {
            turn,
            timestamp: Utc::now(),
            messages_snapshot: vec![Message {
                role: Role::User,
                content: format!("test message {turn}"),
                tool_calls: None,
                tool_call_id: None,
            }],
            llm_response: format!("response {turn}"),
            parsed_action: Action::FinalAnswer {
                summary: format!("summary {turn}"),
            },
            guard_result: GuardResult::Allowed,
            tool_result: None,
            feedback_results: vec![FeedbackResult {
                channel: "lint".to_string(),
                passed: true,
                errors: vec![],
                summary: "all good".to_string(),
            }],
        }
    }

    #[test]
    fn test_trace_log_writes_jsonl() {
        let tmp = NamedTempFile::new().expect("tempfile");
        let path = tmp.path().to_path_buf();
        let mut log = TraceLog::new(path.clone());

        let entry1 = make_sample_entry(1);
        let entry2 = make_sample_entry(2);

        log.record(entry1.clone());
        log.record(entry2.clone());

        // Read the file and parse JSONL
        let file = std::fs::File::open(&path).expect("open file");
        let reader = std::io::BufReader::new(file);
        let lines: Vec<String> = reader.lines().map(|l| l.unwrap()).collect();

        assert_eq!(lines.len(), 2, "should have two JSONL lines");

        let parsed1: TraceEntry =
            serde_json::from_str(&lines[0]).expect("first line should deserialize");
        assert_eq!(parsed1.turn, 1);
        assert_eq!(parsed1.llm_response, "response 1");

        let parsed2: TraceEntry =
            serde_json::from_str(&lines[1]).expect("second line should deserialize");
        assert_eq!(parsed2.turn, 2);
        assert_eq!(parsed2.llm_response, "response 2");
    }

    #[test]
    fn test_trace_log_replay() {
        let tmp = NamedTempFile::new().expect("tempfile");
        let path = tmp.path().to_path_buf();
        let mut log = TraceLog::new(path);

        let entry1 = make_sample_entry(1);
        let entry2 = make_sample_entry(2);
        let entry3 = make_sample_entry(3);

        log.record(entry1.clone());
        log.record(entry2.clone());
        log.record(entry3.clone());

        let replayed: Vec<&TraceEntry> = log.replay().collect();
        assert_eq!(replayed.len(), 3, "should replay all three entries");
        assert_eq!(replayed[0].turn, 1);
        assert_eq!(replayed[1].turn, 2);
        assert_eq!(replayed[2].turn, 3);
    }
}