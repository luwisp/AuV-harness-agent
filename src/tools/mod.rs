pub mod bash;
pub mod context;
pub mod file;
pub mod git;
pub mod search;
pub mod subagent;
pub mod test_runner;

use crate::error::{HarnessError, Result};
use crate::types::{ToolInfo, ToolResult};
use context::ToolContext;
use serde_json::Value;

/// The `Tool` trait defines the interface every tool must implement.
///
/// Tools are registered in the [`ToolRegistry`] and invoked by the agent
/// loop when the LLM decides to call a tool.
pub trait Tool: Send + Sync {
    /// Unique name of the tool (e.g. "read_file", "bash").
    fn name(&self) -> &str;

    /// Human-readable description of what the tool does.
    fn description(&self) -> &str;

    /// JSON Schema describing the parameters the tool accepts.
    fn parameters(&self) -> Value;

    /// Execute the tool with the given parameters and context.
    ///
    /// # Arguments
    /// * `params` - The tool arguments as a JSON value (the content of the
    ///   `arguments` field from the LLM tool call).
    /// * `ctx` - Execution context including workspace root, timeout, and
    ///   network permissions.
    fn execute(&self, params: &Value, ctx: &ToolContext) -> Result<ToolResult>;
}

/// A registry holding all available tools and dispatching executions.
///
/// Tools are stored as trait objects so that new tool implementations can
/// be added without changing the registry.
pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl ToolRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    /// 已注册工具名列表（测试/诊断用）。
    pub fn names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name().to_string()).collect()
    }

    /// Register a tool. Returns `Ok(())` on success, or an error if a tool
    /// with the same name is already registered.
    pub fn register(&mut self, tool: Box<dyn Tool>) -> Result<()> {
        let name = tool.name().to_string();
        if self.tools.iter().any(|t| t.name() == name) {
            return Err(HarnessError::ToolExecution(format!(
                "Tool '{}' is already registered",
                name
            )));
        }
        self.tools.push(tool);
        Ok(())
    }

    /// Look up a tool by name.
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.iter().find(|t| t.name() == name).map(|t| t.as_ref())
    }

    /// Return metadata for every registered tool.
    pub fn list_tools(&self) -> Vec<ToolInfo> {
        self.tools
            .iter()
            .map(|t| ToolInfo {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.parameters(),
            })
            .collect()
    }

    /// Build the tool menu in the format expected by the OpenAI chat
    /// completions API (and compatible providers).
    ///
    /// Each entry is a JSON object with `type: "function"` and a `function`
    /// sub-object containing `name`, `description`, and `parameters`.
    pub fn generate_tool_menu(&self) -> Vec<Value> {
        self.tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name(),
                        "description": t.description(),
                        "parameters": t.parameters(),
                    }
                })
            })
            .collect()
    }

    /// Execute a tool by name, forwarding the given parameters and context.
    ///
    /// Returns `ToolNotFound` if no tool matches `name`.
    pub fn execute(
        &self,
        name: &str,
        params: &Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult> {
        let tool = self
            .get(name)
            .ok_or_else(|| HarnessError::ToolNotFound(name.to_string()))?;
        tool.execute(params, ctx)
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Artifact;
    use serde_json::json;

    // ------------------------------------------------------------------
    // Test helpers
    // ------------------------------------------------------------------

    /// A minimal tool implementation used in tests.
    struct EchoTool;

    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }

        fn description(&self) -> &str {
            "Echoes back the provided message."
        }

        fn parameters(&self) -> Value {
            json!({
                "type": "object",
                "properties": {
                    "message": {
                        "type": "string",
                        "description": "The message to echo."
                    }
                },
                "required": ["message"]
            })
        }

        fn execute(&self, params: &Value, _ctx: &ToolContext) -> Result<ToolResult> {
            let message = params["message"]
                .as_str()
                .unwrap_or("(no message)");
            Ok(ToolResult {
                success: true,
                content: message.to_string(),
                structured: Some(json!({ "echoed": message })),
                artifacts: vec![],
            })
        }
    }

    struct GreetTool;

    impl Tool for GreetTool {
        fn name(&self) -> &str {
            "greet"
        }

        fn description(&self) -> &str {
            "Greets a person by name."
        }

        fn parameters(&self) -> Value {
            json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name of the person to greet."
                    }
                },
                "required": ["name"]
            })
        }

        fn execute(&self, params: &Value, _ctx: &ToolContext) -> Result<ToolResult> {
            let name = params["name"].as_str().unwrap_or("World");
            Ok(ToolResult {
                success: true,
                content: format!("Hello, {}!", name),
                structured: None,
                artifacts: vec![],
            })
        }
    }

    struct FailingTool;

    impl Tool for FailingTool {
        fn name(&self) -> &str {
            "failing"
        }

        fn description(&self) -> &str {
            "Always fails."
        }

        fn parameters(&self) -> Value {
            json!({ "type": "object", "properties": {} })
        }

        fn execute(&self, _params: &Value, _ctx: &ToolContext) -> Result<ToolResult> {
            Err(HarnessError::ToolExecution("simulated failure".to_string()))
        }
    }

    struct ArtifactTool;

    impl Tool for ArtifactTool {
        fn name(&self) -> &str {
            "artifact"
        }

        fn description(&self) -> &str {
            "Returns artifacts."
        }

        fn parameters(&self) -> Value {
            json!({ "type": "object", "properties": {} })
        }

        fn execute(&self, _params: &Value, _ctx: &ToolContext) -> Result<ToolResult> {
            Ok(ToolResult {
                success: true,
                content: "created files".to_string(),
                structured: None,
                artifacts: vec![
                    Artifact {
                        path: std::path::PathBuf::from("/tmp/a.txt"),
                        content_type: "text/plain".to_string(),
                        size_bytes: 42,
                    },
                    Artifact {
                        path: std::path::PathBuf::from("/tmp/b.png"),
                        content_type: "image/png".to_string(),
                        size_bytes: 1024,
                    },
                ],
            })
        }
    }

    fn make_registry() -> ToolRegistry {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(EchoTool)).unwrap();
        reg.register(Box::new(GreetTool)).unwrap();
        reg.register(Box::new(FailingTool)).unwrap();
        reg
    }

    // ------------------------------------------------------------------
    // Tests
    // ------------------------------------------------------------------

    #[test]
    fn test_registry_register_and_get() {
        let reg = make_registry();
        let echo = reg.get("echo").expect("echo tool should be present");
        assert_eq!(echo.name(), "echo");
        assert_eq!(echo.description(), "Echoes back the provided message.");

        let greet = reg.get("greet").expect("greet tool should be present");
        assert_eq!(greet.name(), "greet");

        assert!(reg.get("nonexistent").is_none());
    }

    #[test]
    fn test_registry_duplicate_register() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(EchoTool)).unwrap();
        let result = reg.register(Box::new(EchoTool));
        assert!(result.is_err());
    }

    #[test]
    fn test_registry_list_tools() {
        let reg = make_registry();
        let list = reg.list_tools();
        assert_eq!(list.len(), 3);

        let names: Vec<&str> = list.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"echo"));
        assert!(names.contains(&"greet"));
        assert!(names.contains(&"failing"));

        // Verify a ToolInfo carries parameter schema
        let echo_info = list.iter().find(|t| t.name == "echo").unwrap();
        assert!(echo_info.description.contains("Echoes"));
        assert!(echo_info.parameters.is_object());
    }

    #[test]
    fn test_registry_execute() {
        let reg = make_registry();
        let ctx = ToolContext::default();

        let result = reg
            .execute("greet", &json!({ "name": "Alice" }), &ctx)
            .expect("greet should succeed");
        assert!(result.success);
        assert_eq!(result.content, "Hello, Alice!");

        // Execute a tool that returns structured data
        let result = reg
            .execute("echo", &json!({ "message": "hi" }), &ctx)
            .expect("echo should succeed");
        assert!(result.success);
        assert_eq!(result.content, "hi");
        assert_eq!(result.structured, Some(json!({ "echoed": "hi" })));
    }

    #[test]
    fn test_registry_execute_tool_not_found() {
        let reg = make_registry();
        let ctx = ToolContext::default();
        let result = reg.execute("no_such_tool", &json!({}), &ctx);
        assert!(result.is_err());
        match result {
            Err(HarnessError::ToolNotFound(name)) => {
                assert_eq!(name, "no_such_tool");
            }
            other => panic!("expected ToolNotFound, got {:?}", other),
        }
    }

    #[test]
    fn test_registry_execute_tool_error() {
        let reg = make_registry();
        let ctx = ToolContext::default();
        let result = reg.execute("failing", &json!({}), &ctx);
        assert!(result.is_err());
        match result {
            Err(HarnessError::ToolExecution(msg)) => {
                assert!(msg.contains("simulated failure"));
            }
            other => panic!("expected ToolExecution, got {:?}", other),
        }
    }

    #[test]
    fn test_generate_tool_menu() {
        let reg = make_registry();
        let menu = reg.generate_tool_menu();
        assert_eq!(menu.len(), 3);

        // Each entry must have the OpenAI function-calling shape
        for entry in &menu {
            assert_eq!(entry["type"], "function");
            let func = &entry["function"];
            assert!(func["name"].is_string());
            assert!(func["description"].is_string());
            assert!(func["parameters"].is_object());
        }

        // Spot-check a known tool
        let echo_entry = menu
            .iter()
            .find(|e| e["function"]["name"] == "echo")
            .expect("echo tool in menu");
        assert_eq!(echo_entry["function"]["description"], "Echoes back the provided message.");
    }

    #[test]
    fn test_empty_registry() {
        let reg = ToolRegistry::new();
        assert!(reg.list_tools().is_empty());
        assert!(reg.generate_tool_menu().is_empty());
        assert!(reg.get("anything").is_none());
        let ctx = ToolContext::default();
        let result = reg.execute("anything", &json!({}), &ctx);
        assert!(matches!(result, Err(HarnessError::ToolNotFound(_))));
    }

    #[test]
    fn test_registry_artifacts() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(ArtifactTool)).unwrap();

        let ctx = ToolContext::default();
        let result = reg
            .execute("artifact", &json!({}), &ctx)
            .expect("artifact tool should succeed");
        assert!(result.success);
        assert_eq!(result.artifacts.len(), 2);
        assert_eq!(result.artifacts[0].path, std::path::PathBuf::from("/tmp/a.txt"));
        assert_eq!(result.artifacts[0].content_type, "text/plain");
        assert_eq!(result.artifacts[0].size_bytes, 42);
    }

    #[test]
    fn test_registry_send_sync() {
        // ToolRegistry and Tool trait objects must be Send + Sync so they
        // can be shared across threads.
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ToolRegistry>();
    }
}