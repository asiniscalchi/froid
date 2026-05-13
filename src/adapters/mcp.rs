//! Bridges the analyzer services to the MCP `Streamable HTTP` server.
//!
//! Tools come from the analyzer [`ToolRegistry`]. Journal entries and daily /
//! weekly reviews are also exposed as MCP **resources**, addressed by URI
//! templates so a host can read a specific entry or review without going
//! through a tool call.
//!
//! Every incoming MCP request is scoped to a single, fixed `UserContext`
//! configured at startup. MCP serving is intended for local clients and the
//! CLI rejects non-loopback bind addresses when MCP is enabled.

use std::{borrow::Cow, sync::Arc};

use chrono::NaiveDate;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    model::{
        Annotated, CallToolRequestParams, CallToolResult, ErrorCode, Implementation,
        ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
        RawResourceTemplate, ReadResourceRequestParams, ReadResourceResult, ResourceContents,
        ServerCapabilities, ServerInfo, Tool,
    },
    service::RequestContext,
};

use crate::journal::analyzer::{
    AnalyzerMcpComponents, UserContext,
    journal::JournalReadService,
    review::ReviewReadService,
    tools::{ToolError, ToolRegistry},
    types::AnalyzerError,
};

/// MIME type used for every resource exposed by this server. The payload is
/// always a JSON-serialised view object (entry / review).
const RESOURCE_MIME_TYPE: &str = "application/json";

/// `ServerHandler` that serves the analyzer tool registry and read resources
/// over MCP.
#[derive(Clone)]
pub struct AnalyzerMcpServer {
    registry: Arc<ToolRegistry>,
    journal_service: Arc<dyn JournalReadService>,
    review_service: Arc<dyn ReviewReadService>,
    user: UserContext,
    server_info: ServerInfo,
    tools: Arc<[Tool]>,
    resource_templates: Arc<[rmcp::model::ResourceTemplate]>,
}

impl AnalyzerMcpServer {
    pub fn new(components: AnalyzerMcpComponents, user: UserContext) -> Self {
        let AnalyzerMcpComponents {
            registry,
            journal_service,
            review_service,
        } = components;

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

        let resource_templates: Arc<[rmcp::model::ResourceTemplate]> = build_resource_templates();

        let server_info = ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::new(
            env!("CARGO_PKG_NAME"),
            crate::version::VERSION,
        ));

        Self {
            registry,
            journal_service,
            review_service,
            user,
            server_info,
            tools,
            resource_templates,
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

        let result = self
            .registry
            .dispatch(request.name.as_ref(), &self.user, arguments)
            .await;
        map_dispatch_result(result)
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        // Concrete resource instances (one per id / date) are unbounded and
        // discovered by the host through the URI templates below; nothing to
        // enumerate eagerly.
        Ok(ListResourcesResult::with_all_items(Vec::new()))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        Ok(ListResourceTemplatesResult::with_all_items(
            self.resource_templates.iter().cloned().collect(),
        ))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        self.read_resource_by_uri(&request.uri).await
    }
}

impl AnalyzerMcpServer {
    /// Read a resource by URI. Extracted from `read_resource` so it can be
    /// unit-tested without constructing a `RequestContext` (whose constructor
    /// is `pub(crate)` in rmcp).
    async fn read_resource_by_uri(&self, uri: &str) -> Result<ReadResourceResult, McpError> {
        let parsed = parse_resource_uri(uri)
            .ok_or_else(|| McpError::invalid_params(format!("unknown uri: {uri}"), None))?;
        let payload = match parsed {
            ResourceRef::JournalEntry(id) => {
                let entry = self
                    .journal_service
                    .get_by_id(&self.user, id)
                    .await
                    .map_err(analyzer_to_mcp)?
                    .ok_or_else(|| {
                        McpError::new(
                            ErrorCode::INVALID_PARAMS,
                            format!("no journal entry with id {id}"),
                            None,
                        )
                    })?;
                serde_json::json!({
                    "id": entry.id,
                    "received_at": entry.received_at,
                    "text": entry.text,
                })
                .to_string()
            }
            ResourceRef::DailyReview(date) => {
                let review = self
                    .review_service
                    .get_daily_review(&self.user, date)
                    .await
                    .map_err(analyzer_to_mcp)?
                    .ok_or_else(|| {
                        McpError::new(
                            ErrorCode::INVALID_PARAMS,
                            format!("no completed daily review for {date}"),
                            None,
                        )
                    })?;
                serde_json::json!({
                    "review_date": review.review_date,
                    "review_text": review.review_text,
                    "created_at": review.created_at,
                })
                .to_string()
            }
            ResourceRef::WeeklyReview(week_start) => {
                let review = self
                    .review_service
                    .get_weekly_review(&self.user, week_start)
                    .await
                    .map_err(analyzer_to_mcp)?
                    .ok_or_else(|| {
                        McpError::new(
                            ErrorCode::INVALID_PARAMS,
                            format!("no completed weekly review for week starting {week_start}"),
                            None,
                        )
                    })?;
                serde_json::json!({
                    "week_start": review.week_start,
                    "week_end": review.week_end,
                    "review_text": review.review_text,
                    "created_at": review.created_at,
                })
                .to_string()
            }
        };

        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(payload, uri.to_string()).with_mime_type(RESOURCE_MIME_TYPE),
        ]))
    }
}

fn build_resource_templates() -> Arc<[rmcp::model::ResourceTemplate]> {
    let templates = vec![
        Annotated::new(
            RawResourceTemplate::new("journal://entry/{id}", "journal_entry")
                .with_title("Journal entry by id")
                .with_description(
                    "Read a single journal entry by its numeric id (e.g. journal://entry/42).",
                )
                .with_mime_type(RESOURCE_MIME_TYPE),
            None,
        ),
        Annotated::new(
            RawResourceTemplate::new("review://daily/{date}", "daily_review")
                .with_title("Daily review by date")
                .with_description(
                    "Read the completed daily review for a YYYY-MM-DD date (e.g. review://daily/2026-04-28).",
                )
                .with_mime_type(RESOURCE_MIME_TYPE),
            None,
        ),
        Annotated::new(
            RawResourceTemplate::new("review://weekly/{week_start}", "weekly_review")
                .with_title("Weekly review by week start")
                .with_description(
                    "Read the completed weekly review whose week starts on a YYYY-MM-DD date (e.g. review://weekly/2026-04-20).",
                )
                .with_mime_type(RESOURCE_MIME_TYPE),
            None,
        ),
    ];
    Arc::from(templates)
}

/// Parsed reference to one of the analyzer-backed resources.
#[derive(Debug, PartialEq, Eq)]
enum ResourceRef {
    JournalEntry(i64),
    DailyReview(NaiveDate),
    WeeklyReview(NaiveDate),
}

fn parse_resource_uri(uri: &str) -> Option<ResourceRef> {
    if let Some(rest) = uri.strip_prefix("journal://entry/") {
        return rest.parse::<i64>().ok().map(ResourceRef::JournalEntry);
    }
    if let Some(rest) = uri.strip_prefix("review://daily/") {
        return NaiveDate::parse_from_str(rest, "%Y-%m-%d")
            .ok()
            .map(ResourceRef::DailyReview);
    }
    if let Some(rest) = uri.strip_prefix("review://weekly/") {
        return NaiveDate::parse_from_str(rest, "%Y-%m-%d")
            .ok()
            .map(ResourceRef::WeeklyReview);
    }
    None
}

fn analyzer_to_mcp(err: AnalyzerError) -> McpError {
    match err {
        AnalyzerError::InvalidArgument(message) => McpError::invalid_params(message, None),
        AnalyzerError::LimitTooLarge { max } => {
            McpError::invalid_params(format!("limit exceeds maximum (max {max})"), None)
        }
        AnalyzerError::Internal(source) => {
            McpError::internal_error(format!("internal error: {source}"), None)
        }
    }
}

/// Map a `ToolRegistry::dispatch` outcome onto an MCP `CallToolResult` /
/// `McpError`. Extracted from `call_tool` so the error-code mapping can be
/// unit-tested without constructing a `RequestContext`.
fn map_dispatch_result(
    result: Result<serde_json::Value, ToolError>,
) -> Result<CallToolResult, McpError> {
    match result {
        Ok(value) => Ok(CallToolResult::structured(value)),
        Err(ToolError::UnknownTool(name)) => Err(McpError::new(
            ErrorCode::METHOD_NOT_FOUND,
            format!("unknown tool: {name}"),
            None,
        )),
        Err(ToolError::InvalidInput(message)) => Err(McpError::invalid_params(message, None)),
        Err(ToolError::Analyzer(err)) => Err(analyzer_to_mcp(err)),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use chrono::{DateTime, NaiveDate, TimeZone, Utc};
    use serde_json::{Value, json};

    use super::*;
    use crate::journal::analyzer::tools::Tool as AnalyzerTool;
    use crate::journal::analyzer::types::{
        DailyReviewView, GetRecentRequest, GetReviewsRequest, JournalEntryView,
        SearchSemanticRequest, SearchTextRequest, SemanticHit, WeeklyReviewView,
    };

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
        async fn dispatch(&self, ctx: &UserContext, args: Value) -> Result<Value, ToolError> {
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
        async fn dispatch(&self, _ctx: &UserContext, _args: Value) -> Result<Value, ToolError> {
            Err(ToolError::Analyzer(AnalyzerError::InvalidArgument(
                "limit must be > 0".into(),
            )))
        }
    }

    #[derive(Default)]
    struct StubJournalService {
        last_id: Mutex<Option<i64>>,
        get_by_id_response: Mutex<Option<JournalEntryView>>,
    }

    #[async_trait]
    impl JournalReadService for StubJournalService {
        async fn get_recent(
            &self,
            _ctx: &UserContext,
            _request: GetRecentRequest,
        ) -> Result<Vec<JournalEntryView>, AnalyzerError> {
            Ok(Vec::new())
        }
        async fn search_text(
            &self,
            _ctx: &UserContext,
            _request: SearchTextRequest,
        ) -> Result<Vec<JournalEntryView>, AnalyzerError> {
            Ok(Vec::new())
        }
        async fn search_semantic(
            &self,
            _ctx: &UserContext,
            _request: SearchSemanticRequest,
        ) -> Result<Vec<SemanticHit>, AnalyzerError> {
            Ok(Vec::new())
        }
        async fn get_by_id(
            &self,
            _ctx: &UserContext,
            id: i64,
        ) -> Result<Option<JournalEntryView>, AnalyzerError> {
            *self.last_id.lock().unwrap() = Some(id);
            Ok(self.get_by_id_response.lock().unwrap().clone())
        }
    }

    #[derive(Default)]
    struct StubReviewService {
        last_daily_date: Mutex<Option<NaiveDate>>,
        last_weekly_week: Mutex<Option<NaiveDate>>,
        daily_response: Mutex<Option<DailyReviewView>>,
        weekly_response: Mutex<Option<WeeklyReviewView>>,
    }

    #[async_trait]
    impl ReviewReadService for StubReviewService {
        async fn get_daily_reviews(
            &self,
            _ctx: &UserContext,
            _request: GetReviewsRequest,
        ) -> Result<Vec<DailyReviewView>, AnalyzerError> {
            Ok(Vec::new())
        }
        async fn get_weekly_reviews(
            &self,
            _ctx: &UserContext,
            _request: GetReviewsRequest,
        ) -> Result<Vec<WeeklyReviewView>, AnalyzerError> {
            Ok(Vec::new())
        }
        async fn get_daily_review(
            &self,
            _ctx: &UserContext,
            review_date: NaiveDate,
        ) -> Result<Option<DailyReviewView>, AnalyzerError> {
            *self.last_daily_date.lock().unwrap() = Some(review_date);
            Ok(self.daily_response.lock().unwrap().clone())
        }
        async fn get_weekly_review(
            &self,
            _ctx: &UserContext,
            week_start: NaiveDate,
        ) -> Result<Option<WeeklyReviewView>, AnalyzerError> {
            *self.last_weekly_week.lock().unwrap() = Some(week_start);
            Ok(self.weekly_response.lock().unwrap().clone())
        }
    }

    fn registry_with(tools: Vec<Arc<dyn AnalyzerTool>>) -> Arc<ToolRegistry> {
        let mut registry = ToolRegistry::new();
        for tool in tools {
            registry.register(tool);
        }
        Arc::new(registry)
    }

    fn components(
        journal: Arc<dyn JournalReadService>,
        review: Arc<dyn ReviewReadService>,
    ) -> AnalyzerMcpComponents {
        AnalyzerMcpComponents {
            registry: registry_with(vec![
                Arc::new(EchoTool { name: "echo" }),
                Arc::new(FailingTool),
            ]),
            journal_service: journal,
            review_service: review,
        }
    }

    fn server() -> AnalyzerMcpServer {
        AnalyzerMcpServer::new(
            components(
                Arc::new(StubJournalService::default()),
                Arc::new(StubReviewService::default()),
            ),
            UserContext::new("u-123"),
        )
    }

    fn at(h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 28, h, 0, 0).unwrap()
    }

    fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
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
    fn server_info_advertises_tools_and_resources_capabilities() {
        let server = server();
        let info = server.get_info();
        assert!(info.capabilities.tools.is_some());
        assert!(info.capabilities.resources.is_some());
        assert_eq!(info.server_info.name, env!("CARGO_PKG_NAME"));
        assert_eq!(info.server_info.version, crate::version::VERSION);
    }

    #[test]
    fn resource_templates_cover_journal_and_review_uris() {
        let server = server();
        let names: Vec<&str> = server
            .resource_templates
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(names.len(), 3);
        for expected in ["journal_entry", "daily_review", "weekly_review"] {
            assert!(
                names.contains(&expected),
                "missing template {expected} in {names:?}"
            );
        }
        let uris: Vec<&str> = server
            .resource_templates
            .iter()
            .map(|t| t.uri_template.as_str())
            .collect();
        assert!(uris.contains(&"journal://entry/{id}"));
        assert!(uris.contains(&"review://daily/{date}"));
        assert!(uris.contains(&"review://weekly/{week_start}"));
    }

    #[test]
    fn parse_resource_uri_recognises_supported_schemes() {
        assert_eq!(
            parse_resource_uri("journal://entry/42"),
            Some(ResourceRef::JournalEntry(42))
        );
        assert_eq!(
            parse_resource_uri("review://daily/2026-04-28"),
            Some(ResourceRef::DailyReview(ymd(2026, 4, 28)))
        );
        assert_eq!(
            parse_resource_uri("review://weekly/2026-04-20"),
            Some(ResourceRef::WeeklyReview(ymd(2026, 4, 20)))
        );
    }

    #[test]
    fn parse_resource_uri_rejects_malformed_uris() {
        assert!(parse_resource_uri("journal://entry/not-a-number").is_none());
        assert!(parse_resource_uri("review://daily/not-a-date").is_none());
        assert!(parse_resource_uri("review://weekly/2026/04/20").is_none());
        assert!(parse_resource_uri("unknown://thing/1").is_none());
    }

    fn server_with(
        journal: Arc<StubJournalService>,
        review: Arc<StubReviewService>,
    ) -> AnalyzerMcpServer {
        AnalyzerMcpServer::new(components(journal, review), UserContext::new("u-123"))
    }

    #[tokio::test]
    async fn read_resource_returns_journal_entry_as_json() {
        let journal = Arc::new(StubJournalService::default());
        *journal.get_by_id_response.lock().unwrap() = Some(JournalEntryView {
            id: 42,
            received_at: at(10),
            text: "hello".into(),
        });
        let server = server_with(journal.clone(), Arc::new(StubReviewService::default()));

        let result = server
            .read_resource_by_uri("journal://entry/42")
            .await
            .expect("read_resource ok");

        assert_eq!(*journal.last_id.lock().unwrap(), Some(42));
        assert_eq!(result.contents.len(), 1);
        let ResourceContents::TextResourceContents {
            uri,
            text,
            mime_type,
            ..
        } = &result.contents[0]
        else {
            panic!("expected text contents, got {:?}", result.contents[0]);
        };
        assert_eq!(uri, "journal://entry/42");
        assert_eq!(mime_type.as_deref(), Some(RESOURCE_MIME_TYPE));
        let parsed: Value = serde_json::from_str(text).expect("payload is JSON");
        assert_eq!(parsed["id"], 42);
        assert_eq!(parsed["text"], "hello");
    }

    #[tokio::test]
    async fn read_resource_returns_daily_review_as_json() {
        let review = Arc::new(StubReviewService::default());
        *review.daily_response.lock().unwrap() = Some(DailyReviewView {
            review_date: ymd(2026, 4, 28),
            review_text: "today".into(),
            created_at: at(9),
        });
        let server = server_with(Arc::new(StubJournalService::default()), review.clone());

        let result = server
            .read_resource_by_uri("review://daily/2026-04-28")
            .await
            .expect("read_resource ok");

        assert_eq!(
            *review.last_daily_date.lock().unwrap(),
            Some(ymd(2026, 4, 28))
        );
        let ResourceContents::TextResourceContents { text, .. } = &result.contents[0] else {
            panic!("expected text contents");
        };
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["review_date"], "2026-04-28");
        assert_eq!(parsed["review_text"], "today");
    }

    #[tokio::test]
    async fn read_resource_returns_weekly_review_as_json() {
        let review = Arc::new(StubReviewService::default());
        *review.weekly_response.lock().unwrap() = Some(WeeklyReviewView {
            week_start: ymd(2026, 4, 20),
            week_end: ymd(2026, 4, 26),
            review_text: "weekly".into(),
            created_at: at(9),
        });
        let server = server_with(Arc::new(StubJournalService::default()), review.clone());

        let result = server
            .read_resource_by_uri("review://weekly/2026-04-20")
            .await
            .expect("read_resource ok");

        assert_eq!(
            *review.last_weekly_week.lock().unwrap(),
            Some(ymd(2026, 4, 20))
        );
        let ResourceContents::TextResourceContents { text, .. } = &result.contents[0] else {
            panic!("expected text contents");
        };
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["week_start"], "2026-04-20");
        assert_eq!(parsed["week_end"], "2026-04-26");
        assert_eq!(parsed["review_text"], "weekly");
    }

    #[tokio::test]
    async fn read_resource_missing_entry_returns_invalid_params() {
        let server = server();
        let err = server
            .read_resource_by_uri("journal://entry/999")
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn read_resource_unknown_scheme_returns_invalid_params() {
        let server = server();
        let err = server
            .read_resource_by_uri("bogus://thing/1")
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(err.message.contains("bogus://thing/1"));
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

    #[test]
    fn map_dispatch_result_forwards_structured_value() {
        let result = map_dispatch_result(Ok(json!({"hello": "world"})))
            .expect("ok dispatch maps to CallToolResult");
        assert_ne!(result.is_error, Some(true));
        assert_eq!(
            result.structured_content,
            Some(json!({"hello": "world"})),
            "Ok value should be forwarded as structured_content"
        );
    }

    #[test]
    fn map_dispatch_result_unknown_tool_to_method_not_found() {
        let err = map_dispatch_result(Err(ToolError::UnknownTool("missing".into())))
            .expect_err("UnknownTool maps to McpError");
        assert_eq!(err.code, ErrorCode::METHOD_NOT_FOUND);
        assert!(err.message.contains("missing"));
    }

    #[test]
    fn map_dispatch_result_invalid_input_to_invalid_params() {
        let err = map_dispatch_result(Err(ToolError::InvalidInput("bad json".into())))
            .expect_err("InvalidInput maps to McpError");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(err.message.contains("bad json"));
    }

    #[test]
    fn map_dispatch_result_analyzer_invalid_argument_to_invalid_params() {
        let err = map_dispatch_result(Err(ToolError::Analyzer(AnalyzerError::InvalidArgument(
            "query is empty".into(),
        ))))
        .expect_err("InvalidArgument maps to McpError");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(err.message.contains("query is empty"));
    }

    #[test]
    fn map_dispatch_result_limit_too_large_to_invalid_params() {
        let err = map_dispatch_result(Err(ToolError::Analyzer(AnalyzerError::LimitTooLarge {
            max: 50,
        })))
        .expect_err("LimitTooLarge maps to McpError");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(
            err.message.contains("50"),
            "message should include the max ({})",
            err.message
        );
    }

    #[test]
    fn map_dispatch_result_internal_to_internal_error() {
        let err = map_dispatch_result(Err(ToolError::Analyzer(AnalyzerError::Internal(Box::<
            dyn std::error::Error + Send + Sync,
        >::from(
            "boom"
        )))))
        .expect_err("Internal maps to McpError");
        assert_eq!(err.code, ErrorCode::INTERNAL_ERROR);
        assert!(err.message.contains("boom"));
    }
}
