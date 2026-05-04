use std::{env, error::Error, fmt, fs, path::PathBuf, pin::Pin, sync::Arc};

use async_trait::async_trait;
use rig::{
    agent::Agent,
    client::CompletionClient,
    completion::{Prompt, ToolDefinition},
    providers::openai::Client as OpenAiClient,
    tool::{ToolDyn as RigToolDyn, ToolError as RigToolError},
};

use super::tools::{Tool, ToolRegistry};
use super::types::UserContext;

pub const DEFAULT_ANALYZER_MODEL: &str = rig::providers::openai::completion::GPT_5_MINI;
pub const DEFAULT_ANALYZER_PREAMBLE_PATH: &str = "prompts/analyzer_v1.md";
pub const DEFAULT_ANALYZER_PREAMBLE_VERSION: &str = "analyzer-v1";
pub const DEFAULT_ANALYZER_MAX_TURNS: usize = 6;

#[async_trait]
pub trait AnalyzerAgent: Send + Sync {
    /// Answer the user's question, calling read-only tools as needed.
    async fn ask(&self, ctx: UserContext, message: String) -> Result<String, AnalyzerAgentError>;
}

#[derive(Debug)]
pub enum AnalyzerAgentError {
    MissingApiKey,
    Configuration(String),
    Llm(String),
}

impl fmt::Display for AnalyzerAgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingApiKey => write!(f, "OPENAI_API_KEY is required"),
            Self::Configuration(message) => write!(f, "{message}"),
            Self::Llm(message) => write!(f, "LLM call failed: {message}"),
        }
    }
}

impl Error for AnalyzerAgentError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyzerPreamble {
    pub version: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyzerPreambleConfig {
    pub path: PathBuf,
    pub version: String,
}

impl Default for AnalyzerPreambleConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from(DEFAULT_ANALYZER_PREAMBLE_PATH),
            version: DEFAULT_ANALYZER_PREAMBLE_VERSION.to_string(),
        }
    }
}

impl AnalyzerPreambleConfig {
    pub fn from_env() -> Self {
        Self::from_values(
            env::var("FROID_ANALYZER_PREAMBLE_PATH").ok(),
            env::var("FROID_ANALYZER_PREAMBLE_VERSION").ok(),
        )
    }

    pub(crate) fn from_values(path: Option<String>, version: Option<String>) -> Self {
        let defaults = Self::default();
        Self {
            path: path
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from)
                .unwrap_or(defaults.path),
            version: version
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(defaults.version),
        }
    }

    pub fn load(&self) -> Result<AnalyzerPreamble, AnalyzerAgentError> {
        let text = fs::read_to_string(&self.path).map_err(|source| {
            AnalyzerAgentError::Configuration(format!(
                "failed to load analyzer preamble from {}: {source}",
                self.path.display()
            ))
        })?;

        if text.trim().is_empty() {
            return Err(AnalyzerAgentError::Configuration(format!(
                "analyzer preamble file is empty: {}",
                self.path.display()
            )));
        }

        Ok(AnalyzerPreamble {
            version: self.version.clone(),
            text,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyzerAgentConfig {
    pub model: String,
    pub max_turns: usize,
}

impl Default for AnalyzerAgentConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_ANALYZER_MODEL.to_string(),
            max_turns: DEFAULT_ANALYZER_MAX_TURNS,
        }
    }
}

impl AnalyzerAgentConfig {
    pub fn from_env() -> Self {
        Self::from_values(
            env::var("FROID_ANALYZER_MODEL").ok(),
            env::var("FROID_ANALYZER_MAX_TURNS").ok(),
        )
    }

    pub(crate) fn from_values(model: Option<String>, max_turns: Option<String>) -> Self {
        let defaults = Self::default();
        Self {
            model: model
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(defaults.model),
            max_turns: max_turns
                .as_deref()
                .and_then(|value| value.trim().parse::<usize>().ok())
                .filter(|n| *n > 0)
                .unwrap_or(defaults.max_turns),
        }
    }
}

/// Adapter that exposes one of our analyzer [`Tool`]s as a `rig::tool::ToolDyn`,
/// with the authenticated [`UserContext`] captured at construction time.
struct AnalyzerToolAdapter {
    inner: Arc<dyn Tool>,
    user_context: UserContext,
}

impl AnalyzerToolAdapter {
    fn new(inner: Arc<dyn Tool>, user_context: UserContext) -> Self {
        Self {
            inner,
            user_context,
        }
    }
}

impl RigToolDyn for AnalyzerToolAdapter {
    fn name(&self) -> String {
        self.inner.name().to_string()
    }

    fn definition<'a>(
        &'a self,
        _prompt: String,
    ) -> Pin<Box<dyn std::future::Future<Output = ToolDefinition> + Send + 'a>> {
        let name = self.inner.name().to_string();
        let description = self.inner.description().to_string();
        let parameters = self.inner.input_schema();
        Box::pin(async move {
            ToolDefinition {
                name,
                description,
                parameters,
            }
        })
    }

    fn call<'a>(
        &'a self,
        args: String,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<String, RigToolError>> + Send + 'a>> {
        Box::pin(async move {
            let value: serde_json::Value =
                serde_json::from_str(&args).map_err(RigToolError::JsonError)?;
            let output = self
                .inner
                .dispatch(&self.user_context, value)
                .await
                .map_err(|e| RigToolError::ToolCallError(Box::new(e)))?;
            serde_json::to_string(&output).map_err(RigToolError::JsonError)
        })
    }
}

/// LLM-backed analyzer agent using rig's OpenAI client + multi-turn tool loop.
pub struct RigOpenAiAnalyzerAgent {
    client: OpenAiClient,
    config: AnalyzerAgentConfig,
    preamble: AnalyzerPreamble,
    tools: Arc<ToolRegistry>,
}

impl RigOpenAiAnalyzerAgent {
    pub fn new(
        client: OpenAiClient,
        config: AnalyzerAgentConfig,
        preamble: AnalyzerPreamble,
        tools: Arc<ToolRegistry>,
    ) -> Self {
        Self {
            client,
            config,
            preamble,
            tools,
        }
    }

    pub fn from_env(tools: Arc<ToolRegistry>) -> Result<Self, AnalyzerAgentError> {
        let api_key = env::var("OPENAI_API_KEY").map_err(|_| AnalyzerAgentError::MissingApiKey)?;
        let client = OpenAiClient::new(&api_key)
            .map_err(|e| AnalyzerAgentError::Configuration(e.to_string()))?;
        let config = AnalyzerAgentConfig::from_env();
        let preamble = AnalyzerPreambleConfig::from_env().load()?;
        Ok(Self::new(client, config, preamble, tools))
    }

    fn build_agent(
        &self,
        ctx: &UserContext,
    ) -> Agent<rig::providers::openai::responses_api::ResponsesCompletionModel> {
        let adapters: Vec<Box<dyn RigToolDyn>> = self
            .tools
            .tools()
            .iter()
            .map(|tool| {
                Box::new(AnalyzerToolAdapter::new(tool.clone(), ctx.clone())) as Box<dyn RigToolDyn>
            })
            .collect();

        self.client
            .agent(&self.config.model)
            .preamble(&self.preamble.text)
            .tools(adapters)
            .build()
    }
}

#[async_trait]
impl AnalyzerAgent for RigOpenAiAnalyzerAgent {
    async fn ask(&self, ctx: UserContext, message: String) -> Result<String, AnalyzerAgentError> {
        let agent = self.build_agent(&ctx);
        agent
            .prompt(message)
            .max_turns(self.config.max_turns)
            .await
            .map_err(|e| AnalyzerAgentError::Llm(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use async_trait::async_trait;
    use serde_json::{Value, json};

    use super::super::tools::{Tool, ToolError};
    use super::super::types::AnalyzerError;
    use super::*;

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

    struct CapturingTool {
        captured: std::sync::Mutex<Option<UserContext>>,
    }

    #[async_trait]
    impl Tool for CapturingTool {
        fn name(&self) -> &'static str {
            "capture"
        }
        fn description(&self) -> &'static str {
            "captures the user context"
        }
        fn input_schema(&self) -> Value {
            json!({"type": "object"})
        }
        async fn dispatch(&self, ctx: &UserContext, _args: Value) -> Result<Value, ToolError> {
            *self.captured.lock().unwrap() = Some(ctx.clone());
            Ok(json!({"ok": true}))
        }
    }

    struct FailingTool;

    #[async_trait]
    impl Tool for FailingTool {
        fn name(&self) -> &'static str {
            "fail"
        }
        fn description(&self) -> &'static str {
            "always fails"
        }
        fn input_schema(&self) -> Value {
            json!({"type": "object"})
        }
        async fn dispatch(&self, _ctx: &UserContext, _args: Value) -> Result<Value, ToolError> {
            Err(ToolError::Analyzer(AnalyzerError::InvalidArgument(
                "no good".into(),
            )))
        }
    }

    fn ctx() -> UserContext {
        UserContext::new("user-1")
    }

    #[tokio::test]
    async fn adapter_definition_returns_inner_tool_metadata() {
        let adapter = AnalyzerToolAdapter::new(Arc::new(EchoTool), ctx());

        let def = adapter.definition(String::new()).await;

        assert_eq!(def.name, "echo");
        assert_eq!(def.description, "echoes its input");
        assert_eq!(def.parameters, json!({"type": "object"}));
    }

    #[tokio::test]
    async fn adapter_call_dispatches_through_to_inner_tool() {
        let adapter = AnalyzerToolAdapter::new(Arc::new(EchoTool), ctx());

        let out = adapter.call(r#"{"x":1}"#.to_string()).await.unwrap();

        assert_eq!(out, r#"{"x":1}"#);
    }

    #[tokio::test]
    async fn adapter_call_passes_user_context_to_inner_tool() {
        let captured = Arc::new(CapturingTool {
            captured: std::sync::Mutex::new(None),
        });
        let adapter = AnalyzerToolAdapter::new(captured.clone(), ctx());

        let _ = adapter.call("{}".to_string()).await.unwrap();

        let captured_ctx = captured.captured.lock().unwrap().clone().unwrap();
        assert_eq!(captured_ctx.user_id, "user-1");
    }

    #[tokio::test]
    async fn adapter_call_returns_json_error_for_invalid_args() {
        let adapter = AnalyzerToolAdapter::new(Arc::new(EchoTool), ctx());

        let err = adapter.call("not json".to_string()).await.unwrap_err();

        assert!(matches!(err, RigToolError::JsonError(_)));
    }

    #[tokio::test]
    async fn adapter_call_wraps_inner_tool_error_as_tool_call_error() {
        let adapter = AnalyzerToolAdapter::new(Arc::new(FailingTool), ctx());

        let err = adapter.call("{}".to_string()).await.unwrap_err();

        assert!(matches!(err, RigToolError::ToolCallError(_)));
        let message = err.to_string();
        assert!(message.contains("no good"), "got: {message}");
    }

    #[test]
    fn agent_config_uses_defaults() {
        let cfg = AnalyzerAgentConfig::from_values(None, None);
        assert_eq!(cfg.model, DEFAULT_ANALYZER_MODEL);
        assert_eq!(cfg.max_turns, DEFAULT_ANALYZER_MAX_TURNS);
    }

    #[test]
    fn agent_config_accepts_overrides() {
        let cfg =
            AnalyzerAgentConfig::from_values(Some("gpt-test".to_string()), Some("3".to_string()));
        assert_eq!(cfg.model, "gpt-test");
        assert_eq!(cfg.max_turns, 3);
    }

    #[test]
    fn agent_config_rejects_zero_or_unparsable_max_turns() {
        let cfg = AnalyzerAgentConfig::from_values(None, Some("0".to_string()));
        assert_eq!(cfg.max_turns, DEFAULT_ANALYZER_MAX_TURNS);
        let cfg = AnalyzerAgentConfig::from_values(None, Some("abc".to_string()));
        assert_eq!(cfg.max_turns, DEFAULT_ANALYZER_MAX_TURNS);
    }

    #[test]
    fn agent_config_ignores_blank_overrides() {
        let cfg = AnalyzerAgentConfig::from_values(Some("   ".to_string()), Some("".to_string()));
        assert_eq!(cfg.model, DEFAULT_ANALYZER_MODEL);
        assert_eq!(cfg.max_turns, DEFAULT_ANALYZER_MAX_TURNS);
    }

    #[test]
    fn preamble_config_uses_defaults() {
        let cfg = AnalyzerPreambleConfig::from_values(None, None);
        assert_eq!(cfg.path, PathBuf::from(DEFAULT_ANALYZER_PREAMBLE_PATH));
        assert_eq!(cfg.version, DEFAULT_ANALYZER_PREAMBLE_VERSION);
    }

    #[test]
    fn preamble_config_accepts_overrides() {
        let cfg = AnalyzerPreambleConfig::from_values(
            Some("custom.md".to_string()),
            Some("custom-v2".to_string()),
        );
        assert_eq!(cfg.path, PathBuf::from("custom.md"));
        assert_eq!(cfg.version, "custom-v2");
    }

    #[test]
    fn preamble_loads_file_contents() {
        let path = temp_path("analyzer-preamble-load");
        fs::write(&path, "be helpful").unwrap();

        let preamble = AnalyzerPreambleConfig {
            path: path.clone(),
            version: "v1".to_string(),
        }
        .load()
        .unwrap();

        assert_eq!(preamble.text, "be helpful");
        assert_eq!(preamble.version, "v1");

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn preamble_load_rejects_missing_file() {
        let path = temp_path("analyzer-preamble-missing");
        let err = AnalyzerPreambleConfig {
            path: path.clone(),
            version: "v1".to_string(),
        }
        .load()
        .unwrap_err();
        assert!(matches!(err, AnalyzerAgentError::Configuration(_)));
        assert!(err.to_string().contains(path.to_str().unwrap()));
    }

    #[test]
    fn preamble_load_rejects_empty_file() {
        let path = temp_path("analyzer-preamble-empty");
        fs::write(&path, "  \n  ").unwrap();
        let err = AnalyzerPreambleConfig {
            path: path.clone(),
            version: "v1".to_string(),
        }
        .load()
        .unwrap_err();
        assert!(matches!(err, AnalyzerAgentError::Configuration(_)));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn shipped_analyzer_preamble_loads_successfully() {
        let preamble = AnalyzerPreambleConfig::default()
            .load()
            .expect("default preamble file should load");
        assert!(!preamble.text.trim().is_empty());
        assert_eq!(preamble.version, DEFAULT_ANALYZER_PREAMBLE_VERSION);
    }

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "froid-{name}-{}.md",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
}
