use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BehaviorValence {
    Positive,
    Negative,
    Ambiguous,
    Neutral,
    Unclear,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum NeedStatus {
    Activated,
    Unmet,
    Fulfilled,
    Unclear,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct EmotionExtraction {
    pub label: String,
    pub intensity: f32,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BehaviorExtraction {
    pub label: String,
    pub valence: BehaviorValence,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct NeedExtraction {
    pub label: String,
    pub status: NeedStatus,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct PatternExtraction {
    pub description: String,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct JournalEntryExtractionResult {
    pub summary: String,
    pub domains: Vec<String>,
    pub emotions: Vec<EmotionExtraction>,
    pub behaviors: Vec<BehaviorExtraction>,
    pub needs: Vec<NeedExtraction>,
    pub possible_patterns: Vec<PatternExtraction>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_valid_extraction_with_all_enums() {
        let json = r#"{
            "summary": "Feeling good",
            "domains": ["health"],
            "emotions": [{"label": "joy", "intensity": 0.8, "confidence": 0.9}],
            "behaviors": [{"label": "exercise", "valence": "positive", "confidence": 0.8}],
            "needs": [{"label": "autonomy", "status": "fulfilled", "confidence": 0.7}],
            "possible_patterns": [{"description": "Consistent exercise helps.", "confidence": 0.5}]
        }"#;

        let result: JournalEntryExtractionResult = serde_json::from_str(json).unwrap();

        assert_eq!(result.summary, "Feeling good");
        assert_eq!(result.needs[0].status, NeedStatus::Fulfilled);
        assert_eq!(result.behaviors[0].valence, BehaviorValence::Positive);
    }

    #[test]
    fn fails_to_deserialize_invalid_need_status() {
        let json = r#"{
            "summary": "Feeling good",
            "domains": [],
            "emotions": [],
            "behaviors": [],
            "needs": [{"label": "autonomy", "status": "blocked", "confidence": 0.7}],
            "possible_patterns": []
        }"#;

        let result: Result<JournalEntryExtractionResult, _> = serde_json::from_str(json);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unknown variant `blocked`")
        );
    }

    #[test]
    fn schema_contains_all_need_status_enums() {
        let schema = schemars::schema_for!(JournalEntryExtractionResult);
        let schema_json = serde_json::to_string(&schema).unwrap();

        assert!(schema_json.contains("\"activated\""));
        assert!(schema_json.contains("\"unmet\""));
        assert!(schema_json.contains("\"fulfilled\""));
        assert!(schema_json.contains("\"unclear\""));
        assert!(!schema_json.contains("\"blocked\""));
    }
}
