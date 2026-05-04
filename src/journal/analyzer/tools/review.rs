use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Tool, ToolError, deserialize_input, schema_value, serialize_output};
use crate::journal::analyzer::review::ReviewReadService;
use crate::journal::analyzer::types::{
    DailyReviewView, GetReviewsRequest, UserContext, WeeklyReviewView,
};

#[derive(Debug, Deserialize, JsonSchema)]
struct GetReviewsInput {
    /// Inclusive lower bound on the review date in YYYY-MM-DD form.
    from_date: NaiveDate,
    /// Exclusive upper bound on the review date in YYYY-MM-DD form. Must be
    /// strictly greater than from_date.
    to_date_exclusive: NaiveDate,
}

#[derive(Debug, Serialize)]
struct DailyReviewItem {
    review_date: NaiveDate,
    review_text: String,
    created_at: DateTime<Utc>,
}

impl From<DailyReviewView> for DailyReviewItem {
    fn from(view: DailyReviewView) -> Self {
        Self {
            review_date: view.review_date,
            review_text: view.review_text,
            created_at: view.created_at,
        }
    }
}

#[derive(Debug, Serialize)]
struct DailyReviewsOutput {
    reviews: Vec<DailyReviewItem>,
}

pub struct DailyReviewGetRangeTool {
    service: Arc<dyn ReviewReadService>,
}

impl DailyReviewGetRangeTool {
    pub fn new(service: Arc<dyn ReviewReadService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl Tool for DailyReviewGetRangeTool {
    fn name(&self) -> &'static str {
        "daily_review_get_range"
    }
    fn description(&self) -> &'static str {
        "Retrieve completed daily reviews for a date range, ordered by date ascending. Missing dates return no entry; this tool never generates new reviews."
    }
    fn input_schema(&self) -> Value {
        schema_value::<GetReviewsInput>()
    }
    async fn dispatch(&self, ctx: &UserContext, args: Value) -> Result<Value, ToolError> {
        let input: GetReviewsInput = deserialize_input(args)?;
        let reviews = self
            .service
            .get_daily_reviews(
                ctx,
                GetReviewsRequest {
                    from_date: input.from_date,
                    to_date_exclusive: input.to_date_exclusive,
                },
            )
            .await?;
        serialize_output(DailyReviewsOutput {
            reviews: reviews.into_iter().map(DailyReviewItem::from).collect(),
        })
    }
}

#[derive(Debug, Serialize)]
struct WeeklyReviewItem {
    week_start: NaiveDate,
    week_end: NaiveDate,
    review_text: String,
    created_at: DateTime<Utc>,
}

impl From<WeeklyReviewView> for WeeklyReviewItem {
    fn from(view: WeeklyReviewView) -> Self {
        Self {
            week_start: view.week_start,
            week_end: view.week_end,
            review_text: view.review_text,
            created_at: view.created_at,
        }
    }
}

#[derive(Debug, Serialize)]
struct WeeklyReviewsOutput {
    reviews: Vec<WeeklyReviewItem>,
}

pub struct WeeklyReviewGetRangeTool {
    service: Arc<dyn ReviewReadService>,
}

impl WeeklyReviewGetRangeTool {
    pub fn new(service: Arc<dyn ReviewReadService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl Tool for WeeklyReviewGetRangeTool {
    fn name(&self) -> &'static str {
        "weekly_review_get_range"
    }
    fn description(&self) -> &'static str {
        "Retrieve completed weekly reviews whose week_start_date falls in the given range, ordered ascending. week_end is week_start + 6 days. Never generates new reviews."
    }
    fn input_schema(&self) -> Value {
        schema_value::<GetReviewsInput>()
    }
    async fn dispatch(&self, ctx: &UserContext, args: Value) -> Result<Value, ToolError> {
        let input: GetReviewsInput = deserialize_input(args)?;
        let reviews = self
            .service
            .get_weekly_reviews(
                ctx,
                GetReviewsRequest {
                    from_date: input.from_date,
                    to_date_exclusive: input.to_date_exclusive,
                },
            )
            .await?;
        serialize_output(WeeklyReviewsOutput {
            reviews: reviews.into_iter().map(WeeklyReviewItem::from).collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;
    use std::sync::Mutex;

    use super::*;
    use crate::journal::analyzer::types::AnalyzerError;

    #[derive(Default)]
    struct StubReviewService {
        last_daily: Mutex<Option<GetReviewsRequest>>,
        last_weekly: Mutex<Option<GetReviewsRequest>>,
        daily_response: Mutex<Vec<DailyReviewView>>,
        weekly_response: Mutex<Vec<WeeklyReviewView>>,
    }

    #[async_trait]
    impl ReviewReadService for StubReviewService {
        async fn get_daily_reviews(
            &self,
            _ctx: &UserContext,
            request: GetReviewsRequest,
        ) -> Result<Vec<DailyReviewView>, AnalyzerError> {
            *self.last_daily.lock().unwrap() = Some(request);
            Ok(self.daily_response.lock().unwrap().clone())
        }
        async fn get_weekly_reviews(
            &self,
            _ctx: &UserContext,
            request: GetReviewsRequest,
        ) -> Result<Vec<WeeklyReviewView>, AnalyzerError> {
            *self.last_weekly.lock().unwrap() = Some(request);
            Ok(self.weekly_response.lock().unwrap().clone())
        }
    }

    fn ctx() -> UserContext {
        UserContext::new("user-1")
    }

    fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn ts() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 28, 9, 0, 0).unwrap()
    }

    #[tokio::test]
    async fn daily_review_get_range_dispatches_and_serializes() {
        let stub = Arc::new(StubReviewService::default());
        *stub.daily_response.lock().unwrap() = vec![DailyReviewView {
            review_date: ymd(2026, 4, 28),
            review_text: "today's review".into(),
            created_at: ts(),
        }];
        let tool = DailyReviewGetRangeTool::new(stub.clone());

        let out = tool
            .dispatch(
                &ctx(),
                json!({
                    "from_date": "2026-04-27",
                    "to_date_exclusive": "2026-04-29"
                }),
            )
            .await
            .unwrap();

        let captured = stub.last_daily.lock().unwrap().clone().unwrap();
        assert_eq!(captured.from_date, ymd(2026, 4, 27));
        assert_eq!(captured.to_date_exclusive, ymd(2026, 4, 29));
        assert_eq!(out["reviews"][0]["review_text"], "today's review");
        assert_eq!(out["reviews"][0]["review_date"], "2026-04-28");
    }

    #[tokio::test]
    async fn daily_review_get_range_requires_both_dates() {
        let tool = DailyReviewGetRangeTool::new(Arc::new(StubReviewService::default()));

        let err = tool
            .dispatch(&ctx(), json!({"from_date": "2026-04-27"}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn weekly_review_get_range_dispatches_and_emits_week_end() {
        let stub = Arc::new(StubReviewService::default());
        *stub.weekly_response.lock().unwrap() = vec![WeeklyReviewView {
            week_start: ymd(2026, 4, 20),
            week_end: ymd(2026, 4, 26),
            review_text: "week recap".into(),
            created_at: ts(),
        }];
        let tool = WeeklyReviewGetRangeTool::new(stub.clone());

        let out = tool
            .dispatch(
                &ctx(),
                json!({
                    "from_date": "2026-04-20",
                    "to_date_exclusive": "2026-04-27"
                }),
            )
            .await
            .unwrap();

        let captured = stub.last_weekly.lock().unwrap().clone().unwrap();
        assert_eq!(captured.from_date, ymd(2026, 4, 20));
        assert_eq!(captured.to_date_exclusive, ymd(2026, 4, 27));
        assert_eq!(out["reviews"][0]["week_start"], "2026-04-20");
        assert_eq!(out["reviews"][0]["week_end"], "2026-04-26");
    }

    #[test]
    fn review_tools_have_distinct_names() {
        let stub = Arc::new(StubReviewService::default());
        let daily = DailyReviewGetRangeTool::new(stub.clone());
        let weekly = WeeklyReviewGetRangeTool::new(stub);
        assert_ne!(daily.name(), weekly.name());
    }

    #[test]
    fn review_input_schema_is_object_with_required_dates() {
        let tool = DailyReviewGetRangeTool::new(Arc::new(StubReviewService::default()));
        let schema = tool.input_schema();
        assert!(schema.is_object());
        let required = schema["required"].as_array().unwrap();
        let names: Vec<_> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(names.contains(&"from_date"));
        assert!(names.contains(&"to_date_exclusive"));
    }
}
