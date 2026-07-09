pub mod rules;

/// Context passed to guardrail evaluation.
///
/// Carries information about the execution environment that rules may
/// consult when deciding whether an action is dangerous.
#[derive(Debug, Clone)]
pub struct GuardContext {
    /// Optional user identifier (e.g. login name).
    pub user: Option<String>,
    /// Optional working directory the action was requested from.
    pub working_directory: Option<std::path::PathBuf>,
}