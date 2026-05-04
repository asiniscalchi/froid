//! LLM-facing tools that wrap analyzer read services.
//!
//! Each tool exposes a stable name, a JSON schema for inputs, and a dispatch
//! method that takes JSON in and returns JSON out. The analyzer agent loop
//! looks tools up by name in [`ToolRegistry`] and invokes them with the
//! authenticated [`UserContext`] — `user_id` is never part of the tool input.

pub mod journal;
pub mod review;
pub mod signal;

use std::{collections::HashMap, error::Error, fmt, sync::Arc};

use async_trait::async_trait;
use serde_json::Value;

use super::types::{AnalyzerError, UserContext};

#[derive(Debug)]
pub enum ToolError {
    InvalidInput(String),
    Analyzer(AnalyzerError),
    UnknownTool(String),
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(message) => write!(f, "invalid input: {message}"),
            Self::Analyzer(err) => write!(f, "{err}"),
            Self::UnknownTool(name) => write!(f, "unknown tool: {name}"),
        }
    }
}

impl Error for ToolError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Analyzer(err) => Some(err),
            _ => None,
        }
    }
}

impl From<AnalyzerError> for ToolError {
    fn from(err: AnalyzerError) -> Self {
        Self::Analyzer(err)
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn input_schema(&self) -> Value;
    async fn dispatch(&self, ctx: &UserContext, args: Value) -> Result<Value, ToolError>;
}

/// Holds an ordered set of tools indexed by name.
pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
    by_name: HashMap<&'static str, usize>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: Vec::new(),
            by_name: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        let name = tool.name();
        if self.by_name.contains_key(name) {
            panic!("tool already registered: {name}");
        }
        let index = self.tools.len();
        self.tools.push(tool);
        self.by_name.insert(name, index);
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.by_name.get(name).map(|&i| &self.tools[i])
    }

    pub fn tools(&self) -> &[Arc<dyn Tool>] {
        &self.tools
    }

    pub async fn dispatch(
        &self,
        name: &str,
        ctx: &UserContext,
        args: Value,
    ) -> Result<Value, ToolError> {
        let tool = self
            .get(name)
            .ok_or_else(|| ToolError::UnknownTool(name.to_string()))?;
        tool.dispatch(ctx, args).await
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn schema_value<T: schemars::JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).expect("JSON schema must serialize")
}

pub(crate) fn deserialize_input<T: serde::de::DeserializeOwned>(
    value: Value,
) -> Result<T, ToolError> {
    serde_json::from_value(value).map_err(|e| ToolError::InvalidInput(e.to_string()))
}

pub(crate) fn serialize_output<T: serde::Serialize>(value: T) -> Result<Value, ToolError> {
    serde_json::to_value(value)
        .map_err(|e| ToolError::Analyzer(AnalyzerError::Internal(Box::new(e))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &'static str {
            "echo"
        }
        fn description(&self) -> &'static str {
            "echoes its input"
        }
        fn input_schema(&self) -> Value {
            json!({"type": "object"})
        }
        async fn dispatch(&self, _ctx: &UserContext, args: Value) -> Result<Value, ToolError> {
            Ok(args)
        }
    }

    fn ctx() -> UserContext {
        UserContext::new("u")
    }

    #[tokio::test]
    async fn registry_dispatches_by_name() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));

        let out = registry
            .dispatch("echo", &ctx(), json!({"x": 1}))
            .await
            .unwrap();

        assert_eq!(out, json!({"x": 1}));
    }

    #[tokio::test]
    async fn registry_returns_unknown_tool_error() {
        let registry = ToolRegistry::new();

        let err = registry
            .dispatch("missing", &ctx(), json!({}))
            .await
            .unwrap_err();

        assert!(matches!(err, ToolError::UnknownTool(name) if name == "missing"));
    }

    #[test]
    #[should_panic(expected = "tool already registered: echo")]
    fn registry_panics_on_duplicate_registration() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        registry.register(Arc::new(EchoTool));
    }

    #[test]
    fn tool_error_from_analyzer_error_preserves_message() {
        let analyzer_err = AnalyzerError::InvalidArgument("limit must be > 0".into());
        let tool_err: ToolError = analyzer_err.into();
        assert_eq!(tool_err.to_string(), "invalid argument: limit must be > 0");
    }

    #[test]
    fn tool_error_unknown_tool_displays_name() {
        let err = ToolError::UnknownTool("foo".into());
        assert_eq!(err.to_string(), "unknown tool: foo");
    }
}
