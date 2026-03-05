//! Upstream format discovery: probe supported formats and choose default (most generic) conversion target.

use std::collections::HashSet;

use crate::config::build_upstream_url;
use crate::formats::UpstreamFormat;

/// Order of "genericity" for default conversion target (first supported wins).
const DEFAULT_TARGET_ORDER: [UpstreamFormat; 4] = [
    UpstreamFormat::OpenAiCompletion,
    UpstreamFormat::OpenAiResponses,
    UpstreamFormat::Anthropic,
    UpstreamFormat::Google,
];

/// Resolved upstream capability: which formats are supported and the default target for translation.
#[derive(Debug, Clone)]
pub struct UpstreamCapability {
    /// Formats the upstream supports (discovered or from fixed config).
    pub supported: HashSet<UpstreamFormat>,
    /// Default format to use when client format is not in supported (most generic of supported).
    pub default_target: UpstreamFormat,
}

impl UpstreamCapability {
    /// Use a single fixed format (no discovery): supported = {format}, default_target = format.
    pub fn fixed(format: UpstreamFormat) -> Self {
        let mut supported = HashSet::new();
        supported.insert(format);
        Self {
            supported,
            default_target: format,
        }
    }

    /// Choose default target as first in DEFAULT_TARGET_ORDER that is in supported.
    pub fn from_supported(supported: HashSet<UpstreamFormat>) -> Self {
        let default_target = DEFAULT_TARGET_ORDER
            .iter()
            .find(|f| supported.contains(f))
            .copied()
            .unwrap_or(UpstreamFormat::OpenAiCompletion);
        Self {
            supported,
            default_target,
        }
    }

    /// Upstream format to use for this request: client format if supported, else default target.
    pub fn upstream_format_for_request(&self, client_format: UpstreamFormat) -> UpstreamFormat {
        if self.supported.contains(&client_format) {
            client_format
        } else {
            self.default_target
        }
    }

    /// Whether we should passthrough (no translation) for this client format.
    pub fn should_passthrough(&self, client_format: UpstreamFormat) -> bool {
        self.supported.contains(&client_format)
    }
}

/// Minimal JSON body for probe (invalid but enough to get a non-404 from the right endpoint).
fn minimal_probe_body(format: UpstreamFormat) -> serde_json::Value {
    match format {
        UpstreamFormat::OpenAiCompletion => serde_json::json!({
            "model": "gpt-4o",
            "messages": []
        }),
        UpstreamFormat::OpenAiResponses => serde_json::json!({
            "model": "gpt-4o",
            "input": []
        }),
        UpstreamFormat::Anthropic => serde_json::json!({
            "model": "claude-3-5-sonnet-20241022",
            "max_tokens": 1,
            "messages": []
        }),
        UpstreamFormat::Google => serde_json::json!({
            "contents": []
        }),
    }
}

/// Probe upstream to discover which formats are supported (POST minimal body per format).
/// If response is not 404 and not a connection error, consider that format supported.
/// Returns supported set; caller uses UpstreamCapability::from_supported.
pub async fn discover_supported_formats(
    base_url: &str,
    timeout: std::time::Duration,
) -> HashSet<UpstreamFormat> {
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let mut supported = HashSet::new();
    for &format in &DEFAULT_TARGET_ORDER {
        let url = build_upstream_url(base_url, format, None);
        let body = minimal_probe_body(format);
        let res = client
            .post(&url)
            .json(&body)
            .send()
            .await;
        match res {
            Ok(r) => {
                let status = r.status();
                if status.as_u16() != 404 {
                    supported.insert(format);
                }
            }
            Err(_) => {}
        }
    }
    supported
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_fixed_single_format() {
        let cap = UpstreamCapability::fixed(UpstreamFormat::Anthropic);
        assert!(cap.supported.contains(&UpstreamFormat::Anthropic));
        assert_eq!(cap.default_target, UpstreamFormat::Anthropic);
        assert_eq!(cap.upstream_format_for_request(UpstreamFormat::Anthropic), UpstreamFormat::Anthropic);
        assert_eq!(cap.upstream_format_for_request(UpstreamFormat::OpenAiCompletion), UpstreamFormat::Anthropic);
    }

    #[test]
    fn capability_from_supported_default_target_order() {
        let mut supported = HashSet::new();
        supported.insert(UpstreamFormat::Google);
        supported.insert(UpstreamFormat::OpenAiCompletion);
        let cap = UpstreamCapability::from_supported(supported);
        assert_eq!(cap.default_target, UpstreamFormat::OpenAiCompletion);
        assert_eq!(cap.upstream_format_for_request(UpstreamFormat::OpenAiResponses), UpstreamFormat::OpenAiCompletion);
        assert_eq!(cap.upstream_format_for_request(UpstreamFormat::OpenAiCompletion), UpstreamFormat::OpenAiCompletion);
    }

    #[test]
    fn should_passthrough_when_client_in_supported() {
        let cap = UpstreamCapability::fixed(UpstreamFormat::OpenAiCompletion);
        assert!(cap.should_passthrough(UpstreamFormat::OpenAiCompletion));
        assert!(!cap.should_passthrough(UpstreamFormat::Anthropic));
    }

    #[test]
    fn from_supported_empty_defaults_to_openai_completion() {
        let cap = UpstreamCapability::from_supported(HashSet::new());
        assert_eq!(cap.default_target, UpstreamFormat::OpenAiCompletion);
        assert!(cap.supported.is_empty());
    }

    #[test]
    fn default_target_order_respects_genericity() {
        let mut supported = HashSet::new();
        supported.insert(UpstreamFormat::Google);
        supported.insert(UpstreamFormat::Anthropic);
        let cap = UpstreamCapability::from_supported(supported);
        assert_eq!(cap.default_target, UpstreamFormat::Anthropic);
    }
}
