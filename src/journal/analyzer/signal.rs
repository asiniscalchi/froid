use async_trait::async_trait;

use crate::journal::review::signals::repository::{
    DailyReviewSignalRepository, DailyReviewSignalRepositoryError, SignalSearchFilters,
};

use super::types::{
    AnalyzerError, MAX_SIGNAL_LIMIT, SearchSignalsRequest, SignalView, UserContext,
};
use super::validation::{validate_limit, validate_optional_range};

#[async_trait]
pub trait SignalReadService: Send + Sync {
    async fn search(
        &self,
        ctx: &UserContext,
        request: SearchSignalsRequest,
    ) -> Result<Vec<SignalView>, AnalyzerError>;
}

#[derive(Debug, Clone)]
pub struct DefaultSignalReadService {
    repository: DailyReviewSignalRepository,
}

impl DefaultSignalReadService {
    pub fn new(repository: DailyReviewSignalRepository) -> Self {
        Self { repository }
    }
}

fn map_storage_error(err: DailyReviewSignalRepositoryError) -> AnalyzerError {
    AnalyzerError::Internal(Box::new(err))
}

fn validate_min_strength(min_strength: Option<f32>) -> Result<(), AnalyzerError> {
    if let Some(s) = min_strength
        && !(0.0..=1.0).contains(&s)
    {
        return Err(AnalyzerError::InvalidArgument(
            "min_strength must be in [0.0, 1.0]".into(),
        ));
    }
    Ok(())
}

fn normalize_label_contains(value: Option<String>) -> Result<Option<String>, AnalyzerError> {
    match value {
        None => Ok(None),
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                Err(AnalyzerError::InvalidArgument(
                    "label_contains must not be blank when provided".into(),
                ))
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
    }
}

#[async_trait]
impl SignalReadService for DefaultSignalReadService {
    async fn search(
        &self,
        ctx: &UserContext,
        request: SearchSignalsRequest,
    ) -> Result<Vec<SignalView>, AnalyzerError> {
        let limit = validate_limit(request.limit, MAX_SIGNAL_LIMIT)?;
        validate_optional_range(request.from_date, request.to_date_exclusive)?;
        validate_min_strength(request.min_strength)?;
        let label_contains = normalize_label_contains(request.label_contains)?;

        let filters = SignalSearchFilters {
            signal_type: request.signal_type,
            label_contains,
            status: request.status,
            valence: request.valence,
            from_date: request.from_date,
            to_date_exclusive: request.to_date_exclusive,
            min_strength: request.min_strength,
            limit,
        };

        let rows = self
            .repository
            .search(&ctx.user_id, &filters)
            .await
            .map_err(map_storage_error)?;

        Ok(rows
            .into_iter()
            .map(|signal| SignalView {
                id: signal.id,
                review_date: signal.review_date,
                signal_type: signal.signal_type,
                label: signal.label,
                status: signal.status,
                valence: signal.valence,
                strength: signal.strength,
                confidence: signal.confidence,
                evidence: signal.evidence,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use sqlx::SqlitePool;

    use super::*;
    use crate::database;
    use crate::journal::extraction::{BehaviorValence, NeedStatus};
    use crate::journal::review::repository::DailyReviewRepository;
    use crate::journal::review::signals::types::{DailyReviewSignalCandidate, SignalType};

    async fn setup() -> (
        DefaultSignalReadService,
        DailyReviewSignalRepository,
        DailyReviewRepository,
    ) {
        database::register_sqlite_vec_extension();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        let signals = DailyReviewSignalRepository::new(pool.clone());
        let reviews = DailyReviewRepository::new(pool);
        let service = DefaultSignalReadService::new(signals.clone());
        (service, signals, reviews)
    }

    fn ctx() -> UserContext {
        UserContext::new("user-1")
    }

    fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn theme() -> DailyReviewSignalCandidate {
        DailyReviewSignalCandidate {
            signal_type: SignalType::Theme,
            label: "physical appearance".to_string(),
            status: None,
            valence: None,
            strength: 0.8,
            confidence: 0.9,
            evidence: "evidence".to_string(),
        }
    }

    fn need() -> DailyReviewSignalCandidate {
        DailyReviewSignalCandidate {
            signal_type: SignalType::Need,
            label: "control".to_string(),
            status: Some(NeedStatus::Unmet),
            valence: None,
            strength: 0.7,
            confidence: 0.85,
            evidence: "evidence".to_string(),
        }
    }

    fn behavior() -> DailyReviewSignalCandidate {
        DailyReviewSignalCandidate {
            signal_type: SignalType::Behavior,
            label: "plan switching".to_string(),
            status: None,
            valence: Some(BehaviorValence::Negative),
            strength: 0.4,
            confidence: 0.8,
            evidence: "evidence".to_string(),
        }
    }

    async fn seed(
        signals: &DailyReviewSignalRepository,
        reviews: &DailyReviewRepository,
        user_id: &str,
        date: NaiveDate,
        candidates: &[DailyReviewSignalCandidate],
    ) {
        let review = reviews
            .upsert_completed(user_id, date, "review text", "m", "v1")
            .await
            .unwrap();
        signals
            .replace_in_transaction(review.id, user_id, date, candidates, "m", "v1")
            .await
            .unwrap();
    }

    fn req(limit: u32) -> SearchSignalsRequest {
        SearchSignalsRequest {
            limit,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn search_returns_signals_in_date_then_id_order() {
        let (service, signals, reviews) = setup().await;
        seed(&signals, &reviews, "user-1", ymd(2026, 4, 27), &[theme()]).await;
        seed(&signals, &reviews, "user-1", ymd(2026, 4, 28), &[need()]).await;
        seed(
            &signals,
            &reviews,
            "user-1",
            ymd(2026, 4, 29),
            &[behavior()],
        )
        .await;

        let result = service.search(&ctx(), req(10)).await.unwrap();

        assert_eq!(result.len(), 3);
        let dates: Vec<_> = result.iter().map(|s| s.review_date).collect();
        assert_eq!(
            dates,
            vec![ymd(2026, 4, 27), ymd(2026, 4, 28), ymd(2026, 4, 29)]
        );
    }

    #[tokio::test]
    async fn search_applies_filters() {
        let (service, signals, reviews) = setup().await;
        seed(&signals, &reviews, "user-1", ymd(2026, 4, 27), &[theme()]).await;
        seed(&signals, &reviews, "user-1", ymd(2026, 4, 28), &[need()]).await;
        seed(
            &signals,
            &reviews,
            "user-1",
            ymd(2026, 4, 29),
            &[behavior()],
        )
        .await;

        let result = service
            .search(
                &ctx(),
                SearchSignalsRequest {
                    signal_type: Some(SignalType::Behavior),
                    valence: Some(BehaviorValence::Negative),
                    ..req(10)
                },
            )
            .await
            .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].signal_type, SignalType::Behavior);
        assert_eq!(result[0].valence, Some(BehaviorValence::Negative));
    }

    #[tokio::test]
    async fn search_trims_label_contains_and_rejects_blank() {
        let (service, signals, reviews) = setup().await;
        seed(&signals, &reviews, "user-1", ymd(2026, 4, 27), &[theme()]).await;

        let result = service
            .search(
                &ctx(),
                SearchSignalsRequest {
                    label_contains: Some("  PHYSICAL  ".to_string()),
                    ..req(10)
                },
            )
            .await
            .unwrap();
        assert_eq!(result.len(), 1);

        let err = service
            .search(
                &ctx(),
                SearchSignalsRequest {
                    label_contains: Some("   ".to_string()),
                    ..req(10)
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, AnalyzerError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn search_rejects_min_strength_outside_range() {
        let (service, _, _) = setup().await;
        for invalid in [-0.1, 1.1] {
            let err = service
                .search(
                    &ctx(),
                    SearchSignalsRequest {
                        min_strength: Some(invalid),
                        ..req(10)
                    },
                )
                .await
                .unwrap_err();
            assert!(matches!(err, AnalyzerError::InvalidArgument(_)));
        }
    }

    #[tokio::test]
    async fn search_rejects_zero_limit() {
        let (service, _, _) = setup().await;
        let err = service.search(&ctx(), req(0)).await.unwrap_err();
        assert!(matches!(err, AnalyzerError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn search_rejects_limit_above_max() {
        let (service, _, _) = setup().await;
        let err = service
            .search(&ctx(), req(MAX_SIGNAL_LIMIT + 1))
            .await
            .unwrap_err();
        assert!(matches!(err, AnalyzerError::LimitTooLarge { .. }));
    }

    #[tokio::test]
    async fn search_rejects_inverted_range() {
        let (service, _, _) = setup().await;
        let err = service
            .search(
                &ctx(),
                SearchSignalsRequest {
                    from_date: Some(ymd(2026, 4, 29)),
                    to_date_exclusive: Some(ymd(2026, 4, 28)),
                    ..req(10)
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, AnalyzerError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn search_uses_single_user_scope() {
        let (service, signals, reviews) = setup().await;
        seed(&signals, &reviews, "user-1", ymd(2026, 4, 27), &[theme()]).await;
        seed(&signals, &reviews, "user-2", ymd(2026, 4, 27), &[theme()]).await;

        let result = service.search(&ctx(), req(10)).await.unwrap();

        assert_eq!(result.len(), 1);
    }

    #[tokio::test]
    async fn search_view_preserves_signal_fields() {
        let (service, signals, reviews) = setup().await;
        seed(&signals, &reviews, "user-1", ymd(2026, 4, 28), &[need()]).await;

        let result = service.search(&ctx(), req(10)).await.unwrap();

        assert_eq!(result.len(), 1);
        let s = &result[0];
        assert!(s.id > 0);
        assert_eq!(s.review_date, ymd(2026, 4, 28));
        assert_eq!(s.signal_type, SignalType::Need);
        assert_eq!(s.label, "control");
        assert_eq!(s.status, Some(NeedStatus::Unmet));
        assert_eq!(s.valence, None);
        assert!((s.strength - 0.7).abs() < 1e-6);
        assert!((s.confidence - 0.85).abs() < 1e-6);
        assert_eq!(s.evidence, "evidence");
    }
}
