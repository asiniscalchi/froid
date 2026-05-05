//! Bridges the analyzer [`ToolRegistry`] to the MCP `Streamable HTTP` server.
//!
//! Each registered analyzer tool surfaces as an MCP tool of the same name.
//! Every incoming MCP request is scoped to a single, fixed `UserContext`
//! configured at startup — there is no per-request authentication here.

use std::{borrow::Cow, sync::Arc};

use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResult, ErrorCode, Implementation, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
    },
    service::RequestContext,
};

use crate::journal::analyzer::{
    UserContext,
    tools::{ToolError, ToolRegistry},
    types::AnalyzerError,
};

/// `ServerHandler` that serves the analyzer tool registry over MCP.
#[derive(Clone)]
pub struct AnalyzerMcpServer {
    registry: Arc<ToolRegistry>,
    user: UserContext,
    server_info: ServerInfo,
    tools: Arc<[Tool]>,
}

impl AnalyzerMcpServer {
    pub fn new(registry: Arc<ToolRegistry>, user: UserContext) -> Self {
        let tools: Arc<[Tool]> = registry
            .tools()
            .iter()
            .map(|tool| {
                let schema = tool.input_schema();
                let schema_object = match schema {
                    serde_json::Value::Object(map) => map,
                    other => {
                        let mut map = serde_json::Map::new();
                        map.insert("schema".to_string(), other);
                        map
                    }
                };
                Tool::new(
                    Cow::Borrowed(tool.name()),
                    Cow::Borrowed(tool.description()),
                    Arc::new(schema_object),
                )
            })
            .collect::<Vec<_>>()
            .into();

        let server_info = ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                env!("CARGO_PKG_NAME"),
                crate::version::VERSION,
            ));

        Self {
            registry,
            user,
            server_info,
            tools,
        }
    }
}

impl ServerHandler for AnalyzerMcpServer {
    fn get_info(&self) -> ServerInfo {
        self.server_info.clone()
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(
            self.tools.iter().cloned().collect(),
        ))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let arguments = request
            .arguments
            .map(serde_json::Value::Object)
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        match self
            .registry
            .dispatch(request.name.as_ref(), &self.user, arguments)
            .await
        {
            Ok(value) => Ok(CallToolResult::structured(value)),
            Err(ToolError::UnknownTool(name)) => Err(McpError::new(
                ErrorCode::METHOD_NOT_FOUND,
                format!("unknown tool: {name}"),
                None,
            )),
            Err(ToolError::InvalidInput(message)) => {
                Err(McpError::invalid_params(message, None))
            }
            Err(ToolError::Analyzer(AnalyzerError::InvalidArgument(message))) => {
                Err(McpError::invalid_params(message, None))
            }
            Err(ToolError::Analyzer(AnalyzerError::LimitTooLarge { max })) => Err(
                McpError::invalid_params(format!("limit exceeds maximum (max {max})"), None),
            ),
            Err(ToolError::Analyzer(AnalyzerError::Internal(source))) => Err(
                McpError::internal_error(format!("internal error: {source}"), None),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use serde_json::{Value, json};

    use super::*;
    use crate::journal::analyzer::tools::Tool as AnalyzerTool;

    struct EchoTool {
        name: &'static str,
    }

    #[async_trait]
    impl AnalyzerTool for EchoTool {
        fn name(&self) -> &'static str {
            self.name
        }
        fn description(&self) -> &'static str {
            "echoes its arguments and the user_id"
        }
        fn input_schema(&self) -> Value {
            json!({"type": "object", "properties": {"x": {"type": "integer"}}})
        }
        async fn dispatch(
            &self,
            ctx: &UserContext,
            args: Value,
        ) -> Result<Value, ToolError> {
            Ok(json!({"user": ctx.user_id, "args": args}))
        }
    }

    struct FailingTool;

    #[async_trait]
    impl AnalyzerTool for FailingTool {
        fn name(&self) -> &'static str {
            "failing"
        }
        fn description(&self) -> &'static str {
            "always returns InvalidArgument"
        }
        fn input_schema(&self) -> Value {
            json!({"type": "object"})
        }
        async fn dispatch(
            &self,
            _ctx: &UserContext,
            _args: Value,
        ) -> Result<Value, ToolError> {
            Err(ToolError::Analyzer(AnalyzerError::InvalidArgument(
                "limit must be > 0".into(),
            )))
        }
    }

    fn registry_with(tools: Vec<Arc<dyn AnalyzerTool>>) -> Arc<ToolRegistry> {
        let mut registry = ToolRegistry::new();
        for tool in tools {
            registry.register(tool);
        }
        Arc::new(registry)
    }

    fn server() -> AnalyzerMcpServer {
        AnalyzerMcpServer::new(
            registry_with(vec![
                Arc::new(EchoTool { name: "echo" }),
                Arc::new(FailingTool),
            ]),
            UserContext::new("u-123"),
        )
    }

    #[test]
    fn list_tools_exposes_registered_metadata() {
        let server = server();
        let names: Vec<_> = server.tools.iter().map(|t| t.name.as_ref()).collect();
        assert_eq!(names, vec!["echo", "failing"]);

        let echo = server
            .tools
            .iter()
            .find(|t| t.name == "echo")
            .expect("echo tool present");
        assert_eq!(
            echo.description.as_deref(),
            Some("echoes its arguments and the user_id")
        );
        let schema = serde_json::Value::Object((*echo.input_schema).clone());
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["x"].is_object());
    }

    #[test]
    fn server_info_advertises_tools_capability() {
        let server = server();
        let info = server.get_info();
        assert!(info.capabilities.tools.is_some());
        assert_eq!(info.server_info.name, env!("CARGO_PKG_NAME"));
        assert_eq!(info.server_info.version, crate::version::VERSION);
    }

    #[tokio::test]
    async fn dispatch_routes_to_registry_with_fixed_user() {
        let server = server();
        let mut args = serde_json::Map::new();
        args.insert("x".to_string(), json!(7));
        let result = server
            .registry
            .dispatch("echo", &server.user, Value::Object(args.clone()))
            .await
            .expect("echo dispatch ok");

        assert_eq!(result["user"], "u-123");
        assert_eq!(result["args"]["x"], 7);
    }

    #[tokio::test]
    async fn dispatch_unknown_tool_returns_error() {
        let server = server();
        let err = server
            .registry
            .dispatch("missing", &server.user, json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::UnknownTool(name) if name == "missing"));
    }

    #[tokio::test]
    async fn dispatch_propagates_analyzer_invalid_argument() {
        let server = server();
        let err = server
            .registry
            .dispatch("failing", &server.user, json!({}))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ToolError::Analyzer(AnalyzerError::InvalidArgument(_))
        ));
    }
}
