pub mod entry;

use crate::error::HarnessError;
use entry::{MemoryEntry, MemoryMetadata, MemoryType};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The name of the index file that tracks all memory entries.
const INDEX_FILENAME: &str = "MEMORY.md";

/// Persistent file-level memory store for the agent.
///
/// Each memory entry is stored as a `.md` file with YAML-like frontmatter.
/// The [`MemoryStore`] maintains an in-memory index and a `MEMORY.md` file
/// that lists all entries for quick reference.
pub struct MemoryStore {
    /// Root directory where memory files and the index live.
    root: PathBuf,
    /// In-memory index of all known memory entries.
    index: Vec<MemoryEntry>,
}

impl MemoryStore {
    /// Create a new [`MemoryStore`] rooted at `root`.
    ///
    /// The directory is created if it does not exist.  Call [`load_all`](Self::load_all)
    /// afterwards to populate the in-memory index from the on-disk `MEMORY.md`.
    pub fn new(root: PathBuf) -> Result<Self, HarnessError> {
        fs::create_dir_all(&root)?;
        Ok(Self {
            root,
            index: Vec::new(),
        })
    }

    /// Load all memory entries from the `MEMORY.md` index file.
    ///
    /// If `MEMORY.md` does not exist, the index remains empty (no error).
    /// Each line is expected to follow the format:
    ///
    /// ```markdown
    /// - [Title](file.md) — description
    /// ```
    ///
    /// The referenced `.md` files are read and their frontmatter is parsed to
    /// reconstruct the [`MemoryEntry`].
    pub fn load_all(&mut self) -> Result<(), HarnessError> {
        let index_path = self.root.join(INDEX_FILENAME);

        if !index_path.exists() {
            self.index.clear();
            return Ok(());
        }

        let contents = fs::read_to_string(&index_path)?;
        let mut entries = Vec::new();

        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || !line.starts_with("- [") {
                continue;
            }

            // Parse: "- [Title](file.md) — description"
            if let Some(entry) = Self::parse_index_line(line) {
                let file_path = self.root.join(&entry.file_path);
                if file_path.exists() {
                    if let Some(parsed) = Self::read_entry_file(&file_path) {
                        entries.push(parsed);
                    }
                }
            }
        }

        self.index = entries;
        Ok(())
    }

    /// Search the in-memory index for entries whose name or description
    /// contain all of the given whitespace-separated keywords (case-insensitive).
    ///
    /// Returns references to matching entries in insertion order.
    pub fn search(&self, query: &str) -> Vec<&MemoryEntry> {
        let terms: Vec<String> = query
            .split_whitespace()
            .map(|t| t.to_lowercase())
            .collect();

        if terms.is_empty() {
            return Vec::new();
        }

        self.index
            .iter()
            .filter(|entry| {
                let haystack = format!(
                    "{} {}",
                    entry.name.to_lowercase(),
                    entry.description.to_lowercase()
                );
                terms.iter().all(|term| haystack.contains(term.as_str()))
            })
            .collect()
    }

    /// Write a new memory entry to disk.
    ///
    /// Creates a `.md` file with YAML-style frontmatter containing the entry
    /// metadata, followed by the user-supplied `content`.  The `MEMORY.md`
    /// index is updated, and the entry is appended to the in-memory index.
    ///
    /// The `entry.file_path` is treated as relative to the store root.
    pub fn write(&mut self, entry: MemoryEntry, content: &str) -> Result<(), HarnessError> {
        // 1. Write the .md file with frontmatter
        let file_path = self.root.join(&entry.file_path);

        // Ensure parent directory exists
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let file_content = Self::format_entry_file(&entry, content);
        fs::write(&file_path, &file_content)?;

        // 2. Update MEMORY.md index
        let index_path = self.root.join(INDEX_FILENAME);
        let index_line = format!(
            "- [{}]({}) — {}\n",
            entry.name,
            entry.file_path.display(),
            entry.description
        );

        let mut index_file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&index_path)?;
        index_file.write_all(index_line.as_bytes())?;

        // 3. Add to in-memory index
        self.index.push(entry);

        Ok(())
    }

    /// Generate a compact "memory directory" text suitable for injection into
    /// an LLM system prompt.
    ///
    /// Each entry is rendered as one line: `name — description`.
    pub fn compact_index(&self) -> String {
        if self.index.is_empty() {
            return "(no memories recorded)\n".to_string();
        }

        let mut out = String::new();
        for entry in &self.index {
            out.push_str(&format!("{} — {}\n", entry.name, entry.description));
        }
        out
    }

    /// Return the number of entries in the in-memory index.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// Return true when the index is empty.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    // -----------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------

    /// Parse a single line of the MEMORY.md index format.
    ///
    /// Expected format: `- [Title](file.md) — description`
    fn parse_index_line(line: &str) -> Option<MemoryEntry> {
        // Strip the leading "- [" and split on "]("
        let after_bracket = line.strip_prefix("- [")?;

        let (name, rest) = after_bracket.split_once("](")?;

        // rest should be "file.md) — description"
        let (file_part, description) = rest.split_once(") — ")?;

        Some(MemoryEntry {
            name: name.to_string(),
            description: description.to_string(),
            file_path: PathBuf::from(file_part),
            metadata: MemoryMetadata {
                mem_type: MemoryType::Reference,
                tags: Vec::new(),
            },
        })
    }

    /// Read a `.md` file and parse its YAML-style frontmatter into a [`MemoryEntry`].
    fn read_entry_file(path: &Path) -> Option<MemoryEntry> {
        let contents = fs::read_to_string(path).ok()?;

        // Extract frontmatter between the first two "---" lines
        let mut lines = contents.lines();
        if lines.next()?.trim() != "---" {
            return None;
        }

        let mut frontmatter_lines = Vec::new();
        for line in lines.by_ref() {
            if line.trim() == "---" {
                break;
            }
            frontmatter_lines.push(line);
        }

        Self::parse_frontmatter(&frontmatter_lines, path)
    }

    /// Parse the frontmatter lines into a [`MemoryEntry`].
    fn parse_frontmatter(lines: &[&str], file_path: &Path) -> Option<MemoryEntry> {
        let mut name: Option<String> = None;
        let mut description: Option<String> = None;
        let mut mem_type: Option<MemoryType> = None;
        let mut tags: Vec<String> = Vec::new();
        let mut in_metadata = false;

        for line in lines {
            let trimmed = line.trim();

            if trimmed.is_empty() {
                continue;
            }

            if trimmed == "metadata:" {
                in_metadata = true;
                continue;
            }

            if in_metadata {
                if let Some(value) = trimmed.strip_prefix("type:") {
                    mem_type = MemoryType::from_str(value.trim());
                } else if let Some(value) = trimmed.strip_prefix("tags:") {
                    // Parse inline YAML list: tags: [tag1, tag2]
                    let value = value.trim();
                    if let Some(list_str) = value
                        .strip_prefix('[')
                        .and_then(|s| s.strip_suffix(']'))
                    {
                        tags = list_str
                            .split(',')
                            .map(|t| t.trim().trim_matches('"').to_string())
                            .filter(|t| !t.is_empty())
                            .collect();
                    }
                }
                // If the line does not have leading whitespace, we've left
                // the metadata block.
                if !line.starts_with(' ') {
                    in_metadata = false;
                }
            } else {
                if let Some(value) = trimmed.strip_prefix("name:") {
                    name = Some(value.trim().to_string());
                } else if let Some(value) = trimmed.strip_prefix("description:") {
                    description = Some(value.trim().to_string());
                }
            }
        }

        Some(MemoryEntry {
            name: name?,
            description: description?,
            file_path: file_path
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("unknown.md")),
            metadata: MemoryMetadata {
                mem_type: mem_type.unwrap_or(MemoryType::Reference),
                tags,
            },
        })
    }

    /// Format a [`MemoryEntry`] and its content into the full `.md` file text
    /// including YAML-style frontmatter.
    fn format_entry_file(entry: &MemoryEntry, content: &str) -> String {
        let mut out = String::new();
        out.push_str("---\n");
        out.push_str(&format!("name: {}\n", entry.name));
        out.push_str(&format!("description: {}\n", entry.description));
        out.push_str("metadata:\n");
        out.push_str(&format!("  type: {}\n", entry.metadata.mem_type.as_str()));
        if !entry.metadata.tags.is_empty() {
            let tag_list = entry
                .metadata
                .tags
                .iter()
                .map(|t| format!("\"{}\"", t))
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!("  tags: [{}]\n", tag_list));
        }
        out.push_str("---\n");
        out.push_str(content);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Helper to create a test entry.
    fn make_entry(name: &str, description: &str, mem_type: MemoryType) -> MemoryEntry {
        MemoryEntry {
            name: name.to_string(),
            description: description.to_string(),
            file_path: PathBuf::from(format!("{}.md", name)),
            metadata: MemoryMetadata {
                mem_type,
                tags: Vec::new(),
            },
        }
    }

    /// Helper to create a MemoryStore rooted in a temp directory and load it.
    fn store_with_entries(
        entries: &[(MemoryEntry, &str)],
    ) -> (TempDir, MemoryStore) {
        let dir = TempDir::new().expect("tempdir");
        let mut store = MemoryStore::new(dir.path().to_path_buf()).expect("new store");

        for (entry, content) in entries {
            store.write(entry.clone(), content).expect("write entry");
        }

        (dir, store)
    }

    // -----------------------------------------------------------------
    // test_write_and_read_memory
    // -----------------------------------------------------------------

    #[test]
    fn test_write_and_read_memory() {
        let dir = TempDir::new().expect("tempdir");
        let mut store = MemoryStore::new(dir.path().to_path_buf()).expect("new store");

        let entry = make_entry("my-project-conventions", "Coding conventions for the project", MemoryType::Project);
        let content = "# Project Conventions\n\n- Use 4 spaces for indentation\n- Max line length: 100\n";

        store.write(entry.clone(), content).expect("write");

        // Verify the .md file exists and has frontmatter
        let file_path = dir.path().join("my-project-conventions.md");
        assert!(file_path.exists(), "memory file should exist");

        let file_contents = fs::read_to_string(&file_path).expect("read file");
        assert!(file_contents.contains("name: my-project-conventions"));
        assert!(file_contents.contains("description: Coding conventions for the project"));
        assert!(file_contents.contains("type: project"));
        assert!(file_contents.contains("# Project Conventions"));

        // Verify MEMORY.md index exists
        let index_path = dir.path().join("MEMORY.md");
        assert!(index_path.exists(), "index file should exist");
        let index_contents = fs::read_to_string(&index_path).expect("read index");
        assert!(index_contents.contains("[my-project-conventions]"));
        assert!(index_contents.contains("my-project-conventions.md"));
        assert!(index_contents.contains("Coding conventions for the project"));

        // Now reload into a fresh store
        let mut store2 = MemoryStore::new(dir.path().to_path_buf()).expect("new store2");
        store2.load_all().expect("load_all");

        assert_eq!(store2.len(), 1, "should have one entry after reload");
        let loaded = &store2.index[0];
        assert_eq!(loaded.name, "my-project-conventions");
        assert_eq!(loaded.description, "Coding conventions for the project");
        assert_eq!(loaded.metadata.mem_type, MemoryType::Project);
    }

    // -----------------------------------------------------------------
    // test_search_finds_relevant_entries
    // -----------------------------------------------------------------

    #[test]
    fn test_search_finds_relevant_entries() {
        let entries = vec![
            (make_entry("rust-style-guide", "Rust coding style and formatting rules", MemoryType::Project), "content 1"),
            (make_entry("python-notes", "Python library usage notes and tips", MemoryType::Reference), "content 2"),
            (make_entry("user-prefs", "User preferences for editor and shell", MemoryType::User), "content 3"),
        ];

        let (_dir, store) = store_with_entries(&entries);

        // Search by name
        let results = store.search("rust");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "rust-style-guide");

        // Search by description
        let results = store.search("python");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "python-notes");

        // Search with multiple keywords (AND)
        let results = store.search("style formatting");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "rust-style-guide");

        // Case-insensitive search
        let results = store.search("RUST");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "rust-style-guide");

        // No match
        let results = store.search("nonexistent");
        assert!(results.is_empty());

        // Empty query
        let results = store.search("");
        assert!(results.is_empty());
    }

    // -----------------------------------------------------------------
    // test_compact_index_format
    // -----------------------------------------------------------------

    #[test]
    fn test_compact_index_format() {
        let entries = vec![
            (make_entry("alpha", "First entry", MemoryType::User), "a"),
            (make_entry("beta", "Second entry", MemoryType::Project), "b"),
        ];

        let (_dir, store) = store_with_entries(&entries);

        let compact = store.compact_index();
        assert!(compact.contains("alpha — First entry"));
        assert!(compact.contains("beta — Second entry"));
        // Each entry on its own line
        let lines: Vec<&str> = compact.lines().collect();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_compact_index_empty() {
        let dir = TempDir::new().expect("tempdir");
        let store = MemoryStore::new(dir.path().to_path_buf()).expect("new store");

        let compact = store.compact_index();
        assert!(compact.contains("(no memories recorded)"));
    }

    // -----------------------------------------------------------------
    // test_memory_persists_across_instances
    // -----------------------------------------------------------------

    #[test]
    fn test_memory_persists_across_instances() {
        let dir = TempDir::new().expect("tempdir");

        // First instance: write some entries
        {
            let mut store = MemoryStore::new(dir.path().to_path_buf()).expect("new store");
            store
                .write(
                    make_entry("entry-one", "The first persisted entry", MemoryType::User),
                    "Content for entry one.",
                )
                .expect("write 1");
            store
                .write(
                    make_entry("entry-two", "The second persisted entry", MemoryType::Reference),
                    "Content for entry two.",
                )
                .expect("write 2");
        }

        // Second instance: load and verify
        {
            let mut store = MemoryStore::new(dir.path().to_path_buf()).expect("new store");
            store.load_all().expect("load_all");

            assert_eq!(store.len(), 2, "should have two entries after reload");

            let names: Vec<&str> = store.index.iter().map(|e| e.name.as_str()).collect();
            assert!(names.contains(&"entry-one"));
            assert!(names.contains(&"entry-two"));

            // Search should still work
            let results = store.search("persisted");
            assert_eq!(results.len(), 2);
        }

        // Third instance: verify data is still intact
        {
            let mut store = MemoryStore::new(dir.path().to_path_buf()).expect("new store");
            store.load_all().expect("load_all");
            assert_eq!(store.len(), 2);
        }
    }

    // -----------------------------------------------------------------
    // Additional tests for edge cases
    // -----------------------------------------------------------------

    #[test]
    fn test_load_all_with_no_index_file() {
        let dir = TempDir::new().expect("tempdir");
        let mut store = MemoryStore::new(dir.path().to_path_buf()).expect("new store");
        store.load_all().expect("load_all");
        assert!(store.is_empty());
    }

    #[test]
    fn test_load_all_with_empty_index() {
        let dir = TempDir::new().expect("tempdir");
        let index_path = dir.path().join("MEMORY.md");
        fs::write(&index_path, "").expect("write empty index");

        let mut store = MemoryStore::new(dir.path().to_path_buf()).expect("new store");
        store.load_all().expect("load_all");
        assert!(store.is_empty());
    }

    #[test]
    fn test_write_creates_parent_directories() {
        let dir = TempDir::new().expect("tempdir");
        let mut store = MemoryStore::new(dir.path().to_path_buf()).expect("new store");

        let entry = MemoryEntry {
            name: "nested-entry".to_string(),
            description: "Entry in a subdirectory".to_string(),
            file_path: PathBuf::from("sub/dir/nested-entry.md"),
            metadata: MemoryMetadata {
                mem_type: MemoryType::Reference,
                tags: vec!["important".to_string()],
            },
        };

        store.write(entry, "Nested content").expect("write nested");

        let file_path = dir.path().join("sub/dir/nested-entry.md");
        assert!(file_path.exists(), "nested file should exist");

        let contents = fs::read_to_string(&file_path).expect("read nested");
        assert!(contents.contains("name: nested-entry"));
        assert!(contents.contains("tags: [\"important\"]"));
    }

    #[test]
    fn test_frontmatter_roundtrip() {
        let dir = TempDir::new().expect("tempdir");
        let mut store = MemoryStore::new(dir.path().to_path_buf()).expect("new store");

        let entry = MemoryEntry {
            name: "roundtrip-test".to_string(),
            description: "Testing frontmatter serialization".to_string(),
            file_path: PathBuf::from("roundtrip-test.md"),
            metadata: MemoryMetadata {
                mem_type: MemoryType::Feedback,
                tags: vec!["test".to_string(), "roundtrip".to_string()],
            },
        };

        store.write(entry, "Body content here").expect("write");

        // Reload and verify all fields match
        let mut store2 = MemoryStore::new(dir.path().to_path_buf()).expect("new store2");
        store2.load_all().expect("load_all");

        assert_eq!(store2.len(), 1);
        let loaded = &store2.index[0];
        assert_eq!(loaded.name, "roundtrip-test");
        assert_eq!(loaded.description, "Testing frontmatter serialization");
        assert_eq!(loaded.metadata.mem_type, MemoryType::Feedback);
        assert_eq!(loaded.metadata.tags, vec!["test", "roundtrip"]);
    }

    #[test]
    fn test_memory_type_serialization() {
        // Verify MemoryType serializes to lowercase strings
        assert_eq!(MemoryType::User.as_str(), "user");
        assert_eq!(MemoryType::Feedback.as_str(), "feedback");
        assert_eq!(MemoryType::Project.as_str(), "project");
        assert_eq!(MemoryType::Reference.as_str(), "reference");

        // Verify roundtrip
        assert_eq!(MemoryType::from_str("user"), Some(MemoryType::User));
        assert_eq!(MemoryType::from_str("feedback"), Some(MemoryType::Feedback));
        assert_eq!(MemoryType::from_str("project"), Some(MemoryType::Project));
        assert_eq!(MemoryType::from_str("reference"), Some(MemoryType::Reference));
        assert_eq!(MemoryType::from_str("unknown"), None);
    }

    #[test]
    fn test_search_with_partial_word_match() {
        let entries = vec![
            (make_entry("rust-style", "Rust style guide", MemoryType::Project), "c"),
            (make_entry("rust-testing", "Testing strategies for Rust", MemoryType::Reference), "d"),
            (make_entry("python-notes", "Python notes", MemoryType::Reference), "e"),
        ];

        let (_dir, store) = store_with_entries(&entries);

        // "rust" should match both rust entries
        let results = store.search("rust");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_search_multiple_terms_and() {
        let entries = vec![
            (make_entry("rust-style", "Rust style guide", MemoryType::Project), "c"),
            (make_entry("rust-testing", "Testing strategies", MemoryType::Reference), "d"),
        ];

        let (_dir, store) = store_with_entries(&entries);

        // "rust testing" should only match rust-testing (has both terms)
        let results = store.search("rust testing");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "rust-testing");
    }
}