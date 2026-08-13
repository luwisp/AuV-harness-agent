use crate::config::HarnessConfig;
use crate::types::{Message, Role};

/// Builds the full message context for each LLM call in the agent loop.
///
/// The context is assembled from:
/// - A system prompt (configurable, or a sensible default)
/// - A tool menu listing available tools and their descriptions
/// - Project rules loaded from rule files
/// - Skill descriptions loaded from the skills directory
/// - A compact memory index from the persistent memory store
/// - The user's task
/// - The existing conversation history (previous assistant/tool messages)
pub struct ContextBuilder {
    system_prompt: String,
    tool_menu: String,
    rules_fragment: String,
    skills_fragment: String,
    memory_index: String,
}

impl ContextBuilder {
    /// Create a new builder with the given fragments.
    ///
    /// All fragments are stored as pre-formatted strings so that `build` can
    /// assemble the context quickly on every turn.
    pub fn new(
        system_prompt: String,
        tool_menu: String,
        rules_fragment: String,
        skills_fragment: String,
        memory_index: String,
    ) -> Self {
        Self {
            system_prompt,
            tool_menu,
            rules_fragment,
            skills_fragment,
            memory_index,
        }
    }

    /// Create a builder from a [`HarnessConfig`], tool menu, and memory index.
    ///
    /// The system prompt is taken from `config.agent.system_prompt` if set,
    /// otherwise a default prompt is used.  Rules are loaded from the files
    /// listed in `config.agent.rules_files`.
    pub fn from_config(
        config: &HarnessConfig,
        tool_menu: String,
        rules_fragment: String,
        skills_fragment: String,
        memory_index: String,
    ) -> Self {
        let system_prompt = config
            .agent
            .system_prompt
            .clone()
            .unwrap_or_else(|| default_system_prompt());

        Self {
            system_prompt,
            tool_menu,
            rules_fragment,
            skills_fragment,
            memory_index,
        }
    }

    /// Build the full message list for an LLM call.
    ///
    /// The returned vector contains, in chronological order:
    ///
    /// 1. A system message (combined prompt, tool menu, rules, skills, memory).
    /// 2. Existing conversation history (User, Assistant, and Tool messages
    ///    from previous turns — must include User messages to form a valid
    ///    alternating conversation).
    /// 3. The current user task as a new User message.
    ///
    /// This ordering ensures the LLM sees the conversation in the correct
    /// temporal sequence: old task → old response → new task.
    pub fn build(&self, messages: &[Message], user_task: &str) -> Vec<Message> {
        let mut result = Vec::new();

        // 1. System message: combine all context fragments
        let mut system_content = self.system_prompt.clone();

        if !self.tool_menu.is_empty() {
            system_content.push_str("\n\n## Available Tools\n");
            system_content.push_str(&self.tool_menu);
        }

        if !self.rules_fragment.is_empty() {
            system_content.push_str(&self.rules_fragment);
        }

        if !self.skills_fragment.is_empty() {
            system_content.push_str("\n\n## Available Skills\n");
            system_content.push_str(&self.skills_fragment);
        }

        if !self.memory_index.is_empty() {
            system_content.push_str("\n\n## Memory Index\n");
            system_content.push_str(&self.memory_index);
        }

        result.push(Message {
            role: Role::System,
            content: system_content,
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
        });

        // 2. Existing conversation history (in chronological order)
        result.extend_from_slice(messages);

        // 3. Current user task (always last — the newest message)
        result.push(Message {
            role: Role::User,
            content: user_task.to_string(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
        });

        result
    }
}

/// 默认系统提示词（角色说明文件未配置时使用）。
pub fn default_system_prompt() -> String {
    r#"You are AuV harness agent, an AI coding assistant running inside AuV that helps users with software development tasks.

You have access to a set of tools that you can use to read, write, and edit files,
run shell commands, search the codebase, and execute tests.

When you are done with the task, provide your final answer by starting your
response with "FINAL ANSWER:" followed by a clear summary of what you did.

Always think step by step.  Before making changes, read relevant files first.
After making changes, run tests to verify your work."#
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_builder() -> ContextBuilder {
        ContextBuilder::new(
            "You are a helpful assistant.".to_string(),
            "- bash: run shell commands\n- read_file: read a file".to_string(),
            "\n\n## Rules (MUST follow):\n- Always use async/await\n".to_string(),
            "\n\n## Skills\n- rust-style — Rust coding style guide\n".to_string(),
            "fix-bug-123 — Fixed login bug in auth.rs\n".to_string(),
        )
    }

    #[test]
    fn test_build_creates_system_message_with_all_fragments() {
        let builder = make_builder();
        let messages = builder.build(&[], "Write a hello world program");

        assert!(!messages.is_empty());

        let system_msg = &messages[0];
        assert_eq!(system_msg.role, Role::System);
        assert!(system_msg.content.contains("You are a helpful assistant"));
        assert!(system_msg.content.contains("## Available Tools"));
        assert!(system_msg.content.contains("- bash: run shell commands"));
        assert!(system_msg.content.contains("## Rules (MUST follow)"));
        assert!(system_msg.content.contains("Always use async/await"));
        assert!(system_msg.content.contains("## Memory Index"));
        assert!(system_msg.content.contains("rust-style"));
    }

    #[test]
    fn test_build_includes_user_task() {
        let builder = make_builder();
        let messages = builder.build(&[], "Fix the bug in login");

        assert!(messages.len() >= 2);
        let user_msg = &messages[1];
        assert_eq!(user_msg.role, Role::User);
        assert_eq!(user_msg.content, "Fix the bug in login");
    }

    #[test]
    fn test_build_preserves_existing_messages() {
        let builder = make_builder();
        let existing = vec![
            Message {
                role: Role::User,
                content: "Read the main file".to_string(),
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: None,
            },
            Message {
                role: Role::Assistant,
                content: "I'll read the file first.".to_string(),
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: None,
            },
            Message {
                role: Role::Tool,
                content: "File contents here...".to_string(),
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: Some("call-1".to_string()),
            },
        ];

        let messages = builder.build(&existing, "Continue");

        // System + 3 existing (User, Assistant, Tool) + User("Continue") = 5
        assert_eq!(messages.len(), 5);
        // messages[0] = System
        assert_eq!(messages[0].role, Role::System);
        // messages[1] = User from history
        assert_eq!(messages[1].role, Role::User);
        assert_eq!(messages[1].content, "Read the main file");
        // messages[2] = Assistant from history
        assert_eq!(messages[2].role, Role::Assistant);
        assert_eq!(messages[2].content, "I'll read the file first.");
        // messages[3] = Tool from history
        assert_eq!(messages[3].role, Role::Tool);
        assert_eq!(messages[3].content, "File contents here...");
        // messages[4] = User (new task)
        assert_eq!(messages[4].role, Role::User);
        assert_eq!(messages[4].content, "Continue");
    }

    #[test]
    fn test_build_with_empty_fragments() {
        let builder = ContextBuilder::new(
            "Minimal system prompt".to_string(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        );

        let messages = builder.build(&[], "hello");

        let system_msg = &messages[0];
        assert_eq!(system_msg.role, Role::System);
        assert_eq!(system_msg.content, "Minimal system prompt");
        // No tool menu, rules, or memory sections
        assert!(!system_msg.content.contains("## Available Tools"));
        assert!(!system_msg.content.contains("## Rules"));
        assert!(!system_msg.content.contains("## Memory Index"));
    }

    #[test]
    fn test_default_system_prompt_is_non_empty() {
        let prompt = super::default_system_prompt();
        assert!(!prompt.is_empty());
        assert!(prompt.contains("AuV harness agent"));
        assert!(prompt.contains("running inside AuV"));
        assert!(prompt.contains("FINAL ANSWER"));
    }

    #[test]
    fn test_from_config_uses_custom_system_prompt() {
        let mut config = HarnessConfig::default();
        config.agent.system_prompt = Some("Custom agent prompt".to_string());

        let builder = ContextBuilder::from_config(
            &config,
            "tool menu".to_string(),
            "rules".to_string(),
            String::new(),
            "memory".to_string(),
        );

        let messages = builder.build(&[], "task");
        assert!(messages[0].content.contains("Custom agent prompt"));
    }

    #[test]
    fn test_from_config_falls_back_to_default() {
        let config = HarnessConfig::default();
        let builder = ContextBuilder::from_config(
            &config,
            "tool menu".to_string(),
            String::new(),
            String::new(),
            String::new(),
        );

        let messages = builder.build(&[], "task");
        assert!(messages[0].content.contains("AuV harness agent"));
    }
}