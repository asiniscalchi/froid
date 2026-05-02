use std::{error::Error, fmt};

use super::types::{DailyReviewSignalCandidate, SignalType};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyReviewSignalValidationError {
    message: String,
}

impl DailyReviewSignalValidationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for DailyReviewSignalValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl Error for DailyReviewSignalValidationError {}

pub fn validate_signal(
    signal: &DailyReviewSignalCandidate,
) -> Result<(), DailyReviewSignalValidationError> {
    if signal.label.trim().is_empty() {
        return Err(DailyReviewSignalValidationError::new("label must not be empty"));
    }

    if signal.evidence.trim().is_empty() {
        return Err(DailyReviewSignalValidationError::new("evidence must not be empty"));
    }

    if !(0.0..=1.0).contains(&signal.strength) {
        return Err(DailyReviewSignalValidationError::new(format!(
            "strength must be between 0.0 and 1.0, got {}",
            signal.strength
        )));
    }

    if !(0.0..=1.0).contains(&signal.confidence) {
        return Err(DailyReviewSignalValidationError::new(format!(
            "confidence must be between 0.0 and 1.0, got {}",
            signal.confidence
        )));
    }

    if signal.signal_type == SignalType::Need && signal.status.is_none() {
        return Err(DailyReviewSignalValidationError::new(
            "need signal must have a status",
        ));
    }

    if signal.signal_type != SignalType::Need && signal.status.is_some() {
        return Err(DailyReviewSignalValidationError::new(format!(
            "{} signal must not have a status",
            signal.signal_type.as_str()
        )));
    }

    if signal.signal_type == SignalType::Behavior && signal.valence.is_none() {
        return Err(DailyReviewSignalValidationError::new(
            "behavior signal must have a valence",
        ));
    }

    if signal.signal_type != SignalType::Behavior && signal.valence.is_some() {
        return Err(DailyReviewSignalValidationError::new(format!(
            "{} signal must not have a valence",
            signal.signal_type.as_str()
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::journal::extraction::{BehaviorValence, NeedStatus};

    use super::*;

    fn candidate(signal_type: SignalType) -> DailyReviewSignalCandidate {
        DailyReviewSignalCandidate {
            signal_type,
            label: "test label".to_string(),
            status: None,
            valence: None,
            strength: 0.7,
            confidence: 0.8,
            evidence: "some evidence".to_string(),
        }
    }

    fn need_candidate() -> DailyReviewSignalCandidate {
        DailyReviewSignalCandidate {
            status: Some(NeedStatus::Unmet),
            ..candidate(SignalType::Need)
        }
    }

    fn behavior_candidate() -> DailyReviewSignalCandidate {
        DailyReviewSignalCandidate {
            valence: Some(BehaviorValence::Positive),
            ..candidate(SignalType::Behavior)
        }
    }

    #[test]
    fn accepts_valid_theme_signal() {
        assert!(validate_signal(&candidate(SignalType::Theme)).is_ok());
    }

    #[test]
    fn accepts_valid_need_signal_with_status() {
        assert!(validate_signal(&need_candidate()).is_ok());
    }

    #[test]
    fn accepts_valid_behavior_signal_with_valence() {
        assert!(validate_signal(&behavior_candidate()).is_ok());
    }

    #[test]
    fn rejects_empty_label() {
        let signal = DailyReviewSignalCandidate {
            label: "  ".to_string(),
            ..candidate(SignalType::Theme)
        };
        let err = validate_signal(&signal).unwrap_err();
        assert!(err.to_string().contains("label must not be empty"));
    }

    #[test]
    fn rejects_empty_evidence() {
        let signal = DailyReviewSignalCandidate {
            evidence: "".to_string(),
            ..candidate(SignalType::Theme)
        };
        let err = validate_signal(&signal).unwrap_err();
        assert!(err.to_string().contains("evidence must not be empty"));
    }

    #[test]
    fn rejects_strength_above_1() {
        let signal = DailyReviewSignalCandidate {
            strength: 1.01,
            ..candidate(SignalType::Theme)
        };
        let err = validate_signal(&signal).unwrap_err();
        assert!(err.to_string().contains("strength must be between 0.0 and 1.0"));
    }

    #[test]
    fn rejects_strength_below_0() {
        let signal = DailyReviewSignalCandidate {
            strength: -0.01,
            ..candidate(SignalType::Theme)
        };
        let err = validate_signal(&signal).unwrap_err();
        assert!(err.to_string().contains("strength must be between 0.0 and 1.0"));
    }

    #[test]
    fn rejects_confidence_above_1() {
        let signal = DailyReviewSignalCandidate {
            confidence: 1.5,
            ..candidate(SignalType::Emotion)
        };
        let err = validate_signal(&signal).unwrap_err();
        assert!(err.to_string().contains("confidence must be between 0.0 and 1.0"));
    }

    #[test]
    fn rejects_confidence_below_0() {
        let signal = DailyReviewSignalCandidate {
            confidence: -0.1,
            ..candidate(SignalType::Emotion)
        };
        let err = validate_signal(&signal).unwrap_err();
        assert!(err.to_string().contains("confidence must be between 0.0 and 1.0"));
    }

    #[test]
    fn rejects_need_signal_without_status() {
        let signal = candidate(SignalType::Need);
        let err = validate_signal(&signal).unwrap_err();
        assert!(err.to_string().contains("need signal must have a status"));
    }

    #[test]
    fn rejects_non_need_signal_with_status() {
        let signal = DailyReviewSignalCandidate {
            status: Some(NeedStatus::Fulfilled),
            ..candidate(SignalType::Theme)
        };
        let err = validate_signal(&signal).unwrap_err();
        assert!(err.to_string().contains("must not have a status"));
    }

    #[test]
    fn rejects_behavior_signal_without_valence() {
        let signal = candidate(SignalType::Behavior);
        let err = validate_signal(&signal).unwrap_err();
        assert!(err.to_string().contains("behavior signal must have a valence"));
    }

    #[test]
    fn rejects_non_behavior_signal_with_valence() {
        let signal = DailyReviewSignalCandidate {
            valence: Some(BehaviorValence::Neutral),
            ..candidate(SignalType::Tension)
        };
        let err = validate_signal(&signal).unwrap_err();
        assert!(err.to_string().contains("must not have a valence"));
    }

    #[test]
    fn accepts_boundary_strength_and_confidence_values() {
        let min = DailyReviewSignalCandidate {
            strength: 0.0,
            confidence: 0.0,
            ..candidate(SignalType::Pattern)
        };
        let max = DailyReviewSignalCandidate {
            strength: 1.0,
            confidence: 1.0,
            ..candidate(SignalType::Pattern)
        };
        assert!(validate_signal(&min).is_ok());
        assert!(validate_signal(&max).is_ok());
    }
}
