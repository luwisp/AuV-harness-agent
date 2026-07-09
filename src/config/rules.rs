use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleFile {
    pub rules: Vec<String>,
}

impl RuleFile {
    pub fn from_file(path: &Path) -> Result<Self, crate::error::HarnessError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| crate::error::HarnessError::Config(format!("Cannot read rules file: {}", e)))?;
        let rules: Vec<String> = content
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect();
        Ok(RuleFile { rules })
    }

    pub fn to_system_prompt_fragment(&self) -> String {
        if self.rules.is_empty() {
            return String::new();
        }
        let mut fragment = String::from("\n\n## Rules (MUST follow):\n");
        for rule in &self.rules {
            fragment.push_str(&format!("- {}\n", rule));
        }
        fragment
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_parse_rules_as_plain_text() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "Always use async/await").unwrap();
        writeln!(f, "# This is a comment").unwrap();
        writeln!(f, "Never use unwrap()").unwrap();
        let path = f.path().to_path_buf();

        let rules = RuleFile::from_file(&path).unwrap();
        assert_eq!(rules.rules.len(), 2);
        assert!(rules.rules.contains(&"Always use async/await".to_string()));
        assert!(rules.rules.contains(&"Never use unwrap()".to_string()));
        assert!(!rules.rules.contains(&"# This is a comment".to_string()));
    }

    #[test]
    fn test_empty_rules_returns_empty_fragment() {
        let rules = RuleFile { rules: vec![] };
        assert!(rules.to_system_prompt_fragment().is_empty());
    }

    #[test]
    fn test_rules_format_as_prompt_fragment() {
        let rules = RuleFile {
            rules: vec!["Use tabs".to_string(), "No unsafe".to_string()],
        };
        let fragment = rules.to_system_prompt_fragment();
        assert!(fragment.contains("## Rules (MUST follow)"));
        assert!(fragment.contains("- Use tabs"));
        assert!(fragment.contains("- No unsafe"));
    }
}