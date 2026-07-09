use std::path::PathBuf;
use std::time::Duration;

/// Context passed to every tool execution, providing workspace and
/// sandbox configuration.
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// Root directory for tool operations.
    pub workspace_root: PathBuf,
    /// Maximum duration a tool may run before it is cancelled.
    pub command_timeout: Duration,
    /// Whether the tool is permitted to make network requests.
    pub network_allowed: bool,
}

impl Default for ToolContext {
    fn default() -> Self {
        Self {
            workspace_root: std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from(".")),
            command_timeout: Duration::from_secs(300),
            network_allowed: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_tool_context() {
        let ctx = ToolContext::default();
        assert_eq!(ctx.command_timeout, Duration::from_secs(300));
        assert!(ctx.network_allowed);
        // workspace_root should be the current directory
        assert!(ctx.workspace_root.is_absolute());
    }

    #[test]
    fn test_tool_context_custom() {
        let ctx = ToolContext {
            workspace_root: PathBuf::from("/tmp/test"),
            command_timeout: Duration::from_secs(60),
            network_allowed: false,
        };
        assert_eq!(ctx.workspace_root, PathBuf::from("/tmp/test"));
        assert_eq!(ctx.command_timeout, Duration::from_secs(60));
        assert!(!ctx.network_allowed);
    }

    #[test]
    fn test_tool_context_clone() {
        let ctx = ToolContext::default();
        let cloned = ctx.clone();
        assert_eq!(ctx.workspace_root, cloned.workspace_root);
        assert_eq!(ctx.command_timeout, cloned.command_timeout);
        assert_eq!(ctx.network_allowed, cloned.network_allowed);
    }
}