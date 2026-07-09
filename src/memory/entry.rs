use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Categorizes a memory entry by its origin or purpose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryType {
    /// Information explicitly provided by the user.
    User,
    /// Feedback captured from tool execution or guardrail results.
    Feedback,
    /// Project-level conventions, architecture decisions, or constraints.
    Project,
    /// Reference material such as API docs or library usage notes.
    Reference,
}

impl MemoryType {
    /// Return the lowercase string representation used in serialized frontmatter.
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryType::User => "user",
            MemoryType::Feedback => "feedback",
            MemoryType::Project => "project",
            MemoryType::Reference => "reference",
        }
    }

    /// Parse a lowercase string back into a [`MemoryType`].
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "user" => Some(MemoryType::User),
            "feedback" => Some(MemoryType::Feedback),
            "project" => Some(MemoryType::Project),
            "reference" => Some(MemoryType::Reference),
            _ => None,
        }
    }
}

impl Serialize for MemoryType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for MemoryType {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        MemoryType::from_str(&s)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown memory type: {}", s)))
    }
}

/// Metadata attached to every memory entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryMetadata {
    /// The category of this memory.
    #[serde(rename = "type")]
    pub mem_type: MemoryType,
    /// Optional tags for filtering and search.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

/// A single entry in the agent's persistent memory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryEntry {
    /// Kebab-case slug that uniquely identifies this memory.
    pub name: String,
    /// One-line summary shown in the index and compact listings.
    pub description: String,
    /// Path to the `.md` file, relative to the memory store root.
    pub file_path: PathBuf,
    /// Categorization metadata.
    pub metadata: MemoryMetadata,
}