use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDef {
    pub name: String,
    pub description: String,
    pub file_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct SkillIndex {
    pub skills: Vec<SkillDef>,
}

impl SkillIndex {
    pub fn from_dir(dir: &Path) -> Result<Self, crate::error::HarnessError> {
        let mut skills = Vec::new();
        if !dir.exists() {
            return Ok(Self { skills });
        }
        for entry in std::fs::read_dir(dir)
            .map_err(|e| crate::error::HarnessError::Config(format!("Cannot read skills dir: {}", e)))? {
            let entry = entry.map_err(|e| crate::error::HarnessError::Config(format!("Cannot read entry: {}", e)))?;
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "md") {
                let name = path.file_stem().unwrap().to_string_lossy().to_string();
                let content = std::fs::read_to_string(&path).unwrap_or_default();
                let description = extract_description(&content);
                skills.push(SkillDef {
                    name,
                    description,
                    file_path: path,
                });
            }
        }
        Ok(Self { skills })
    }

    pub fn to_prompt_fragment(&self) -> String {
        if self.skills.is_empty() {
            return String::new();
        }
        let mut fragment = String::from("\n\n## Available Skills:\n");
        for s in &self.skills {
            fragment.push_str(&format!("- **{}**: {}\n", s.name, s.description));
        }
        fragment
    }
}

fn extract_description(content: &str) -> String {
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(desc) = trimmed.strip_prefix("description:") {
            return desc.trim().to_string();
        }
    }
    "No description".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_skill_index_from_dir() {
        let dir = tempfile::tempdir().unwrap();
        let skill_path = dir.path().join("deploy.md");
        let mut f = std::fs::File::create(&skill_path).unwrap();
        writeln!(f, "---").unwrap();
        writeln!(f, "description: Deploy the app to production").unwrap();
        writeln!(f, "---").unwrap();
        writeln!(f, "# Deploy Skill").unwrap();
        drop(f);

        let index = SkillIndex::from_dir(dir.path()).unwrap();
        assert_eq!(index.skills.len(), 1);
        assert_eq!(index.skills[0].name, "deploy");
        assert_eq!(index.skills[0].description, "Deploy the app to production");
    }

    #[test]
    fn test_skill_index_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let index = SkillIndex::from_dir(dir.path()).unwrap();
        assert!(index.skills.is_empty());
        assert!(index.to_prompt_fragment().is_empty());
    }

    #[test]
    fn test_skill_index_to_prompt_fragment() {
        let index = SkillIndex {
            skills: vec![SkillDef {
                name: "test".to_string(),
                description: "Run tests".to_string(),
                file_path: PathBuf::from("test.md"),
            }],
        };
        let fragment = index.to_prompt_fragment();
        assert!(fragment.contains("## Available Skills"));
        assert!(fragment.contains("**test**"));
        assert!(fragment.contains("Run tests"));
    }
}