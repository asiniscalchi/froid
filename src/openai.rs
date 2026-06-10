//! Shared construction of rig OpenAI clients.
//!
//! Every LLM-facing component (embedder, extraction and review generators)
//! builds its client here so the optional `OPENAI_BASE_URL` override applies
//! uniformly. Pointing it at any OpenAI-compatible endpoint (Ollama,
//! OpenRouter, a self-hosted gateway) keeps journal data off openai.com
//! entirely.

use rig::providers::openai::Client as OpenAiClient;

pub(crate) const OPENAI_BASE_URL_ENV: &str = "OPENAI_BASE_URL";

/// Build a client for the given API key, honoring `OPENAI_BASE_URL` when set.
pub(crate) fn client_from_env(api_key: &str) -> rig::http_client::Result<OpenAiClient> {
    client_with_base_url(api_key, std::env::var(OPENAI_BASE_URL_ENV).ok().as_deref())
}

fn client_with_base_url(
    api_key: &str,
    base_url: Option<&str>,
) -> rig::http_client::Result<OpenAiClient> {
    let builder = OpenAiClient::builder().api_key(api_key);
    match base_url.map(str::trim).filter(|url| !url.is_empty()) {
        Some(url) => builder.base_url(url).build(),
        None => builder.build(),
    }
}

#[cfg(test)]
mod tests {
    use super::client_with_base_url;

    #[test]
    fn defaults_to_openai_base_url() {
        let client = client_with_base_url("key", None).unwrap();

        assert_eq!(client.base_url(), "https://api.openai.com/v1");
    }

    #[test]
    fn overrides_base_url_when_provided() {
        let client = client_with_base_url("key", Some("http://localhost:11434/v1")).unwrap();

        assert_eq!(client.base_url(), "http://localhost:11434/v1");
    }

    #[test]
    fn ignores_blank_base_url() {
        let client = client_with_base_url("key", Some("   ")).unwrap();

        assert_eq!(client.base_url(), "https://api.openai.com/v1");
    }
}
