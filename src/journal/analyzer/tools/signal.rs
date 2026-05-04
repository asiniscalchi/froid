use std::sync::Arc;

use async_trait::async_trait;
use chrono::NaiveDate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Tool, ToolError, deserialize_input, schema_value, serialize_output};
use crate::journal::analyzer::signal::SignalReadService;
use crate::journal::analyzer::types::{SearchSignalsRequest, SignalView, UserContext};
use crate::journal::extraction::{BehaviorValence, NeedStatus};
use crate::journal::review::signals::types::SignalType;

#[derive(Debug, Deserialize, JsonSchema)]
struct SearchSignalsInput {
    /// Maximum number of signals to return. Must be between 1 and 50.
    limit: u32,
    /// Restrict results to a specific signal_type
    /// (theme, emotion, behavior, need, tension, pattern, tomorrow_attention).
    #[serde(default)]
    signal_type: Option<SignalType>,
    /// Case-insensitive substring filter on the signal label.
    #[serde(default)]
    label_contains: Option<String>,
    /// Restrict results to needs with this status (only meaningful for need signals).
    #[serde(default)]
    status: Option<NeedStatus>,
    /// Restrict results to behaviors with this valence (only meaningful for behavior signals).
    #[serde(default)]
    valence: Option<BehaviorValence>,
    /// Inclusive lower bound on the source review_date in YYYY-MM-DD form.
    #[serde(default)]
    from_date: Option<NaiveDate>,
    /// Exclusive upper bound on the source review_date in YYYY-MM-DD form.
    #[serde(default)]
    to_date_exclusive: Option<NaiveDate>,
    /// Minimum strength threshold; values must lie in [0.0, 1.0].
    #[serde(default)]
    min_strength: Option<f32>,
}

#[derive(Debug, Serialize)]
struct SignalItem {
    id: i64,
    review_date: NaiveDate,
    signal_type: SignalType,
    label: String,
    status: Option<NeedStatus>,
    valence: Option<BehaviorValence>,
    strength: f32,
    confidence: f32,
    evidence: String,
}

impl From<SignalView> for SignalItem {
    fn from(view: SignalView) -> Self {
        Self {
            id: view.id,
            review_date: view.review_date,
            signal_type: view.signal_type,
            label: view.label,
            status: view.status,
            valence: view.valence,
            strength: view.strength,
            confidence: view.confidence,
            evidence: view.evidence,
        }
    }
}

#[derive(Debug, Serialize)]
struct SignalsOutput {
    signals: Vec<SignalItem>,
}

pub struct SignalsSearchTool {
    service: Arc<dyn SignalReadService>,
}

impl SignalsSearchTool {
    pub fn new(service: Arc<dyn SignalReadService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl Tool for SignalsSearchTool {
    fn name(&self) -> &'static str {
        "signals_search"
    }
    fn description(&self) -> &'static str {
        "Search structured signals (themes, emotions, behaviors, needs, tensions, patterns) extracted from the user's daily reviews. Combine filters to narrow results; ordered by review_date ascending."
    }
    fn input_schema(&self) -> Value {
        schema_value::<SearchSignalsInput>()
    }
    async fn dispatch(&self, ctx: &UserContext, args: Value) -> Result<Value, ToolError> {
        let input: SearchSignalsInput = deserialize_input(args)?;
        let signals = self
            .service
            .search(
                ctx,
                SearchSignalsRequest {
                    signal_type: input.signal_type,
                    label_contains: input.label_contains,
                    status: input.status,
                    valence: input.valence,
                    from_date: input.from_date,
                    to_date_exclusive: input.to_date_exclusive,
                    min_strength: input.min_strength,
                    limit: input.limit,
                },
            )
            .await?;
        serialize_output(SignalsOutput {
            signals: signals.into_iter().map(SignalItem::from).collect(),
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
    struct StubSignalService {
        last: Mutex<Option<SearchSignalsRequest>>,
        response: Mutex<Vec<SignalView>>,
    }

    #[async_trait]
    impl SignalReadService for StubSignalService {
        async fn search(
            &self,
            _ctx: &UserContext,
            request: SearchSignalsRequest,
        ) -> Result<Vec<SignalView>, AnalyzerError> {
            *self.last.lock().unwrap() = Some(request);
            Ok(self.response.lock().unwrap().clone())
        }
    }

    fn ctx() -> UserContext {
        UserContext::new("user-1")
    }

    #[tokio::test]
    async fn signals_search_dispatches_filters_and_serializes_output() {
        let stub = Arc::new(StubSignalService::default());
        *stub.response.lock().unwrap() = vec![SignalView {
            id: 11,
            review_date: NaiveDate::from_ymd_opt(2026, 4, 28).unwrap(),
            signal_type: SignalType::Need,
            label: "control".into(),
            status: Some(NeedStatus::Unmet),
            valence: None,
            strength: 0.7,
            confidence: 0.85,
            evidence: "evidence text".into(),
        }];
        let tool = SignalsSearchTool::new(stub.clone());

        let out = tool
            .dispatch(
                &ctx(),
                json!({
                    "limit": 10,
                    "signal_type": "need",
                    "status": "unmet",
                    "min_strength": 0.5,
                    "from_date": "2026-04-01",
                    "to_date_exclusive": "2026-05-01"
                }),
            )
            .await
            .unwrap();

        let captured = stub.last.lock().unwrap().clone().unwrap();
        assert_eq!(captured.limit, 10);
        assert_eq!(captured.signal_type, Some(SignalType::Need));
        assert_eq!(captured.status, Some(NeedStatus::Unmet));
        assert_eq!(captured.min_strength, Some(0.5));
        assert_eq!(captured.from_date, NaiveDate::from_ymd_opt(2026, 4, 1));
        assert_eq!(
            captured.to_date_exclusive,
            NaiveDate::from_ymd_opt(2026, 5, 1)
        );

        assert_eq!(out["signals"][0]["id"], 11);
        assert_eq!(out["signals"][0]["signal_type"], "need");
        assert_eq!(out["signals"][0]["status"], "unmet");
        assert_eq!(out["signals"][0]["label"], "control");
    }

    #[tokio::test]
    async fn signals_search_accepts_minimal_input() {
        let stub = Arc::new(StubSignalService::default());
        let tool = SignalsSearchTool::new(stub.clone());

        let _ = tool.dispatch(&ctx(), json!({"limit": 5})).await.unwrap();

        let captured = stub.last.lock().unwrap().clone().unwrap();
        assert_eq!(captured.limit, 5);
        assert!(captured.signal_type.is_none());
        assert!(captured.label_contains.is_none());
        assert!(captured.status.is_none());
        assert!(captured.valence.is_none());
        assert!(captured.from_date.is_none());
        assert!(captured.to_date_exclusive.is_none());
        assert!(captured.min_strength.is_none());
    }

    #[tokio::test]
    async fn signals_search_rejects_unknown_signal_type() {
        let tool = SignalsSearchTool::new(Arc::new(StubSignalService::default()));

        let err = tool
            .dispatch(&ctx(), json!({"limit": 5, "signal_type": "diagnosis"}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[test]
    fn signals_search_schema_is_object_with_required_limit() {
        let tool = SignalsSearchTool::new(Arc::new(StubSignalService::default()));
        let schema = tool.input_schema();
        assert!(schema.is_object());
        let required = schema["required"].as_array().unwrap();
        let names: Vec<_> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(names.contains(&"limit"));
    }
}
