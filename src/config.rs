//! Configuration: upstream URL, format, timeouts.

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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: "0.0.0.0:8080".to_string(),
            upstream_url: "https://api.openai.com/v1".to_string(),
            fixed_upstream_format: None,
            upstream_timeout: Duration::from_secs(120),
        }
    }
}

impl Config {
    /// Load config from environment variables.
    /// - `LISTEN` — e.g. "0.0.0.0:8080"
    /// - `UPSTREAM_URL` — e.g. "https://api.openai.com/v1"
    /// - `UPSTREAM_FORMAT` — optional; if set, skip discovery and use this format only
    /// - `UPSTREAM_TIMEOUT_SECS` — optional; default 120
    pub fn from_env() -> Self {
        let listen = std::env::var("LISTEN").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
        let upstream_url =
            std::env::var("UPSTREAM_URL").unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
        let fixed_upstream_format = std::env::var("UPSTREAM_FORMAT").ok().and_then(|s| s.parse().ok());
        let upstream_timeout = std::env::var("UPSTREAM_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(120));
        Self {
            listen,
            upstream_url,
            fixed_upstream_format,
            upstream_timeout,
        }
    }

    /// Build the full POST URL for a given format (path suffix per provider).
    /// For Google (Gemini), pass `Some(model)` so the URL includes `/models/{model}:generateContent` per official API.
    pub fn upstream_url_for_format(&self, format: UpstreamFormat, model: Option<&str>) -> String {
        build_upstream_url(&self.upstream_url, format, model)
    }
}

/// Build full upstream POST URL for a format (reference: protocol baselines).
/// Google Gemini API requires model in path: POST .../models/{model}:generateContent.
pub fn build_upstream_url(base_url: &str, format: UpstreamFormat, model: Option<&str>) -> String {
    let base = base_url.trim_end_matches('/');
    match format {
        UpstreamFormat::OpenAiCompletion => format!("{}/chat/completions", base),
        UpstreamFormat::OpenAiResponses => format!("{}/responses", base),
        UpstreamFormat::Anthropic => format!("{}/messages", base),
        UpstreamFormat::Google => {
            let model = model
                .filter(|s| !s.is_empty())
                .unwrap_or("gemini-1.5");
            format!("{}/models/{}:generateContent", base, model)
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_upstream_url_openai_completion() {
        assert_eq!(
            build_upstream_url("https://api.openai.com/v1", UpstreamFormat::OpenAiCompletion, None),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn build_upstream_url_openai_responses() {
        assert_eq!(
            build_upstream_url("https://api.openai.com/v1", UpstreamFormat::OpenAiResponses, None),
            "https://api.openai.com/v1/responses"
        );
    }

    #[test]
    fn build_upstream_url_anthropic() {
        assert_eq!(
            build_upstream_url("https://api.anthropic.com/v1", UpstreamFormat::Anthropic, None),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn build_upstream_url_google() {
        assert_eq!(
            build_upstream_url("https://generativelanguage.googleapis.com/v1beta", UpstreamFormat::Google, None),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-1.5:generateContent"
        );
        assert_eq!(
            build_upstream_url("https://generativelanguage.googleapis.com/v1beta", UpstreamFormat::Google, Some("gemini-2.0-flash")),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.0-flash:generateContent"
        );
    }

    #[test]
    fn build_upstream_url_strips_trailing_slash() {
        assert_eq!(
            build_upstream_url("https://api.openai.com/v1/", UpstreamFormat::OpenAiCompletion, None),
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
        assert!(c.upstream_url_for_format(UpstreamFormat::OpenAiCompletion, None).ends_with("/chat/completions"));
        assert!(c.upstream_url_for_format(UpstreamFormat::OpenAiResponses, None).ends_with("/responses"));
    }
}
