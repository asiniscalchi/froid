use chrono::{DateTime, NaiveDate, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::journal::extraction::{BehaviorValence, NeedStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SignalType {
    Theme,
    Emotion,
    Behavior,
    Need,
    Tension,
    Pattern,
    TomorrowAttention,
}

impl SignalType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Theme => "theme",
            Self::Emotion => "emotion",
            Self::Behavior => "behavior",
            Self::Need => "need",
            Self::Tension => "tension",
            Self::Pattern => "pattern",
            Self::TomorrowAttention => "tomorrow_attention",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "theme" => Some(Self::Theme),
            "emotion" => Some(Self::Emotion),
            "behavior" => Some(Self::Behavior),
            "need" => Some(Self::Need),
            "tension" => Some(Self::Tension),
            "pattern" => Some(Self::Pattern),
            "tomorrow_attention" => Some(Self::TomorrowAttention),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalJobStatus {
    Pending,
    Completed,
    Failed,
}

impl SignalJobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

/// A single signal as returned by the LLM.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DailyReviewSignalCandidate {
    pub signal_type: SignalType,
    pub label: String,
    pub status: Option<NeedStatus>,
    pub valence: Option<BehaviorValence>,
    pub strength: f32,
    pub confidence: f32,
    pub evidence: String,
}

/// The full LLM output for signal extraction — a list of candidates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DailyReviewSignalsOutput {
    pub signals: Vec<DailyReviewSignalCandidate>,
}

/// A persisted signal, linked to its source daily review.
#[derive(Debug, Clone, PartialEq)]
pub struct DailyReviewSignal {
    pub id: i64,
    pub daily_review_id: i64,
    pub user_id: String,
    pub review_date: NaiveDate,
    pub signal_type: SignalType,
    pub label: String,
    pub status: Option<NeedStatus>,
    pub valence: Option<BehaviorValence>,
    pub strength: f32,
    pub confidence: f32,
    pub evidence: String,
    pub model: String,
    pub prompt_version: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A job record tracking the outcome of a single signal generation run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyReviewSignalJob {
    pub id: i64,
    pub daily_review_id: i64,
    pub status: SignalJobStatus,
    pub error_message: Option<String>,
    pub model: Option<String>,
    pub prompt_version: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_type_round_trips_through_str() {
        for (variant, expected) in [
            (SignalType::Theme, "theme"),
            (SignalType::Emotion, "emotion"),
            (SignalType::Behavior, "behavior"),
            (SignalType::Need, "need"),
            (SignalType::Tension, "tension"),
            (SignalType::Pattern, "pattern"),
            (SignalType::TomorrowAttention, "tomorrow_attention"),
        ] {
            assert_eq!(variant.as_str(), expected);
            assert_eq!(SignalType::from_str(expected), Some(variant));
        }
    }

    #[test]
    fn signal_type_from_str_returns_none_for_unknown_value() {
        assert_eq!(SignalType::from_str("unknown"), None);
        assert_eq!(SignalType::from_str(""), None);
    }

    #[test]
    fn signal_candidate_deserializes_from_json() {
        let json = r#"{
            "signals": [
                {
                    "signal_type": "theme",
                    "label": "physical appearance",
                    "status": null,
                    "valence": null,
                    "strength": 0.8,
                    "confidence": 0.9,
                    "evidence": "Review mentions concern around training and diet."
                },
                {
                    "signal_type": "need",
                    "label": "control",
                    "status": "unmet",
                    "valence": null,
                    "strength": 0.7,
                    "confidence": 0.85,
                    "evidence": "Review describes repeated attempts to regain control."
                },
                {
                    "signal_type": "behavior",
                    "label": "plan switching",
                    "status": null,
                    "valence": "negative",
                    "strength": 0.75,
                    "confidence": 0.8,
                    "evidence": "Review notes plan was changed multiple times."
                }
            ]
        }"#;

        let output: DailyReviewSignalsOutput = serde_json::from_str(json).unwrap();

        assert_eq!(output.signals.len(), 3);
        assert_eq!(output.signals[0].signal_type, SignalType::Theme);
        assert_eq!(output.signals[0].label, "physical appearance");
        assert_eq!(output.signals[0].status, None);
        assert_eq!(output.signals[0].valence, None);
        assert_eq!(output.signals[1].signal_type, SignalType::Need);
        assert_eq!(output.signals[1].status, Some(NeedStatus::Unmet));
        assert_eq!(output.signals[2].signal_type, SignalType::Behavior);
        assert_eq!(output.signals[2].valence, Some(BehaviorValence::Negative));
    }

    #[test]
    fn signal_candidate_fails_to_deserialize_invalid_signal_type() {
        let json = r#"{
            "signals": [{"signal_type": "diagnosis", "label": "x", "status": null, "valence": null,
                          "strength": 0.5, "confidence": 0.5, "evidence": "e"}]
        }"#;

        let result: Result<DailyReviewSignalsOutput, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn signal_candidate_fails_to_deserialize_invalid_need_status() {
        let json = r#"{
            "signals": [{"signal_type": "need", "label": "x", "status": "blocked", "valence": null,
                          "strength": 0.5, "confidence": 0.5, "evidence": "e"}]
        }"#;

        let result: Result<DailyReviewSignalsOutput, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn signal_candidate_fails_to_deserialize_invalid_behavior_valence() {
        let json = r#"{
            "signals": [{"signal_type": "behavior", "label": "x", "status": null, "valence": "bad",
                          "strength": 0.5, "confidence": 0.5, "evidence": "e"}]
        }"#;

        let result: Result<DailyReviewSignalsOutput, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn all_signal_types_deserialize() {
        for type_str in [
            "theme",
            "emotion",
            "behavior",
            "need",
            "tension",
            "pattern",
            "tomorrow_attention",
        ] {
            let json = format!(
                r#"{{"signals": [{{"signal_type": "{type_str}", "label": "x", "status": null,
                     "valence": null, "strength": 0.5, "confidence": 0.5, "evidence": "e"}}]}}"#
            );
            let result: Result<DailyReviewSignalsOutput, _> = serde_json::from_str(&json);
            assert!(result.is_ok(), "failed for signal_type: {type_str}");
        }
    }
}
