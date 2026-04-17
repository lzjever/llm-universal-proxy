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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpstreamAvailability {
    Available,
    Unavailable { reason: String },
}

impl UpstreamAvailability {
    pub fn available() -> Self {
        Self::Available
    }

    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self::Unavailable {
            reason: reason.into(),
        }
    }

    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Available => None,
            Self::Unavailable { reason } => Some(reason.as_str()),
        }
    }

    pub fn status_label(&self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Unavailable { .. } => "unavailable",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiscoveredUpstream {
    pub capability: Option<UpstreamCapability>,
    pub availability: UpstreamAvailability,
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
    pub fn from_supported(supported: HashSet<UpstreamFormat>) -> Option<Self> {
        if supported.is_empty() {
            return None;
        }
        let default_target = DEFAULT_TARGET_ORDER
            .iter()
            .find(|f| supported.contains(f))
            .copied()
            .expect("supported formats are non-empty");
        Some(Self {
            supported,
            default_target,
        })
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

impl DiscoveredUpstream {
    pub fn fixed(format: UpstreamFormat) -> Self {
        Self {
            capability: Some(UpstreamCapability::fixed(format)),
            availability: UpstreamAvailability::available(),
        }
    }

    pub fn from_supported(supported: HashSet<UpstreamFormat>) -> Self {
        match UpstreamCapability::from_supported(supported) {
            Some(capability) => Self {
                capability: Some(capability),
                availability: UpstreamAvailability::available(),
            },
            None => Self {
                capability: None,
                availability: UpstreamAvailability::unavailable(
                    "protocol discovery returned no supported formats",
                ),
            },
        }
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
    api_key: Option<&str>,
    extra_headers: &[(String, String)],
) -> HashSet<UpstreamFormat> {
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let mut supported = HashSet::new();
    for &format in &DEFAULT_TARGET_ORDER {
        let url = build_upstream_url(base_url, format, None, false);
        let body = minimal_probe_body(format);
        let mut req = client.post(&url).json(&body);
        for (name, value) in default_headers_for_format(format) {
            req = req.header(name, value);
        }
        for (name, value) in extra_headers {
            req = req.header(name, value);
        }
        if let Some(key) = api_key {
            let (name, value) = auth_header_for_format(format, key);
            req = req.header(name, value);
        }
        if let Ok(r) = req.send().await {
            let status = r.status();
            if status_indicates_support(status.as_u16()) {
                supported.insert(format);
            }
        }
    }
    supported
}

fn default_headers_for_format(format: UpstreamFormat) -> Vec<(&'static str, &'static str)> {
    match format {
        UpstreamFormat::Anthropic => vec![("anthropic-version", "2023-06-01")],
        _ => Vec::new(),
    }
}

fn auth_header_for_format(format: UpstreamFormat, api_key: &str) -> (&'static str, String) {
    match format {
        UpstreamFormat::OpenAiCompletion | UpstreamFormat::OpenAiResponses => {
            ("authorization", format!("Bearer {}", api_key))
        }
        UpstreamFormat::Anthropic => ("x-api-key", api_key.to_string()),
        UpstreamFormat::Google => ("x-goog-api-key", api_key.to_string()),
    }
}

fn status_indicates_support(status: u16) -> bool {
    matches!(
        status,
        200..=299 | 400 | 401 | 403 | 405 | 406 | 409 | 415 | 422 | 429
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_fixed_single_format() {
        let cap = UpstreamCapability::fixed(UpstreamFormat::Anthropic);
        assert!(cap.supported.contains(&UpstreamFormat::Anthropic));
        assert_eq!(cap.default_target, UpstreamFormat::Anthropic);
        assert_eq!(
            cap.upstream_format_for_request(UpstreamFormat::Anthropic),
            UpstreamFormat::Anthropic
        );
        assert_eq!(
            cap.upstream_format_for_request(UpstreamFormat::OpenAiCompletion),
            UpstreamFormat::Anthropic
        );
    }

    #[test]
    fn capability_from_supported_default_target_order() {
        let mut supported = HashSet::new();
        supported.insert(UpstreamFormat::Google);
        supported.insert(UpstreamFormat::OpenAiCompletion);
        let cap = UpstreamCapability::from_supported(supported).expect("capability");
        assert_eq!(cap.default_target, UpstreamFormat::OpenAiCompletion);
        assert_eq!(
            cap.upstream_format_for_request(UpstreamFormat::OpenAiResponses),
            UpstreamFormat::OpenAiCompletion
        );
        assert_eq!(
            cap.upstream_format_for_request(UpstreamFormat::OpenAiCompletion),
            UpstreamFormat::OpenAiCompletion
        );
    }

    #[test]
    fn should_passthrough_when_client_in_supported() {
        let cap = UpstreamCapability::fixed(UpstreamFormat::OpenAiCompletion);
        assert!(cap.should_passthrough(UpstreamFormat::OpenAiCompletion));
        assert!(!cap.should_passthrough(UpstreamFormat::Anthropic));
    }

    #[test]
    fn from_supported_empty_returns_no_capability() {
        assert!(UpstreamCapability::from_supported(HashSet::new()).is_none());
    }

    #[test]
    fn default_target_order_respects_genericity() {
        let mut supported = HashSet::new();
        supported.insert(UpstreamFormat::Google);
        supported.insert(UpstreamFormat::Anthropic);
        let cap = UpstreamCapability::from_supported(supported).expect("capability");
        assert_eq!(cap.default_target, UpstreamFormat::Anthropic);
    }

    #[test]
    fn discovered_upstream_marks_empty_discovery_as_unavailable() {
        let discovered = DiscoveredUpstream::from_supported(HashSet::new());

        assert!(discovered.capability.is_none());
        assert_eq!(
            discovered.availability,
            UpstreamAvailability::Unavailable {
                reason: "protocol discovery returned no supported formats".to_string()
            }
        );
    }

    #[test]
    fn discovered_upstream_fixed_format_is_available() {
        let discovered = DiscoveredUpstream::fixed(UpstreamFormat::OpenAiResponses);

        assert!(discovered.availability.is_available());
        assert!(discovered
            .capability
            .as_ref()
            .expect("capability")
            .supported
            .contains(&UpstreamFormat::OpenAiResponses));
    }
}
