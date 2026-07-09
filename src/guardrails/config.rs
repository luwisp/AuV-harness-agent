//! Guardrail configuration file parsing.
//!
//! Reads custom guardrail rules from JSON or TOML files and merges them with
//! the built-in rule set.  Custom rules can override built-in rules when they
//! share the same `id` — the rule with the higher priority wins.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::error::HarnessError;
use crate::guardrails::rules::{FileOp, GuardRule, RuleAction, RulePattern};

// ---------------------------------------------------------------------------
// On-disk configuration structures
// ---------------------------------------------------------------------------

/// Top-level structure of a guardrail configuration file.
#[derive(Debug, Deserialize)]
struct ConfigFile {
    rules: Vec<ConfigRule>,
}

/// A single rule as it appears in a JSON or TOML config file.
#[derive(Debug, Deserialize)]
struct ConfigRule {
    id: String,
    name: String,
    pattern_type: String,
    pattern_value: serde_json::Value,
    action: String,
    priority: u8,
    /// Human-readable reason emitted when the rule denies an action.
    #[serde(default)]
    reason: Option<String>,
    /// File operation constraint (only meaningful for `FilePath` patterns).
    #[serde(default)]
    file_op: Option<String>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse a guardrail configuration file (JSON or TOML) and return the merged
/// set of built-in and custom rules.
///
/// # Merging strategy
///
/// 1. Load all built-in rules into a map keyed by rule `id`.
/// 2. Parse the custom rules from `path`.
/// 3. For each custom rule, insert it into the map **only if** no built-in rule
///    with the same `id` has a higher priority.  When the custom rule has a
///    higher (or equal) priority it replaces the built-in entry.
/// 4. Return the values of the map as a flat `Vec<GuardRule>`.
///
/// # File format
///
/// The file may be JSON (`.json`) or TOML (`.toml`).  Extension is used to
/// choose the parser.  Example JSON:
///
/// ```json
/// {
///   "rules": [
///     {
///       "id": "deny-rm-rf-root",
///       "name": "Custom rm -rf / override",
///       "pattern_type": "CommandGlob",
///       "pattern_value": ["*rm -rf /**"],
///       "action": "Escalate",
///       "priority": 200
///     }
///   ]
/// }
/// ```
pub fn parse_rules_from_file(path: &Path) -> Result<Vec<GuardRule>, HarnessError> {
    let custom_rules = read_rules_file(path)?;
    let builtin_rules = builtin_rules_vec();
    Ok(merge_rules(builtin_rules, custom_rules))
}

/// Merge two sets of rules.  When a rule in `custom` has the same `id` as a
/// rule in `builtin`, the entry with the higher priority is kept.  On a tie
/// the custom rule wins.
fn merge_rules(builtin: Vec<GuardRule>, custom: Vec<GuardRule>) -> Vec<GuardRule> {
    let mut map: HashMap<String, GuardRule> = HashMap::new();

    for rule in builtin {
        map.insert(rule.id.clone(), rule);
    }

    for rule in custom {
        match map.get(&rule.id) {
            Some(existing) if existing.priority > rule.priority => {
                // Built-in has higher priority — keep it.
            }
            _ => {
                // Custom rule has higher or equal priority — replace.
                map.insert(rule.id.clone(), rule);
            }
        }
    }

    map.into_values().collect()
}

// ---------------------------------------------------------------------------
// File reading
// ---------------------------------------------------------------------------

/// Read a JSON or TOML rules file and convert each `ConfigRule` into a
/// `GuardRule`.
fn read_rules_file(path: &Path) -> Result<Vec<GuardRule>, HarnessError> {
    let content =
        std::fs::read_to_string(path).map_err(|e| HarnessError::Config(format!(
            "Failed to read guardrail config file {}: {e}",
            path.display()
        )))?;

    let config: ConfigFile = match path.extension().and_then(|e| e.to_str()) {
        Some("json") => serde_json::from_str(&content).map_err(|e| {
            HarnessError::Config(format!("Invalid JSON in {}: {e}", path.display()))
        })?,
        Some("toml") => toml::from_str(&content).map_err(|e| {
            HarnessError::Config(format!("Invalid TOML in {}: {e}", path.display()))
        })?,
        Some(ext) => {
            return Err(HarnessError::Config(format!(
                "Unsupported guardrail config format: .{ext} (expected .json or .toml)"
            )));
        }
        None => {
            return Err(HarnessError::Config(
                "Guardrail config file has no extension (expected .json or .toml)".into(),
            ));
        }
    };

    config
        .rules
        .into_iter()
        .map(|cr| config_rule_to_guard_rule(cr, path))
        .collect()
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

/// Convert a deserialized `ConfigRule` into a `GuardRule`.
fn config_rule_to_guard_rule(
    cr: ConfigRule,
    _path: &Path,
) -> Result<GuardRule, HarnessError> {
    let pattern = parse_pattern(&cr.pattern_type, &cr.pattern_value, cr.file_op.as_deref())?;
    let action = parse_action(&cr.action, cr.reason.as_deref(), &cr.name)?;

    Ok(GuardRule {
        id: cr.id,
        name: cr.name,
        pattern,
        action,
        priority: cr.priority,
    })
}

/// Parse a pattern type string and its associated value into a `RulePattern`.
fn parse_pattern(
    pattern_type: &str,
    value: &serde_json::Value,
    file_op: Option<&str>,
) -> Result<RulePattern, HarnessError> {
    match pattern_type {
        "CommandGlob" => {
            let globs = value
                .as_array()
                .ok_or_else(|| {
                    HarnessError::Config(
                        "CommandGlob pattern_value must be an array of strings".into(),
                    )
                })?
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(String::from)
                        .ok_or_else(|| {
                            HarnessError::Config(
                                "CommandGlob pattern_value entries must be strings".into(),
                            )
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(RulePattern::CommandGlob { globs })
        }
        "FilePath" => {
            let paths = value
                .as_array()
                .ok_or_else(|| {
                    HarnessError::Config(
                        "FilePath pattern_value must be an array of strings".into(),
                    )
                })?
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(String::from)
                        .ok_or_else(|| {
                            HarnessError::Config(
                                "FilePath pattern_value entries must be strings".into(),
                            )
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;

            let op = match file_op.unwrap_or("Any") {
                "Read" => FileOp::Read,
                "Write" => FileOp::Write,
                "Delete" => FileOp::Delete,
                "Any" => FileOp::Any,
                other => {
                    return Err(HarnessError::Config(format!(
                        "Unknown file_op '{other}' (expected Read, Write, Delete, or Any)"
                    )));
                }
            };

            Ok(RulePattern::FilePath { paths, op })
        }
        "NetworkDest" => {
            let hosts = value
                .as_array()
                .ok_or_else(|| {
                    HarnessError::Config(
                        "NetworkDest pattern_value must be an array of strings".into(),
                    )
                })?
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(String::from)
                        .ok_or_else(|| {
                            HarnessError::Config(
                                "NetworkDest pattern_value entries must be strings".into(),
                            )
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(RulePattern::NetworkDest { hosts })
        }
        "Composite" => {
            // Composite patterns are not yet supported from config files.
            Err(HarnessError::Config(
                "Composite patterns are not supported in config files".into(),
            ))
        }
        other => Err(HarnessError::Config(format!(
            "Unknown pattern_type '{other}' (expected CommandGlob, FilePath, NetworkDest, or Composite)"
        ))),
    }
}

/// Parse an action string into a `RuleAction`.
fn parse_action(
    action: &str,
    reason: Option<&str>,
    rule_name: &str,
) -> Result<RuleAction, HarnessError> {
    match action {
        "Allow" => Ok(RuleAction::Allow),
        "Deny" => Ok(RuleAction::Deny(
            reason.unwrap_or(rule_name).to_string(),
        )),
        "Escalate" => Ok(RuleAction::Escalate),
        other => Err(HarnessError::Config(format!(
            "Unknown action '{other}' (expected Allow, Deny, or Escalate)"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Built-in rules (kept private — use `parse_rules_from_file()` to get the
// merged set)
// ---------------------------------------------------------------------------

/// Return the set of built-in guardrail rules.
fn builtin_rules_vec() -> Vec<GuardRule> {
    vec![
        // Priority 100 – hard blocks
        GuardRule {
            id: "deny-rm-rf-root".into(),
            name: "Block rm -rf /".into(),
            pattern: RulePattern::CommandGlob {
                globs: vec!["*rm -rf /**".into()],
            },
            action: RuleAction::Deny("Destructive recursive deletion of root".into()),
            priority: 100,
        },
        GuardRule {
            id: "deny-drop-database".into(),
            name: "Block DROP DATABASE".into(),
            pattern: RulePattern::CommandGlob {
                globs: vec!["*DROP DATABASE*".into()],
            },
            action: RuleAction::Deny("Database deletion blocked".into()),
            priority: 100,
        },
        GuardRule {
            id: "deny-dd-if".into(),
            name: "Block dd if=".into(),
            pattern: RulePattern::CommandGlob {
                globs: vec!["*dd if=*".into()],
            },
            action: RuleAction::Deny("Block-level device operations blocked".into()),
            priority: 100,
        },
        GuardRule {
            id: "deny-mkfs".into(),
            name: "Block mkfs.*".into(),
            pattern: RulePattern::CommandGlob {
                globs: vec!["mkfs.*".into()],
            },
            action: RuleAction::Deny("Filesystem creation blocked".into()),
            priority: 100,
        },
        // Priority 50 – escalations
        GuardRule {
            id: "escalate-rm-rf-home".into(),
            name: "Escalate rm -rf ~".into(),
            pattern: RulePattern::CommandGlob {
                globs: vec!["*rm -rf ~*".into()],
            },
            action: RuleAction::Escalate,
            priority: 50,
        },
        GuardRule {
            id: "escalate-drop-table".into(),
            name: "Escalate DROP TABLE".into(),
            pattern: RulePattern::CommandGlob {
                globs: vec!["*DROP TABLE*".into()],
            },
            action: RuleAction::Escalate,
            priority: 50,
        },
        GuardRule {
            id: "escalate-curl-pipe-bash".into(),
            name: "Escalate curl | bash".into(),
            pattern: RulePattern::CommandGlob {
                globs: vec!["*curl*|*bash*".into()],
            },
            action: RuleAction::Escalate,
            priority: 50,
        },
        GuardRule {
            id: "escalate-git-push-force".into(),
            name: "Escalate git push --force".into(),
            pattern: RulePattern::CommandGlob {
                globs: vec!["*git push*--force*".into(), "*git push*-f*".into()],
            },
            action: RuleAction::Escalate,
            priority: 50,
        },
        GuardRule {
            id: "escalate-chmod-777".into(),
            name: "Escalate chmod 777".into(),
            pattern: RulePattern::CommandGlob {
                globs: vec!["*chmod 777*".into()],
            },
            action: RuleAction::Escalate,
            priority: 50,
        },
        // File-path based rules
        GuardRule {
            id: "escalate-write-etc".into(),
            name: "Escalate write to /etc/*".into(),
            pattern: RulePattern::FilePath {
                paths: vec!["/etc/**".into()],
                op: FileOp::Write,
            },
            action: RuleAction::Escalate,
            priority: 50,
        },
        GuardRule {
            id: "escalate-write-ssh".into(),
            name: "Escalate write to ~/.ssh/*".into(),
            pattern: RulePattern::FilePath {
                paths: vec!["~/.ssh/**".into()],
                op: FileOp::Write,
            },
            action: RuleAction::Escalate,
            priority: 50,
        },
        GuardRule {
            id: "escalate-write-dotenv".into(),
            name: "Escalate write to .env".into(),
            pattern: RulePattern::FilePath {
                paths: vec!["**/.env".into()],
                op: FileOp::Write,
            },
            action: RuleAction::Escalate,
            priority: 50,
        },
    ]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guardrails::rules::StaticRuleEngine;
    use crate::types::Action;
    use serde_json::json;
    use std::io::Write;
    use tempfile::NamedTempFile;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn bash_action(command: &str) -> Action {
        Action::ToolCall {
            id: "call-1".into(),
            name: "bash".into(),
            params: json!({"command": command}),
        }
    }

    /// Write `content` to a temp file with the given extension, return a
    /// handle that deletes the file on drop.
    fn temp_config(ext: &str, content: &str) -> NamedTempFile {
        let mut file = tempfile::Builder::new()
            .suffix(&format!(".{ext}"))
            .tempfile()
            .expect("create temp config file");
        file.write_all(content.as_bytes()).expect("write temp config");
        file
    }

    // -----------------------------------------------------------------------
    // test_parse_custom_rules_from_json
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_custom_rules_from_json() {
        let json_content = r#"{
            "rules": [
                {
                    "id": "custom-block-curl",
                    "name": "Block all curl",
                    "pattern_type": "CommandGlob",
                    "pattern_value": ["*curl*"],
                    "action": "Deny",
                    "priority": 200,
                    "reason": "curl is forbidden by policy"
                },
                {
                    "id": "custom-allow-ls",
                    "name": "Explicitly allow ls",
                    "pattern_type": "CommandGlob",
                    "pattern_value": ["ls*"],
                    "action": "Allow",
                    "priority": 100
                }
            ]
        }"#;

        let tmp = temp_config("json", json_content);
        let rules = parse_rules_from_file(tmp.path())
            .expect("parse custom rules from JSON");

        // Should contain our custom rules + built-in rules
        let custom_curl = rules.iter().find(|r| r.id == "custom-block-curl")
            .expect("custom-block-curl rule exists");
        assert_eq!(custom_curl.name, "Block all curl");
        assert_eq!(custom_curl.priority, 200);
        match &custom_curl.action {
            RuleAction::Deny(reason) => assert_eq!(reason, "curl is forbidden by policy"),
            other => panic!("expected Deny, got {:?}", other),
        }

        let custom_ls = rules.iter().find(|r| r.id == "custom-allow-ls")
            .expect("custom-allow-ls rule exists");
        assert_eq!(custom_ls.priority, 100);
        assert!(matches!(custom_ls.action, RuleAction::Allow));

        // Built-in rules should still be present
        let builtin = rules.iter().find(|r| r.id == "deny-rm-rf-root")
            .expect("builtin deny-rm-rf-root still present");
        assert_eq!(builtin.priority, 100);
    }

    // -----------------------------------------------------------------------
    // test_custom_rules_override_builtin
    // -----------------------------------------------------------------------

    #[test]
    fn test_custom_rules_override_builtin() {
        // Override the built-in "deny-rm-rf-root" (priority 100, Deny) with a
        // custom rule at priority 200 that Escalates instead.
        let json_content = r#"{
            "rules": [
                {
                    "id": "deny-rm-rf-root",
                    "name": "Custom: escalate rm -rf / instead of denying",
                    "pattern_type": "CommandGlob",
                    "pattern_value": ["*rm -rf /**"],
                    "action": "Escalate",
                    "priority": 200
                }
            ]
        }"#;

        let tmp = temp_config("json", json_content);
        let rules = parse_rules_from_file(tmp.path())
            .expect("parse override rules");

        // The rule with id "deny-rm-rf-root" should be the custom Escalate
        // version (priority 200), not the built-in Deny (priority 100).
        let overridden = rules.iter().find(|r| r.id == "deny-rm-rf-root")
            .expect("deny-rm-rf-root rule exists");
        assert_eq!(overridden.priority, 200);
        assert!(
            matches!(overridden.action, RuleAction::Escalate),
            "expected Escalate, got {:?}",
            overridden.action
        );
        assert_eq!(overridden.name, "Custom: escalate rm -rf / instead of denying");

        // Now verify that this rule actually takes effect in the engine.
        let mut engine = StaticRuleEngine::new();
        for rule in &rules {
            engine.add_rule(rule.clone());
        }

        let ctx = crate::guardrails::GuardContext {
            session_id: "test".into(),
            workspace_root: std::path::PathBuf::from("/tmp"),
            user_id: None,
        };

        // rm -rf / should now escalate (custom rule) instead of being denied
        // (original built-in).
        let result = engine.evaluate(&bash_action("rm -rf /"), &ctx);
        assert!(
            result.needs_approval(),
            "rm -rf / should now escalate (custom rule overrode built-in), got {:?}",
            result
        );
    }

    // -----------------------------------------------------------------------
    // test_custom_rule_lower_priority_does_not_override
    // -----------------------------------------------------------------------

    #[test]
    fn test_custom_rule_lower_priority_does_not_override() {
        // A custom rule with lower priority than the built-in should NOT
        // override.
        let json_content = r#"{
            "rules": [
                {
                    "id": "deny-rm-rf-root",
                    "name": "Lower priority override attempt",
                    "pattern_type": "CommandGlob",
                    "pattern_value": ["*rm -rf /**"],
                    "action": "Escalate",
                    "priority": 50
                }
            ]
        }"#;

        let tmp = temp_config("json", json_content);
        let rules = parse_rules_from_file(tmp.path())
            .expect("parse lower-priority rules");

        let rule = rules.iter().find(|r| r.id == "deny-rm-rf-root")
            .expect("deny-rm-rf-root rule exists");
        // Built-in has priority 100, custom has 50 — built-in should win.
        assert_eq!(rule.priority, 100);
        assert!(matches!(rule.action, RuleAction::Deny(_)));
    }

    // -----------------------------------------------------------------------
    // test_parse_toml_config
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_toml_config() {
        let toml_content = r#"
[[rules]]
id = "toml-deny-sudo"
name = "Block sudo in TOML config"
pattern_type = "CommandGlob"
pattern_value = ["*sudo*"]
action = "Deny"
priority = 150
reason = "sudo is not allowed"

[[rules]]
id = "toml-escalate-write-etc"
name = "Escalate /etc writes from TOML"
pattern_type = "FilePath"
pattern_value = ["/etc/**"]
action = "Escalate"
priority = 75
file_op = "Write"
"#;

        let tmp = temp_config("toml", toml_content);
        let rules = parse_rules_from_file(tmp.path())
            .expect("parse TOML config");

        let sudo_rule = rules.iter().find(|r| r.id == "toml-deny-sudo")
            .expect("toml-deny-sudo rule exists");
        assert_eq!(sudo_rule.priority, 150);
        match &sudo_rule.action {
            RuleAction::Deny(reason) => assert_eq!(reason, "sudo is not allowed"),
            other => panic!("expected Deny, got {:?}", other),
        }

        let etc_rule = rules.iter().find(|r| r.id == "toml-escalate-write-etc")
            .expect("toml-escalate-write-etc rule exists");
        assert_eq!(etc_rule.priority, 75);
        assert!(matches!(etc_rule.action, RuleAction::Escalate));
        match &etc_rule.pattern {
            RulePattern::FilePath { paths, op } => {
                assert_eq!(paths, &["/etc/**"]);
                assert_eq!(*op, FileOp::Write);
            }
            other => panic!("expected FilePath, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Error cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_unsupported_extension() {
        let tmp = temp_config("yaml", "rules: []");
        let err = parse_rules_from_file(tmp.path()).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("Unsupported"), "expected unsupported format error, got: {msg}");
    }

    #[test]
    fn test_invalid_json() {
        let tmp = temp_config("json", "not valid json {{{");
        let err = parse_rules_from_file(tmp.path()).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("Invalid JSON"), "expected JSON error, got: {msg}");
    }

    #[test]
    fn test_unknown_pattern_type() {
        let json_content = r#"{
            "rules": [
                {
                    "id": "bad-rule",
                    "name": "Bad rule",
                    "pattern_type": "UnknownPattern",
                    "pattern_value": ["test"],
                    "action": "Deny",
                    "priority": 100
                }
            ]
        }"#;
        let tmp = temp_config("json", json_content);
        let err = parse_rules_from_file(tmp.path()).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("Unknown pattern_type"), "expected pattern type error, got: {msg}");
    }

    #[test]
    fn test_unknown_action() {
        let json_content = r#"{
            "rules": [
                {
                    "id": "bad-action",
                    "name": "Bad action",
                    "pattern_type": "CommandGlob",
                    "pattern_value": ["test"],
                    "action": "Explode",
                    "priority": 100
                }
            ]
        }"#;
        let tmp = temp_config("json", json_content);
        let err = parse_rules_from_file(tmp.path()).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("Unknown action"), "expected action error, got: {msg}");
    }

    #[test]
    fn test_file_not_found() {
        let err = parse_rules_from_file(std::path::Path::new("/nonexistent/path/rules.json"))
            .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("Failed to read"), "expected file-not-found error, got: {msg}");
    }

    #[test]
    fn test_empty_rules_file() {
        let json_content = r#"{"rules": []}"#;
        let tmp = temp_config("json", json_content);
        let rules = parse_rules_from_file(tmp.path())
            .expect("parse empty rules file");
        // Should only contain built-in rules.
        assert!(!rules.is_empty());
        // All rules should be from builtins.
        let builtin_ids: Vec<&str> = vec![
            "deny-rm-rf-root",
            "deny-drop-database",
            "deny-dd-if",
            "deny-mkfs",
            "escalate-rm-rf-home",
            "escalate-drop-table",
            "escalate-curl-pipe-bash",
            "escalate-git-push-force",
            "escalate-chmod-777",
            "escalate-write-etc",
            "escalate-write-ssh",
            "escalate-write-dotenv",
        ];
        for id in &builtin_ids {
            assert!(
                rules.iter().any(|r| r.id == *id),
                "missing builtin rule: {id}"
            );
        }
    }
}