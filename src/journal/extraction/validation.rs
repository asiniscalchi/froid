use std::{error::Error, fmt};

use serde_json::Value;

const REQUIRED_TOP_LEVEL_FIELDS: [&str; 6] = [
    "summary",
    "domains",
    "emotions",
    "behaviors",
    "needs",
    "possible_patterns",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryExtractionValidationError {
    message: String,
}

impl EntryExtractionValidationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for EntryExtractionValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl Error for EntryExtractionValidationError {}

pub fn validate_extraction_json(raw: &str) -> Result<String, EntryExtractionValidationError> {
    let value: Value = serde_json::from_str(raw).map_err(|error| {
        EntryExtractionValidationError::new(format!("extraction output is not valid JSON: {error}"))
    })?;

    let object = value.as_object().ok_or_else(|| {
        EntryExtractionValidationError::new("extraction output must be an object")
    })?;

    for field in REQUIRED_TOP_LEVEL_FIELDS {
        if !object.contains_key(field) {
            return Err(EntryExtractionValidationError::new(format!(
                "extraction output is missing required field: {field}"
            )));
        }
    }

    require_string(object.get("summary"), "summary")?;
    require_string_array(object.get("domains"), "domains")?;
    validate_emotions(object.get("emotions"))?;
    validate_behaviors(object.get("behaviors"))?;
    validate_needs(object.get("needs"))?;
    validate_possible_patterns(object.get("possible_patterns"))?;

    serde_json::to_string(&value).map_err(|error| {
        EntryExtractionValidationError::new(format!("failed to serialize extraction JSON: {error}"))
    })
}

fn validate_emotions(value: Option<&Value>) -> Result<(), EntryExtractionValidationError> {
    for (index, item) in require_array(value, "emotions")?.iter().enumerate() {
        require_object(item, &format!("emotions[{index}]"))?;
        require_string(item.get("label"), &format!("emotions[{index}].label"))?;
        require_number_0_to_1(
            item.get("intensity"),
            &format!("emotions[{index}].intensity"),
        )?;
        require_number_0_to_1(
            item.get("confidence"),
            &format!("emotions[{index}].confidence"),
        )?;
    }
    Ok(())
}

fn validate_behaviors(value: Option<&Value>) -> Result<(), EntryExtractionValidationError> {
    for (index, item) in require_array(value, "behaviors")?.iter().enumerate() {
        require_object(item, &format!("behaviors[{index}]"))?;
        require_string(item.get("label"), &format!("behaviors[{index}].label"))?;
        require_enum(
            item.get("valence"),
            &format!("behaviors[{index}].valence"),
            &["positive", "negative", "neutral", "mixed"],
        )?;
        require_number_0_to_1(
            item.get("confidence"),
            &format!("behaviors[{index}].confidence"),
        )?;
    }
    Ok(())
}

fn validate_needs(value: Option<&Value>) -> Result<(), EntryExtractionValidationError> {
    for (index, item) in require_array(value, "needs")?.iter().enumerate() {
        require_object(item, &format!("needs[{index}]"))?;
        require_string(item.get("label"), &format!("needs[{index}].label"))?;
        require_enum(
            item.get("status"),
            &format!("needs[{index}].status"),
            &["activated", "unmet", "fulfilled", "unclear"],
        )?;
        require_number_0_to_1(
            item.get("confidence"),
            &format!("needs[{index}].confidence"),
        )?;
    }
    Ok(())
}

fn validate_possible_patterns(value: Option<&Value>) -> Result<(), EntryExtractionValidationError> {
    let patterns = require_array(value, "possible_patterns")?;
    if patterns.len() > 3 {
        return Err(EntryExtractionValidationError::new(
            "possible_patterns must contain at most 3 items",
        ));
    }

    for (index, item) in patterns.iter().enumerate() {
        require_object(item, &format!("possible_patterns[{index}]"))?;
        require_string(
            item.get("description"),
            &format!("possible_patterns[{index}].description"),
        )?;
        require_number_0_to_1(
            item.get("confidence"),
            &format!("possible_patterns[{index}].confidence"),
        )?;
    }
    Ok(())
}

fn require_object<'a>(
    value: &'a Value,
    path: &str,
) -> Result<&'a serde_json::Map<String, Value>, EntryExtractionValidationError> {
    value
        .as_object()
        .ok_or_else(|| EntryExtractionValidationError::new(format!("{path} must be an object")))
}

fn require_array<'a>(
    value: Option<&'a Value>,
    path: &str,
) -> Result<&'a Vec<Value>, EntryExtractionValidationError> {
    value
        .and_then(Value::as_array)
        .ok_or_else(|| EntryExtractionValidationError::new(format!("{path} must be an array")))
}

fn require_string(value: Option<&Value>, path: &str) -> Result<(), EntryExtractionValidationError> {
    value
        .and_then(Value::as_str)
        .map(|_| ())
        .ok_or_else(|| EntryExtractionValidationError::new(format!("{path} must be a string")))
}

fn require_string_array(
    value: Option<&Value>,
    path: &str,
) -> Result<(), EntryExtractionValidationError> {
    for (index, item) in require_array(value, path)?.iter().enumerate() {
        if !item.is_string() {
            return Err(EntryExtractionValidationError::new(format!(
                "{path}[{index}] must be a string"
            )));
        }
    }
    Ok(())
}

fn require_number_0_to_1(
    value: Option<&Value>,
    path: &str,
) -> Result<(), EntryExtractionValidationError> {
    let Some(number) = value.and_then(Value::as_f64) else {
        return Err(EntryExtractionValidationError::new(format!(
            "{path} must be a number"
        )));
    };

    if !(0.0..=1.0).contains(&number) {
        return Err(EntryExtractionValidationError::new(format!(
            "{path} must be between 0 and 1"
        )));
    }

    Ok(())
}

fn require_enum(
    value: Option<&Value>,
    path: &str,
    allowed: &[&str],
) -> Result<(), EntryExtractionValidationError> {
    let Some(value) = value.and_then(Value::as_str) else {
        return Err(EntryExtractionValidationError::new(format!(
            "{path} must be a string"
        )));
    };

    if !allowed.contains(&value) {
        return Err(EntryExtractionValidationError::new(format!(
            "{path} must be one of: {}",
            allowed.join(", ")
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_extraction() -> Value {
        serde_json::json!({
            "summary": "The note records a brief factual update.",
            "domains": [],
            "emotions": [],
            "behaviors": [],
            "needs": [],
            "possible_patterns": []
        })
    }

    fn validate(value: Value) -> Result<String, EntryExtractionValidationError> {
        validate_extraction_json(&value.to_string())
    }

    #[test]
    fn accepts_compact_extraction_with_empty_uncertain_fields() {
        let normalized = validate(valid_extraction()).unwrap();
        let value: Value = serde_json::from_str(&normalized).unwrap();

        assert_eq!(value["summary"], "The note records a brief factual update.");
        assert_eq!(value["domains"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn rejects_missing_required_fields() {
        let error = validate_extraction_json(r#"{"summary":"hello"}"#).unwrap_err();

        assert!(error.to_string().contains("missing required field"));
    }

    #[test]
    fn rejects_values_outside_confidence_range() {
        let mut extraction = valid_extraction();
        extraction["emotions"] = serde_json::json!([{
            "label": "anxiety",
            "intensity": 1.2,
            "confidence": 0.8
        }]);

        let error = validate(extraction).unwrap_err();

        assert!(error.to_string().contains("between 0 and 1"));
    }

    #[test]
    fn rejects_invalid_enums() {
        let mut extraction = valid_extraction();
        extraction["behaviors"] = serde_json::json!([{
            "label": "avoidance",
            "valence": "bad",
            "confidence": 0.7
        }]);

        let error = validate(extraction).unwrap_err();

        assert!(error.to_string().contains("must be one of"));
    }

    #[test]
    fn rejects_too_many_possible_patterns() {
        let mut extraction = valid_extraction();
        extraction["possible_patterns"] = serde_json::json!([
            {"description": "One cautious possibility.", "confidence": 0.3},
            {"description": "Another cautious possibility.", "confidence": 0.3},
            {"description": "A third cautious possibility.", "confidence": 0.3},
            {"description": "A fourth cautious possibility.", "confidence": 0.3}
        ]);

        let error = validate(extraction).unwrap_err();

        assert!(error.to_string().contains("at most 3"));
    }
}
