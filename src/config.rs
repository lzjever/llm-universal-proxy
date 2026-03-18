//! Configuration: upstream URL, format, timeouts.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::formats::UpstreamFormat;

/// Runtime configuration for the proxy.
#[derive(Debug, Clone)]
pub struct Config {
    /// Listen address (e.g. "0.0.0.0:8080").
    pub listen: String,
    /// Upstream base URL (e.g. "https://api.openai.com/v1").
    pub upstream_url: String,
    /// If set, use this as the only supported format (no discovery).
    pub fixed_upstream_format: Option<UpstreamFormat>,
    /// Request timeout to upstream.
    pub upstream_timeout: Duration,
    /// Optional API key to inject into upstream requests.
    /// Set via UPSTREAM_API_KEY env var. If the client doesn't provide auth,
    /// this key will be used as Bearer token.
    pub upstream_api_key: Option<String>,
    /// Optional static headers to inject into every upstream request.
    /// Set via UPSTREAM_HEADERS env var as JSON object.
    pub upstream_headers: Vec<(String, String)>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: "0.0.0.0:8080".to_string(),
            upstream_url: "https://api.openai.com/v1".to_string(),
            fixed_upstream_format: None,
            upstream_timeout: Duration::from_secs(120),
            upstream_api_key: None,
            upstream_headers: Vec::new(),
        }
    }
}

impl Config {
    /// Load config from environment variables.
    /// - `LISTEN` — e.g. "0.0.0.0:8080"
    /// - `UPSTREAM_URL` — e.g. "https://api.openai.com/v1"
    /// - `UPSTREAM_FORMAT` — optional; if set, skip discovery and use this format only
    /// - `UPSTREAM_TIMEOUT_SECS` — optional; default 120
    /// - `UPSTREAM_API_KEY` — optional; API key to inject if client doesn't provide auth
    /// - `UPSTREAM_HEADERS` — optional JSON object of static headers to inject upstream
    pub fn from_env() -> Self {
        let listen = std::env::var("LISTEN").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
        let upstream_url = std::env::var("UPSTREAM_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
        let fixed_upstream_format = std::env::var("UPSTREAM_FORMAT")
            .ok()
            .and_then(|s| s.parse().ok());
        let upstream_timeout = std::env::var("UPSTREAM_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(120));
        let upstream_api_key = std::env::var("UPSTREAM_API_KEY").ok();
        let upstream_headers = parse_upstream_headers_env();
        Self {
            listen,
            upstream_url,
            fixed_upstream_format,
            upstream_timeout,
            upstream_api_key,
            upstream_headers,
        }
    }

    /// Build the full POST URL for a given format (path suffix per provider).
    /// For Google (Gemini), pass `Some(model)` so the URL includes
    /// `/models/{model}:generateContent` or `/models/{model}:streamGenerateContent?alt=sse`
    /// per the official API.
    pub fn upstream_url_for_format(
        &self,
        format: UpstreamFormat,
        model: Option<&str>,
        stream: bool,
    ) -> String {
        build_upstream_url(&self.upstream_url, format, model, stream)
    }
}

fn parse_upstream_headers_env() -> Vec<(String, String)> {
    let raw = match std::env::var("UPSTREAM_HEADERS") {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let parsed: BTreeMap<String, String> = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(err) => {
            eprintln!(
                "warning: failed to parse UPSTREAM_HEADERS as JSON object: {}",
                err
            );
            return Vec::new();
        }
    };

    parsed.into_iter().collect()
}

/// Build full upstream POST URL for a format (reference: protocol baselines).
/// Google Gemini API requires model in path:
/// - standard: POST .../models/{model}:generateContent
/// - streaming: POST .../models/{model}:streamGenerateContent?alt=sse
pub fn build_upstream_url(
    base_url: &str,
    format: UpstreamFormat,
    model: Option<&str>,
    stream: bool,
) -> String {
    let base = base_url.trim_end_matches('/');
    match format {
        UpstreamFormat::OpenAiCompletion => format!("{}/chat/completions", base),
        UpstreamFormat::OpenAiResponses => format!("{}/responses", base),
        UpstreamFormat::Anthropic => format!("{}/messages", base),
        UpstreamFormat::Google => {
            let model = model.filter(|s| !s.is_empty()).unwrap_or("gemini-1.5");
            if stream {
                format!("{}/models/{}:streamGenerateContent?alt=sse", base, model)
            } else {
                format!("{}/models/{}:generateContent", base, model)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_upstream_url_openai_completion() {
        assert_eq!(
            build_upstream_url(
                "https://api.openai.com/v1",
                UpstreamFormat::OpenAiCompletion,
                None,
                false
            ),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn build_upstream_url_openai_responses() {
        assert_eq!(
            build_upstream_url(
                "https://api.openai.com/v1",
                UpstreamFormat::OpenAiResponses,
                None,
                false
            ),
            "https://api.openai.com/v1/responses"
        );
    }

    #[test]
    fn build_upstream_url_anthropic() {
        assert_eq!(
            build_upstream_url(
                "https://api.anthropic.com/v1",
                UpstreamFormat::Anthropic,
                None,
                false
            ),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn build_upstream_url_google() {
        assert_eq!(
            build_upstream_url(
                "https://generativelanguage.googleapis.com/v1beta",
                UpstreamFormat::Google,
                None,
                false
            ),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-1.5:generateContent"
        );
        assert_eq!(
            build_upstream_url(
                "https://generativelanguage.googleapis.com/v1beta",
                UpstreamFormat::Google,
                Some("gemini-2.0-flash"),
                false
            ),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.0-flash:generateContent"
        );
        assert_eq!(
            build_upstream_url(
                "https://generativelanguage.googleapis.com/v1beta",
                UpstreamFormat::Google,
                Some("gemini-2.0-flash"),
                true
            ),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.0-flash:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn build_upstream_url_strips_trailing_slash() {
        assert_eq!(
            build_upstream_url(
                "https://api.openai.com/v1/",
                UpstreamFormat::OpenAiCompletion,
                None,
                false
            ),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn config_from_env_uses_defaults_when_env_unset() {
        let c = Config::from_env();
        assert!(!c.listen.is_empty());
        assert!(!c.upstream_url.is_empty());
        assert!(c.upstream_timeout.as_secs() > 0);
    }

    #[test]
    fn config_upstream_url_for_format() {
        let c = Config::default();
        assert!(c
            .upstream_url_for_format(UpstreamFormat::OpenAiCompletion, None, false)
            .ends_with("/chat/completions"));
        assert!(c
            .upstream_url_for_format(UpstreamFormat::OpenAiResponses, None, false)
            .ends_with("/responses"));
        assert!(c
            .upstream_url_for_format(UpstreamFormat::Google, Some("gemini-2.0-flash"), true)
            .ends_with(":streamGenerateContent?alt=sse"));
    }

    #[test]
    fn default_headers_empty() {
        let c = Config::default();
        assert!(c.upstream_headers.is_empty());
    }
}
