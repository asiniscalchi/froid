use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Tool, ToolError, deserialize_input, schema_value, serialize_output};
use crate::journal::analyzer::journal::JournalReadService;
use crate::journal::analyzer::types::{
    GetRecentRequest, JournalEntryView, SearchSemanticRequest, SearchTextRequest, SemanticHit,
    UserContext,
};

#[derive(Debug, Deserialize, JsonSchema)]
struct GetRecentInput {
    /// Maximum number of entries to return. Must be between 1 and 50.
    limit: u32,
    /// Inclusive lower bound on the entry date in YYYY-MM-DD form. Optional.
    #[serde(default)]
    from_date: Option<NaiveDate>,
    /// Exclusive upper bound on the entry date in YYYY-MM-DD form. Optional.
    #[serde(default)]
    to_date_exclusive: Option<NaiveDate>,
}

#[derive(Debug, Serialize)]
struct JournalEntryItem {
    id: i64,
    received_at: DateTime<Utc>,
    text: String,
}

impl From<JournalEntryView> for JournalEntryItem {
    fn from(view: JournalEntryView) -> Self {
        Self {
            id: view.id,
            received_at: view.received_at,
            text: view.text,
        }
    }
}

#[derive(Debug, Serialize)]
struct JournalEntriesOutput {
    entries: Vec<JournalEntryItem>,
}

pub struct JournalGetRecentTool {
    service: Arc<dyn JournalReadService>,
}

impl JournalGetRecentTool {
    pub fn new(service: Arc<dyn JournalReadService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl Tool for JournalGetRecentTool {
    fn name(&self) -> &'static str {
        "journal_get_recent"
    }
    fn description(&self) -> &'static str {
        "Retrieve the user's recent journal entries, newest first. Optionally filter by date range."
    }
    fn input_schema(&self) -> Value {
        schema_value::<GetRecentInput>()
    }
    async fn dispatch(&self, ctx: &UserContext, args: Value) -> Result<Value, ToolError> {
        let input: GetRecentInput = deserialize_input(args)?;
        let entries = self
            .service
            .get_recent(
                ctx,
                GetRecentRequest {
                    limit: input.limit,
                    from_date: input.from_date,
                    to_date_exclusive: input.to_date_exclusive,
                },
            )
            .await?;
        serialize_output(JournalEntriesOutput {
            entries: entries.into_iter().map(JournalEntryItem::from).collect(),
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SearchTextInput {
    /// Free-text query to match (case-insensitive substring on raw entry text).
    query: String,
    /// Maximum number of entries to return. Must be between 1 and 50.
    limit: u32,
    /// Inclusive lower bound on the entry date in YYYY-MM-DD form. Optional.
    #[serde(default)]
    from_date: Option<NaiveDate>,
    /// Exclusive upper bound on the entry date in YYYY-MM-DD form. Optional.
    #[serde(default)]
    to_date_exclusive: Option<NaiveDate>,
}

pub struct JournalSearchTextTool {
    service: Arc<dyn JournalReadService>,
}

impl JournalSearchTextTool {
    pub fn new(service: Arc<dyn JournalReadService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl Tool for JournalSearchTextTool {
    fn name(&self) -> &'static str {
        "journal_search_text"
    }
    fn description(&self) -> &'static str {
        "Search the user's journal entries for a literal substring. Use when the user asks about specific names, places, or words they wrote."
    }
    fn input_schema(&self) -> Value {
        schema_value::<SearchTextInput>()
    }
    async fn dispatch(&self, ctx: &UserContext, args: Value) -> Result<Value, ToolError> {
        let input: SearchTextInput = deserialize_input(args)?;
        let entries = self
            .service
            .search_text(
                ctx,
                SearchTextRequest {
                    query: input.query,
                    limit: input.limit,
                    from_date: input.from_date,
                    to_date_exclusive: input.to_date_exclusive,
                },
            )
            .await?;
        serialize_output(JournalEntriesOutput {
            entries: entries.into_iter().map(JournalEntryItem::from).collect(),
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SearchSemanticInput {
    /// Natural-language query describing what to look for. The query is embedded
    /// and compared to journal entry embeddings.
    query: String,
    /// Maximum number of hits to return. Must be between 1 and 20.
    limit: u32,
    /// Inclusive lower bound on the entry date in YYYY-MM-DD form. Optional.
    #[serde(default)]
    from_date: Option<NaiveDate>,
    /// Exclusive upper bound on the entry date in YYYY-MM-DD form. Optional.
    #[serde(default)]
    to_date_exclusive: Option<NaiveDate>,
}

#[derive(Debug, Serialize)]
struct SemanticHitItem {
    id: i64,
    received_at: DateTime<Utc>,
    text: String,
    distance: f32,
}

impl From<SemanticHit> for SemanticHitItem {
    fn from(hit: SemanticHit) -> Self {
        Self {
            id: hit.id,
            received_at: hit.received_at,
            text: hit.text,
            distance: hit.distance,
        }
    }
}

#[derive(Debug, Serialize)]
struct SemanticHitsOutput {
    hits: Vec<SemanticHitItem>,
}

pub struct JournalSearchSemanticTool {
    service: Arc<dyn JournalReadService>,
}

impl JournalSearchSemanticTool {
    pub fn new(service: Arc<dyn JournalReadService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl Tool for JournalSearchSemanticTool {
    fn name(&self) -> &'static str {
        "journal_search_semantic"
    }
    fn description(&self) -> &'static str {
        "Search the user's journal entries for entries that are semantically similar to a natural-language query. Prefer this for fuzzy themes (\"feeling avoidant\", \"anxiety before meetings\")."
    }
    fn input_schema(&self) -> Value {
        schema_value::<SearchSemanticInput>()
    }
    async fn dispatch(&self, ctx: &UserContext, args: Value) -> Result<Value, ToolError> {
        let input: SearchSemanticInput = deserialize_input(args)?;
        let hits = self
            .service
            .search_semantic(
                ctx,
                SearchSemanticRequest {
                    query: input.query,
                    limit: input.limit,
                    from_date: input.from_date,
                    to_date_exclusive: input.to_date_exclusive,
                },
            )
            .await?;
        serialize_output(SemanticHitsOutput {
            hits: hits.into_iter().map(SemanticHitItem::from).collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use std::sync::Mutex;

    use super::*;
    use crate::journal::analyzer::types::AnalyzerError;

    #[derive(Default)]
    struct StubJournalService {
        last_recent: Mutex<Option<GetRecentRequest>>,
        last_text: Mutex<Option<SearchTextRequest>>,
        last_semantic: Mutex<Option<SearchSemanticRequest>>,
        recent_response: Mutex<Vec<JournalEntryView>>,
        text_response: Mutex<Vec<JournalEntryView>>,
        semantic_response: Mutex<Vec<SemanticHit>>,
    }

    #[async_trait]
    impl JournalReadService for StubJournalService {
        async fn get_recent(
            &self,
            _ctx: &UserContext,
            request: GetRecentRequest,
        ) -> Result<Vec<JournalEntryView>, AnalyzerError> {
            *self.last_recent.lock().unwrap() = Some(request);
            Ok(self.recent_response.lock().unwrap().clone())
        }
        async fn search_text(
            &self,
            _ctx: &UserContext,
            request: SearchTextRequest,
        ) -> Result<Vec<JournalEntryView>, AnalyzerError> {
            *self.last_text.lock().unwrap() = Some(request);
            Ok(self.text_response.lock().unwrap().clone())
        }
        async fn search_semantic(
            &self,
            _ctx: &UserContext,
            request: SearchSemanticRequest,
        ) -> Result<Vec<SemanticHit>, AnalyzerError> {
            *self.last_semantic.lock().unwrap() = Some(request);
            Ok(self.semantic_response.lock().unwrap().clone())
        }
    }

    fn ctx() -> UserContext {
        UserContext::new("user-1")
    }

    fn at(h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 28, h, 0, 0).unwrap()
    }
    use chrono::TimeZone;

    #[tokio::test]
    async fn journal_get_recent_dispatches_request_and_serializes_response() {
        let stub = Arc::new(StubJournalService::default());
        *stub.recent_response.lock().unwrap() = vec![JournalEntryView {
            id: 7,
            received_at: at(10),
            text: "hello".into(),
        }];
        let tool = JournalGetRecentTool::new(stub.clone());

        let out = tool
            .dispatch(
                &ctx(),
                json!({
                    "limit": 5,
                    "from_date": "2026-04-28",
                    "to_date_exclusive": "2026-04-29"
                }),
            )
            .await
            .unwrap();

        let captured = stub.last_recent.lock().unwrap().clone().unwrap();
        assert_eq!(captured.limit, 5);
        assert_eq!(captured.from_date, NaiveDate::from_ymd_opt(2026, 4, 28));
        assert_eq!(
            captured.to_date_exclusive,
            NaiveDate::from_ymd_opt(2026, 4, 29)
        );
        assert_eq!(out["entries"][0]["id"], 7);
        assert_eq!(out["entries"][0]["text"], "hello");
    }

    #[tokio::test]
    async fn journal_get_recent_omits_optional_dates_when_absent() {
        let stub = Arc::new(StubJournalService::default());
        let tool = JournalGetRecentTool::new(stub.clone());

        let _ = tool.dispatch(&ctx(), json!({"limit": 3})).await.unwrap();

        let captured = stub.last_recent.lock().unwrap().clone().unwrap();
        assert_eq!(captured.limit, 3);
        assert!(captured.from_date.is_none());
        assert!(captured.to_date_exclusive.is_none());
    }

    #[tokio::test]
    async fn journal_get_recent_rejects_malformed_input() {
        let tool = JournalGetRecentTool::new(Arc::new(StubJournalService::default()));

        let err = tool
            .dispatch(&ctx(), json!({"limit": "not a number"}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn journal_get_recent_propagates_analyzer_errors() {
        struct FailingService;
        #[async_trait]
        impl JournalReadService for FailingService {
            async fn get_recent(
                &self,
                _: &UserContext,
                _: GetRecentRequest,
            ) -> Result<Vec<JournalEntryView>, AnalyzerError> {
                Err(AnalyzerError::LimitTooLarge { max: 50 })
            }
            async fn search_text(
                &self,
                _: &UserContext,
                _: SearchTextRequest,
            ) -> Result<Vec<JournalEntryView>, AnalyzerError> {
                unreachable!()
            }
            async fn search_semantic(
                &self,
                _: &UserContext,
                _: SearchSemanticRequest,
            ) -> Result<Vec<SemanticHit>, AnalyzerError> {
                unreachable!()
            }
        }
        let tool = JournalGetRecentTool::new(Arc::new(FailingService));

        let err = tool
            .dispatch(&ctx(), json!({"limit": 999}))
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            ToolError::Analyzer(AnalyzerError::LimitTooLarge { max: 50 })
        ));
    }

    #[tokio::test]
    async fn journal_search_text_dispatches_request() {
        let stub = Arc::new(StubJournalService::default());
        *stub.text_response.lock().unwrap() = vec![JournalEntryView {
            id: 1,
            received_at: at(10),
            text: "match".into(),
        }];
        let tool = JournalSearchTextTool::new(stub.clone());

        let out = tool
            .dispatch(&ctx(), json!({"query": "anxious", "limit": 5}))
            .await
            .unwrap();

        let captured = stub.last_text.lock().unwrap().clone().unwrap();
        assert_eq!(captured.query, "anxious");
        assert_eq!(captured.limit, 5);
        assert_eq!(out["entries"][0]["text"], "match");
    }

    #[tokio::test]
    async fn journal_search_semantic_dispatches_request_and_returns_distance() {
        let stub = Arc::new(StubJournalService::default());
        *stub.semantic_response.lock().unwrap() = vec![SemanticHit {
            id: 9,
            received_at: at(10),
            text: "deep cut".into(),
            distance: 0.123,
        }];
        let tool = JournalSearchSemanticTool::new(stub.clone());

        let out = tool
            .dispatch(&ctx(), json!({"query": "avoidance", "limit": 3}))
            .await
            .unwrap();

        let captured = stub.last_semantic.lock().unwrap().clone().unwrap();
        assert_eq!(captured.query, "avoidance");
        assert_eq!(captured.limit, 3);
        let distance = out["hits"][0]["distance"].as_f64().unwrap();
        assert!((distance - 0.123).abs() < 1e-6);
        assert_eq!(out["hits"][0]["id"], 9);
    }

    #[test]
    fn input_schema_is_well_formed_json_object() {
        let stub = Arc::new(StubJournalService::default());
        for schema in [
            JournalGetRecentTool::new(stub.clone()).input_schema(),
            JournalSearchTextTool::new(stub.clone()).input_schema(),
            JournalSearchSemanticTool::new(stub).input_schema(),
        ] {
            assert!(schema.is_object());
            assert!(schema["properties"].is_object());
        }
    }

    #[test]
    fn tools_have_distinct_names_and_descriptions() {
        let stub = Arc::new(StubJournalService::default());
        let names: Vec<_> = [
            JournalGetRecentTool::new(stub.clone()).name(),
            JournalSearchTextTool::new(stub.clone()).name(),
            JournalSearchSemanticTool::new(stub).name(),
        ]
        .into_iter()
        .collect();
        assert_eq!(names.len(), 3);
        assert!(names.iter().all(|n| !n.is_empty()));
        let unique: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len());
    }
}
