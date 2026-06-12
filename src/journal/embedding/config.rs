use super::DEFAULT_EMBEDDING_MODEL;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingConfig {
    pub model: String,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_EMBEDDING_MODEL.to_string(),
        }
    }
}

impl EmbeddingConfig {
    pub(crate) fn from_values(model: Option<String>) -> Self {
        let model = model
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_EMBEDDING_MODEL.to_string());

        Self { model }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::embedding::DEFAULT_EMBEDDING_MODEL;

    #[test]
    fn uses_default_model() {
        let config = EmbeddingConfig::from_values(None);

        assert_eq!(config.model, DEFAULT_EMBEDDING_MODEL);
    }

    #[test]
    fn uses_custom_model() {
        let config = EmbeddingConfig::from_values(Some("custom-model".to_string()));

        assert_eq!(config.model, "custom-model");
    }
}
