//! Configuration: YAML-backed multi-upstream routing, model aliases, and upstream URL building.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use serde::de::{self, value::MapAccessDeserializer, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::formats::UpstreamFormat;

mod model_surface;

pub use self::model_surface::{
    ApplyPatchTransport, CompatibilityMode, ModelModalities, ModelModality, ModelSurface,
    ModelSurfacePatch, ModelToolSurface,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookConfig {
    pub max_pending_bytes: usize,
    pub timeout: Duration,
    pub failure_threshold: usize,
    pub cooldown: Duration,
    pub exchange: Option<HookEndpointConfig>,
    pub usage: Option<HookEndpointConfig>,
}

impl HookConfig {
    pub fn is_enabled(&self) -> bool {
        self.exchange.is_some() || self.usage.is_some()
    }
}

impl Default for HookConfig {
    fn default() -> Self {
        Self {
            max_pending_bytes: default_hook_max_pending_bytes(),
            timeout: Duration::from_secs(default_hook_timeout_secs()),
            failure_threshold: default_hook_failure_threshold(),
            cooldown: Duration::from_secs(default_hook_cooldown_secs()),
            exchange: None,
            usage: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebugTraceConfig {
    pub path: Option<String>,
    pub max_text_chars: usize,
}

impl DebugTraceConfig {
    pub fn is_enabled(&self) -> bool {
        self.path
            .as_deref()
            .map(|path| !path.trim().is_empty())
            .unwrap_or(false)
    }
}

impl Default for DebugTraceConfig {
    fn default() -> Self {
        Self {
            path: None,
            max_text_chars: default_debug_trace_max_text_chars(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceLimits {
    #[serde(default = "default_max_request_body_bytes")]
    pub max_request_body_bytes: usize,
    #[serde(default = "default_max_non_stream_response_bytes")]
    pub max_non_stream_response_bytes: usize,
    #[serde(default = "default_max_upstream_error_body_bytes")]
    pub max_upstream_error_body_bytes: usize,
    #[serde(default = "default_max_sse_frame_bytes")]
    pub max_sse_frame_bytes: usize,
    #[serde(default = "default_stream_idle_timeout_secs")]
    pub stream_idle_timeout_secs: u64,
    #[serde(default = "default_stream_max_duration_secs")]
    pub stream_max_duration_secs: u64,
    #[serde(default = "default_stream_max_events")]
    pub stream_max_events: usize,
    #[serde(default = "default_max_accumulated_stream_state_bytes")]
    pub max_accumulated_stream_state_bytes: usize,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_request_body_bytes: default_max_request_body_bytes(),
            max_non_stream_response_bytes: default_max_non_stream_response_bytes(),
            max_upstream_error_body_bytes: default_max_upstream_error_body_bytes(),
            max_sse_frame_bytes: default_max_sse_frame_bytes(),
            stream_idle_timeout_secs: default_stream_idle_timeout_secs(),
            stream_max_duration_secs: default_stream_max_duration_secs(),
            stream_max_events: default_stream_max_events(),
            max_accumulated_stream_state_bytes: default_max_accumulated_stream_state_bytes(),
        }
    }
}

impl ResourceLimits {
    fn validate(&self) -> Result<(), String> {
        if self.max_request_body_bytes == 0 {
            return Err(
                "resource_limits.max_request_body_bytes must be greater than zero".to_string(),
            );
        }
        if self.max_non_stream_response_bytes == 0 {
            return Err(
                "resource_limits.max_non_stream_response_bytes must be greater than zero"
                    .to_string(),
            );
        }
        if self.max_upstream_error_body_bytes == 0 {
            return Err(
                "resource_limits.max_upstream_error_body_bytes must be greater than zero"
                    .to_string(),
            );
        }
        if self.max_sse_frame_bytes == 0 {
            return Err(
                "resource_limits.max_sse_frame_bytes must be greater than zero".to_string(),
            );
        }
        if self.stream_idle_timeout_secs == 0 {
            return Err(
                "resource_limits.stream_idle_timeout_secs must be greater than zero".to_string(),
            );
        }
        if self.stream_max_duration_secs == 0 {
            return Err(
                "resource_limits.stream_max_duration_secs must be greater than zero".to_string(),
            );
        }
        if self.stream_max_events == 0 {
            return Err("resource_limits.stream_max_events must be greater than zero".to_string());
        }
        if self.max_accumulated_stream_state_bytes == 0 {
            return Err(
                "resource_limits.max_accumulated_stream_state_bytes must be greater than zero"
                    .to_string(),
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookEndpointConfig {
    pub url: String,
    pub authorization: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyConfig {
    Direct,
    Proxy { url: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
enum ProxyConfigSerde {
    Direct(String),
    Proxy { url: String },
}

impl Serialize for ProxyConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Direct => ProxyConfigSerde::Direct("direct".to_string()).serialize(serializer),
            Self::Proxy { url } => {
                ProxyConfigSerde::Proxy { url: url.clone() }.serialize(serializer)
            }
        }
    }
}

impl<'de> Deserialize<'de> for ProxyConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(ProxyConfigVisitor)
    }
}

struct ProxyConfigVisitor;

impl<'de> Visitor<'de> for ProxyConfigVisitor {
    type Value = ProxyConfig;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("proxy config string `direct` or object with field `url`")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if value == "direct" {
            Ok(ProxyConfig::Direct)
        } else {
            Err(de::Error::custom(format!(
                "proxy config string must be `direct`, got `{value}`"
            )))
        }
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_str(&value)
    }

    fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut url = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "url" => {
                    if url.is_some() {
                        return Err(de::Error::duplicate_field("url"));
                    }
                    url = Some(map.next_value()?);
                }
                _ => return Err(de::Error::unknown_field(&key, &["url"])),
            }
        }
        let url = url.ok_or_else(|| de::Error::missing_field("url"))?;
        Ok(ProxyConfig::Proxy { url })
    }
}

impl ProxyConfig {
    fn validate(&self, owner: &str) -> Result<(), String> {
        let Self::Proxy { url } = self else {
            return Ok(());
        };
        if url.trim().is_empty() {
            return Err(format!("{owner} url must not be empty"));
        }
        let parsed = url::Url::parse(url)
            .map_err(|error| format!("{owner} url must be a valid absolute URL: {error}"))?;
        match parsed.scheme() {
            "http" | "https" | "socks5" | "socks5h" => {}
            scheme => {
                return Err(format!(
                    "{owner} url must use http, https, socks5, or socks5h, got `{scheme}`"
                ));
            }
        }
        if !url_has_explicit_authority(url) {
            return Err(format!("{owner} url must include a host"));
        }
        if parsed.host_str().is_none() {
            return Err(format!("{owner} url must include a host"));
        }
        Ok(())
    }
}

fn url_has_explicit_authority(value: &str) -> bool {
    let Some(scheme_end) = value.find("://") else {
        return false;
    };
    let authority_start = scheme_end + 3;
    value
        .as_bytes()
        .get(authority_start)
        .is_some_and(|byte| !matches!(byte, b'/' | b'?' | b'#'))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
}

impl ModelLimits {
    pub fn merged_with(&self, overrides: Option<&ModelLimits>) -> Option<ModelLimits> {
        let merged = ModelLimits {
            context_window: overrides
                .and_then(|item| item.context_window)
                .or(self.context_window),
            max_output_tokens: overrides
                .and_then(|item| item.max_output_tokens)
                .or(self.max_output_tokens),
        };
        if merged.context_window.is_none() && merged.max_output_tokens.is_none() {
            None
        } else {
            Some(merged)
        }
    }

    fn validate(&self, owner: &str) -> Result<(), String> {
        if self.context_window == Some(0) {
            return Err(format!(
                "{owner} limits.context_window must be greater than zero"
            ));
        }
        if self.max_output_tokens == Some(0) {
            return Err(format!(
                "{owner} limits.max_output_tokens must be greater than zero"
            ));
        }
        Ok(())
    }
}

/// Runtime configuration for one named upstream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpstreamConfig {
    /// Stable upstream name referenced by `upstream:model`.
    pub name: String,
    /// Official upstream API root including version suffix, for example `https://api.openai.com/v1`.
    pub api_root: String,
    /// Optional fixed upstream format. When unset, capability discovery is used.
    pub fixed_upstream_format: Option<UpstreamFormat>,
    /// Provider credential env var name used when the proxy owns upstream credentials.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_key_env: Option<String>,
    /// Optional static headers to inject into every upstream request.
    pub upstream_headers: Vec<(String, String)>,
    /// Optional per-upstream proxy override. When unset, the namespace default is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<ProxyConfig>,
    /// Optional default limits for models routed through this upstream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<ModelLimits>,
    /// Optional default client-visible surface for aliases on this upstream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_defaults: Option<ModelSurfacePatch>,
}

/// One local model alias that resolves to a named upstream and upstream model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelAlias {
    pub upstream_name: String,
    pub upstream_model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<ModelLimits>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<ModelSurfacePatch>,
}

/// Resolved model routing decision for one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedModel {
    pub upstream_name: String,
    pub upstream_model: String,
}

/// Runtime configuration for the proxy.
#[derive(Debug, Clone)]
pub struct Config {
    /// Listen address (for example `0.0.0.0:8080`).
    pub listen: String,
    /// Request timeout to upstream.
    pub upstream_timeout: Duration,
    /// Compatibility posture for translated request paths in this namespace.
    pub compatibility_mode: CompatibilityMode,
    /// Default upstream proxy policy for this namespace. When unset, environment proxy resolution is used.
    pub proxy: Option<ProxyConfig>,
    /// Named upstreams available to the proxy.
    pub upstreams: Vec<UpstreamConfig>,
    /// Local unique model names mapped to named upstream models.
    pub model_aliases: BTreeMap<String, ModelAlias>,
    /// Optional audit and metering hooks.
    pub hooks: HookConfig,
    /// Optional local debug trace sink.
    pub debug_trace: DebugTraceConfig,
    /// Resource boundaries for request bodies, upstream bodies, and streaming state.
    pub resource_limits: ResourceLimits,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeHookEndpointConfig {
    pub url: String,
    #[serde(default)]
    pub authorization: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeHookConfig {
    #[serde(default = "default_hook_max_pending_bytes")]
    pub max_pending_bytes: usize,
    #[serde(default = "default_hook_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_hook_failure_threshold")]
    pub failure_threshold: usize,
    #[serde(default = "default_hook_cooldown_secs")]
    pub cooldown_secs: u64,
    #[serde(default)]
    pub exchange: Option<RuntimeHookEndpointConfig>,
    #[serde(default)]
    pub usage: Option<RuntimeHookEndpointConfig>,
}

impl Default for RuntimeHookConfig {
    fn default() -> Self {
        Self {
            max_pending_bytes: default_hook_max_pending_bytes(),
            timeout_secs: default_hook_timeout_secs(),
            failure_threshold: default_hook_failure_threshold(),
            cooldown_secs: default_hook_cooldown_secs(),
            exchange: None,
            usage: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeUpstreamConfig {
    pub name: String,
    pub api_root: String,
    #[serde(default)]
    pub fixed_upstream_format: Option<UpstreamFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_key_env: Option<String>,
    #[serde(default)]
    pub upstream_headers: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<ProxyConfig>,
    #[serde(default)]
    pub limits: Option<ModelLimits>,
    #[serde(default)]
    pub surface_defaults: Option<ModelSurfacePatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfigPayload {
    #[serde(default = "default_listen")]
    pub listen: String,
    #[serde(default = "default_upstream_timeout_secs")]
    pub upstream_timeout_secs: u64,
    #[serde(default)]
    pub compatibility_mode: CompatibilityMode,
    #[serde(
        default,
        alias = "upstream_proxy",
        skip_serializing_if = "Option::is_none"
    )]
    pub proxy: Option<ProxyConfig>,
    #[serde(default)]
    pub upstreams: Vec<RuntimeUpstreamConfig>,
    #[serde(default)]
    pub model_aliases: BTreeMap<String, ModelAlias>,
    #[serde(default)]
    pub hooks: RuntimeHookConfig,
    #[serde(default)]
    pub debug_trace: DebugTraceConfig,
    #[serde(default)]
    pub resource_limits: ResourceLimits,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeConfigSnapshot {
    pub revision: String,
    pub config: RuntimeConfigPayload,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AdminModelAliasView {
    pub upstream_name: String,
    pub upstream_model: String,
    pub limits: Option<ModelLimits>,
    pub surface: Option<ModelSurfacePatch>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AdminHookEndpointView {
    pub url: String,
    pub authorization_configured: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AdminHeaderValueView {
    pub name: String,
    pub value: Option<String>,
    pub value_redacted: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AdminHookConfigView {
    pub max_pending_bytes: usize,
    pub timeout_secs: u64,
    pub failure_threshold: usize,
    pub cooldown_secs: u64,
    pub exchange: Option<AdminHookEndpointView>,
    pub usage: Option<AdminHookEndpointView>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AdminDebugTraceConfigView {
    pub path: Option<String>,
    pub max_text_chars: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AdminUpstreamConfigView {
    pub name: String,
    pub api_root: String,
    pub fixed_upstream_format: Option<UpstreamFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_key_env: Option<String>,
    pub upstream_headers: Vec<AdminHeaderValueView>,
    pub proxy: Option<ProxyConfig>,
    pub limits: Option<ModelLimits>,
    pub surface_defaults: Option<ModelSurfacePatch>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AdminConfigView {
    pub listen: String,
    pub upstream_timeout_secs: u64,
    pub compatibility_mode: CompatibilityMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy: Option<ProxyConfig>,
    pub upstreams: Vec<AdminUpstreamConfigView>,
    pub model_aliases: BTreeMap<String, AdminModelAliasView>,
    pub hooks: AdminHookConfigView,
    pub debug_trace: AdminDebugTraceConfigView,
    pub resource_limits: ResourceLimits,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    #[serde(default = "default_listen")]
    listen: String,
    #[serde(default = "default_upstream_timeout_secs")]
    upstream_timeout_secs: u64,
    #[serde(default)]
    compatibility_mode: CompatibilityMode,
    #[serde(default, alias = "upstream_proxy")]
    proxy: Option<ProxyConfig>,
    #[serde(default)]
    upstreams: BTreeMap<String, UpstreamConfigFile>,
    #[serde(default)]
    model_aliases: BTreeMap<String, ModelAliasFile>,
    #[serde(default)]
    hooks: HooksFileConfig,
    #[serde(default)]
    debug_trace: DebugTraceConfig,
    #[serde(default)]
    resource_limits: ResourceLimits,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpstreamConfigFile {
    #[serde(alias = "url", alias = "upstream_url", alias = "base_url")]
    api_root: String,
    #[serde(default, alias = "upstream_format", alias = "format")]
    fixed_upstream_format: Option<UpstreamFormat>,
    #[serde(default)]
    provider_key_env: Option<String>,
    #[serde(default, alias = "headers", alias = "upstream_headers")]
    upstream_headers: BTreeMap<String, String>,
    #[serde(default)]
    proxy: Option<ProxyConfig>,
    #[serde(default)]
    limits: Option<ModelLimits>,
    #[serde(default)]
    surface_defaults: Option<ModelSurfacePatch>,
}

#[derive(Debug, Clone)]
enum ModelAliasFile {
    Target(String),
    Structured(StructuredModelAliasFile),
}

impl<'de> Deserialize<'de> for ModelAliasFile {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(ModelAliasFileVisitor)
    }
}

struct ModelAliasFileVisitor;

impl<'de> Visitor<'de> for ModelAliasFileVisitor {
    type Value = ModelAliasFile;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("model alias target string or object with field `target`")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(ModelAliasFile::Target(value.to_string()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(ModelAliasFile::Target(value))
    }

    fn visit_map<M>(self, map: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        StructuredModelAliasFile::deserialize(MapAccessDeserializer::new(map))
            .map(ModelAliasFile::Structured)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct StructuredModelAliasFile {
    target: String,
    #[serde(default)]
    limits: Option<ModelLimits>,
    #[serde(default)]
    surface: Option<ModelSurfacePatch>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct HooksFileConfig {
    #[serde(default = "default_hook_max_pending_bytes")]
    max_pending_bytes: usize,
    #[serde(default = "default_hook_timeout_secs")]
    timeout_secs: u64,
    #[serde(default = "default_hook_failure_threshold")]
    failure_threshold: usize,
    #[serde(default = "default_hook_cooldown_secs")]
    cooldown_secs: u64,
    #[serde(default)]
    exchange: Option<HookEndpointConfigFile>,
    #[serde(default)]
    usage: Option<HookEndpointConfigFile>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct HookEndpointConfigFile {
    url: String,
    #[serde(default)]
    authorization: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: default_listen(),
            upstream_timeout: Duration::from_secs(default_upstream_timeout_secs()),
            compatibility_mode: CompatibilityMode::default(),
            proxy: None,
            upstreams: Vec::new(),
            model_aliases: BTreeMap::new(),
            hooks: HookConfig::default(),
            debug_trace: DebugTraceConfig::default(),
            resource_limits: ResourceLimits::default(),
        }
    }
}

impl Config {
    /// Load config from a YAML file.
    pub fn from_yaml_path(path: impl AsRef<Path>) -> Result<Self, String> {
        let path_ref = path.as_ref();
        let raw = std::fs::read_to_string(path_ref)
            .map_err(|e| format!("failed to read config {}: {}", path_ref.display(), e))?;
        Self::from_yaml_str(&raw)
            .map_err(|e| format!("invalid config {}: {}", path_ref.display(), e))
    }

    /// Load config from YAML text. Intended for tests.
    pub fn from_yaml_str(raw: &str) -> Result<Self, String> {
        let parsed: FileConfig =
            serde_yaml::from_str(raw).map_err(|e| format!("failed to parse YAML config: {e}"))?;

        let upstreams = parsed
            .upstreams
            .into_iter()
            .map(|(name, item)| UpstreamConfig {
                name,
                api_root: item.api_root,
                fixed_upstream_format: item.fixed_upstream_format,
                provider_key_env: item.provider_key_env,
                upstream_headers: item.upstream_headers.into_iter().collect(),
                proxy: item.proxy,
                limits: item.limits,
                surface_defaults: item.surface_defaults,
            })
            .collect();

        let model_aliases = parsed
            .model_aliases
            .into_iter()
            .filter_map(|(alias, item)| {
                let (target, limits, surface) = match item {
                    ModelAliasFile::Target(target) => (target, None, None),
                    ModelAliasFile::Structured(item) => (item.target, item.limits, item.surface),
                };
                let (upstream_name, upstream_model) = target.split_once(':')?;
                Some((
                    alias,
                    ModelAlias {
                        upstream_name: upstream_name.to_string(),
                        upstream_model: upstream_model.to_string(),
                        limits,
                        surface,
                    },
                ))
            })
            .collect();

        Ok(Self {
            listen: parsed.listen,
            upstream_timeout: Duration::from_secs(parsed.upstream_timeout_secs),
            compatibility_mode: parsed.compatibility_mode,
            proxy: parsed.proxy,
            upstreams,
            model_aliases,
            hooks: HookConfig {
                max_pending_bytes: parsed.hooks.max_pending_bytes,
                timeout: Duration::from_secs(parsed.hooks.timeout_secs),
                failure_threshold: parsed.hooks.failure_threshold,
                cooldown: Duration::from_secs(parsed.hooks.cooldown_secs),
                exchange: parsed.hooks.exchange.map(|hook| HookEndpointConfig {
                    url: hook.url,
                    authorization: hook.authorization,
                }),
                usage: parsed.hooks.usage.map(|hook| HookEndpointConfig {
                    url: hook.url,
                    authorization: hook.authorization,
                }),
            },
            debug_trace: parsed.debug_trace,
            resource_limits: parsed.resource_limits,
        })
    }

    /// Validate config and return a descriptive error for invalid settings.
    pub fn validate(&self) -> Result<(), String> {
        if self.upstreams.is_empty() {
            return Err("at least one upstream must be configured".to_string());
        }
        self.resource_limits.validate()?;
        if let Some(proxy) = &self.proxy {
            proxy.validate("proxy")?;
        }

        let mut seen = BTreeSet::new();
        for upstream in &self.upstreams {
            if upstream.name.trim().is_empty() {
                return Err("upstream name must not be empty".to_string());
            }
            if upstream.api_root.trim().is_empty() {
                return Err(format!(
                    "upstream `{}` api_root must not be empty",
                    upstream.name
                ));
            }
            let parsed = url::Url::parse(&upstream.api_root).map_err(|error| {
                format!(
                    "upstream `{}` api_root must be a valid absolute URL: {}",
                    upstream.name, error
                )
            })?;
            match parsed.scheme() {
                "http" | "https" => {}
                scheme => {
                    return Err(format!(
                        "upstream `{}` api_root must use http or https, got `{}`",
                        upstream.name, scheme
                    ));
                }
            }
            if parsed.host_str().is_none() {
                return Err(format!(
                    "upstream `{}` api_root must include a host",
                    upstream.name
                ));
            }
            if url_has_userinfo(&parsed) {
                return Err(format!(
                    "upstream `{}` api_root must not include userinfo",
                    upstream.name
                ));
            }
            if upstream
                .provider_key_env
                .as_deref()
                .is_some_and(|value| value.trim().is_empty())
            {
                return Err(format!(
                    "upstream `{}` provider_key_env must not be empty",
                    upstream.name
                ));
            }
            for (header_name, _) in &upstream.upstream_headers {
                if is_forbidden_upstream_header_name(header_name) {
                    return Err(format!(
                        "upstream `{}` upstream_headers must not override forbidden auth/secret header `{}`",
                        upstream.name, header_name
                    ));
                }
            }
            if !seen.insert(upstream.name.clone()) {
                return Err(format!("duplicate upstream name `{}`", upstream.name));
            }
            if let Some(proxy) = &upstream.proxy {
                proxy.validate(&format!("upstream `{}` proxy", upstream.name))?;
            }
            if let Some(limits) = &upstream.limits {
                limits.validate(&format!("upstream `{}`", upstream.name))?;
            }
            if let Some(surface_defaults) = &upstream.surface_defaults {
                surface_defaults.validate(&format!("upstream `{}`", upstream.name))?;
            }
        }

        self.validate_hook("exchange", self.hooks.exchange.as_ref())?;
        self.validate_hook("usage", self.hooks.usage.as_ref())?;
        if self.hooks.is_enabled() {
            if self.hooks.timeout.is_zero() {
                return Err("hook timeout must be greater than zero".to_string());
            }
            if self.hooks.failure_threshold == 0 {
                return Err("hook failure_threshold must be greater than zero".to_string());
            }
            if self.hooks.cooldown.is_zero() {
                return Err("hook cooldown must be greater than zero".to_string());
            }
        }

        for (alias, target) in &self.model_aliases {
            if alias.trim().is_empty() {
                return Err("model alias name must not be empty".to_string());
            }
            if self.upstream(&target.upstream_name).is_none() {
                return Err(format!(
                    "model alias `{}` references unknown upstream `{}`",
                    alias, target.upstream_name
                ));
            }
            if target.upstream_model.trim().is_empty() {
                return Err(format!(
                    "model alias `{alias}` must point to a non-empty upstream model"
                ));
            }
            if let Some(limits) = &target.limits {
                limits.validate(&format!("model alias `{alias}`"))?;
            }
            if let Some(surface) = &target.surface {
                surface.validate(&format!("model alias `{alias}`"))?;
            }
        }

        Ok(())
    }

    fn validate_hook(
        &self,
        hook_name: &str,
        hook: Option<&HookEndpointConfig>,
    ) -> Result<(), String> {
        let Some(hook) = hook else {
            return Ok(());
        };
        if hook.url.trim().is_empty() {
            return Err(format!("{hook_name} hook url must not be empty"));
        }
        let parsed = url::Url::parse(&hook.url)
            .map_err(|e| format!("{hook_name} hook url is invalid: {e}"))?;
        match parsed.scheme() {
            "http" | "https" => {}
            scheme => {
                return Err(format!(
                    "{hook_name} hook url must use http or https, got `{scheme}`"
                ));
            }
        }
        if url_has_userinfo(&parsed) {
            return Err(format!("{hook_name} hook url must not include userinfo"));
        }
        Ok(())
    }

    pub fn upstream(&self, name: &str) -> Option<&UpstreamConfig> {
        self.upstreams.iter().find(|u| u.name == name)
    }

    pub fn effective_model_limits(&self, alias: &ModelAlias) -> Option<ModelLimits> {
        let upstream_limits = self
            .upstream(&alias.upstream_name)
            .and_then(|item| item.limits.clone());
        match upstream_limits {
            Some(base) => base.merged_with(alias.limits.as_ref()),
            None => alias.limits.clone(),
        }
    }

    pub fn effective_model_surface(&self, alias: &ModelAlias) -> ModelSurface {
        let merged_surface = self
            .upstream(&alias.upstream_name)
            .and_then(|item| item.surface_defaults.clone())
            .unwrap_or_default()
            .merged_with(alias.surface.as_ref());
        ModelSurface {
            limits: self.effective_model_limits(alias),
            modalities: merged_surface.modalities,
            tools: merged_surface.tools,
        }
    }

    /// Resolve a client-visible model string to a named upstream and upstream model.
    pub fn resolve_model(&self, requested_model: &str) -> Result<ResolvedModel, String> {
        if let Some((upstream_name, upstream_model)) = requested_model.split_once(':') {
            if self.upstream(upstream_name).is_some() {
                if upstream_model.trim().is_empty() {
                    return Err(format!(
                        "model `{requested_model}` must include a non-empty upstream model after `:`"
                    ));
                }
                return Ok(ResolvedModel {
                    upstream_name: upstream_name.to_string(),
                    upstream_model: upstream_model.to_string(),
                });
            }
        }

        if let Some(alias) = self.model_aliases.get(requested_model) {
            return Ok(ResolvedModel {
                upstream_name: alias.upstream_name.clone(),
                upstream_model: alias.upstream_model.clone(),
            });
        }

        if self.upstreams.len() == 1 {
            return Ok(ResolvedModel {
                upstream_name: self.upstreams[0].name.clone(),
                upstream_model: requested_model.to_string(),
            });
        }

        Err(format!(
            "model `{requested_model}` is ambiguous; use `upstream:model` or configure model_aliases"
        ))
    }

    /// Build the full POST URL for one upstream format.
    pub fn upstream_url_for_format(
        &self,
        upstream: &UpstreamConfig,
        format: UpstreamFormat,
        model: Option<&str>,
        stream: bool,
    ) -> String {
        build_upstream_url(&upstream.api_root, format, model, stream)
    }
}

impl TryFrom<RuntimeConfigPayload> for Config {
    type Error = String;

    fn try_from(value: RuntimeConfigPayload) -> Result<Self, Self::Error> {
        let upstreams = value
            .upstreams
            .into_iter()
            .map(|item| UpstreamConfig {
                name: item.name,
                api_root: item.api_root,
                fixed_upstream_format: item.fixed_upstream_format,
                provider_key_env: item.provider_key_env,
                upstream_headers: item.upstream_headers,
                proxy: item.proxy,
                limits: item.limits,
                surface_defaults: item.surface_defaults,
            })
            .collect::<Vec<_>>();

        let config = Self {
            listen: value.listen,
            upstream_timeout: Duration::from_secs(value.upstream_timeout_secs),
            compatibility_mode: value.compatibility_mode,
            proxy: value.proxy,
            upstreams,
            model_aliases: value.model_aliases,
            hooks: HookConfig {
                max_pending_bytes: value.hooks.max_pending_bytes,
                timeout: Duration::from_secs(value.hooks.timeout_secs),
                failure_threshold: value.hooks.failure_threshold,
                cooldown: Duration::from_secs(value.hooks.cooldown_secs),
                exchange: value.hooks.exchange.map(|hook| HookEndpointConfig {
                    url: hook.url,
                    authorization: hook.authorization,
                }),
                usage: value.hooks.usage.map(|hook| HookEndpointConfig {
                    url: hook.url,
                    authorization: hook.authorization,
                }),
            },
            debug_trace: value.debug_trace,
            resource_limits: value.resource_limits,
        };
        config.validate()?;
        Ok(config)
    }
}

impl From<&Config> for RuntimeConfigPayload {
    fn from(value: &Config) -> Self {
        Self {
            listen: value.listen.clone(),
            upstream_timeout_secs: value.upstream_timeout.as_secs(),
            compatibility_mode: value.compatibility_mode,
            proxy: value.proxy.clone(),
            upstreams: value
                .upstreams
                .iter()
                .map(|item| RuntimeUpstreamConfig {
                    name: item.name.clone(),
                    api_root: item.api_root.clone(),
                    fixed_upstream_format: item.fixed_upstream_format,
                    provider_key_env: item.provider_key_env.clone(),
                    upstream_headers: item.upstream_headers.clone(),
                    proxy: item.proxy.clone(),
                    limits: item.limits.clone(),
                    surface_defaults: item.surface_defaults.clone(),
                })
                .collect(),
            model_aliases: value.model_aliases.clone(),
            hooks: RuntimeHookConfig {
                max_pending_bytes: value.hooks.max_pending_bytes,
                timeout_secs: value.hooks.timeout.as_secs(),
                failure_threshold: value.hooks.failure_threshold,
                cooldown_secs: value.hooks.cooldown.as_secs(),
                exchange: value
                    .hooks
                    .exchange
                    .as_ref()
                    .map(|hook| RuntimeHookEndpointConfig {
                        url: hook.url.clone(),
                        authorization: hook.authorization.clone(),
                    }),
                usage: value
                    .hooks
                    .usage
                    .as_ref()
                    .map(|hook| RuntimeHookEndpointConfig {
                        url: hook.url.clone(),
                        authorization: hook.authorization.clone(),
                    }),
            },
            debug_trace: value.debug_trace.clone(),
            resource_limits: value.resource_limits.clone(),
        }
    }
}

impl From<&Config> for AdminConfigView {
    fn from(value: &Config) -> Self {
        Self {
            listen: value.listen.clone(),
            upstream_timeout_secs: value.upstream_timeout.as_secs(),
            compatibility_mode: value.compatibility_mode,
            proxy: value.proxy.as_ref().map(sanitize_proxy_config_for_admin),
            upstreams: value
                .upstreams
                .iter()
                .map(|item| AdminUpstreamConfigView {
                    name: item.name.clone(),
                    api_root: sanitize_url_for_admin(&item.api_root),
                    fixed_upstream_format: item.fixed_upstream_format,
                    provider_key_env: item.provider_key_env.clone(),
                    upstream_headers: item
                        .upstream_headers
                        .iter()
                        .map(|(name, value)| admin_header_view(name, value))
                        .collect(),
                    proxy: item.proxy.as_ref().map(sanitize_proxy_config_for_admin),
                    limits: item.limits.clone(),
                    surface_defaults: item.surface_defaults.clone(),
                })
                .collect(),
            model_aliases: value
                .model_aliases
                .iter()
                .map(|(alias, model)| {
                    (
                        alias.clone(),
                        AdminModelAliasView {
                            upstream_name: model.upstream_name.clone(),
                            upstream_model: model.upstream_model.clone(),
                            limits: model.limits.clone(),
                            surface: model.surface.clone(),
                        },
                    )
                })
                .collect(),
            hooks: AdminHookConfigView {
                max_pending_bytes: value.hooks.max_pending_bytes,
                timeout_secs: value.hooks.timeout.as_secs(),
                failure_threshold: value.hooks.failure_threshold,
                cooldown_secs: value.hooks.cooldown.as_secs(),
                exchange: value
                    .hooks
                    .exchange
                    .as_ref()
                    .map(|hook| AdminHookEndpointView {
                        url: sanitize_url_for_admin(&hook.url),
                        authorization_configured: hook.authorization.is_some(),
                    }),
                usage: value
                    .hooks
                    .usage
                    .as_ref()
                    .map(|hook| AdminHookEndpointView {
                        url: sanitize_url_for_admin(&hook.url),
                        authorization_configured: hook.authorization.is_some(),
                    }),
            },
            debug_trace: AdminDebugTraceConfigView {
                path: value.debug_trace.path.clone(),
                max_text_chars: value.debug_trace.max_text_chars,
            },
            resource_limits: value.resource_limits.clone(),
        }
    }
}

fn sanitize_proxy_config_for_admin(value: &ProxyConfig) -> ProxyConfig {
    match value {
        ProxyConfig::Direct => ProxyConfig::Direct,
        ProxyConfig::Proxy { url } => ProxyConfig::Proxy {
            url: sanitize_url_for_admin(url),
        },
    }
}

pub(crate) fn sanitize_url_for_admin(value: &str) -> String {
    if let Ok(mut url) = url::Url::parse(value) {
        let _ = url.set_username("");
        let _ = url.set_password(None);
        url.set_query(None);
        url.set_fragment(None);
        return url.to_string();
    }

    let value = strip_url_query_and_fragment(value);
    if let Some(scheme_end) = value.find("://") {
        let authority_start = scheme_end + 3;
        let authority_end = value[authority_start..]
            .find(['/', '?', '#'])
            .map(|offset| authority_start + offset)
            .unwrap_or(value.len());
        if let Some(at_offset) = value[authority_start..authority_end].find('@') {
            let at_index = authority_start + at_offset;
            return format!("{}{}", &value[..authority_start], &value[at_index + 1..]);
        }
    }

    value.to_string()
}

fn strip_url_query_and_fragment(value: &str) -> &str {
    let cutoff = value.find(['?', '#']).unwrap_or(value.len());
    &value[..cutoff]
}

fn admin_header_view(name: &str, value: &str) -> AdminHeaderValueView {
    if is_sensitive_header_name(name) {
        AdminHeaderValueView {
            name: name.to_string(),
            value: None,
            value_redacted: true,
        }
    } else {
        AdminHeaderValueView {
            name: name.to_string(),
            value: Some(value.to_string()),
            value_redacted: false,
        }
    }
}

pub(crate) fn is_sensitive_header_name(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
    normalized == "authorization"
        || normalized == "proxy-authorization"
        || normalized == "cookie"
        || normalized == "set-cookie"
        || normalized == "x-api-key"
        || normalized == "api-key"
        || normalized == "apikey"
        || normalized.contains("api-key")
        || normalized.contains("apikey")
        || normalized.contains("token")
        || normalized.contains("secret")
        || normalized.contains("credential")
}

pub(crate) fn is_forbidden_upstream_header_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization"
            | "proxy-authorization"
            | "x-api-key"
            | "api-key"
            | "openai-api-key"
            | "x-goog-api-key"
            | "anthropic-api-key"
    )
}

fn default_listen() -> String {
    "0.0.0.0:8080".to_string()
}

fn default_upstream_timeout_secs() -> u64 {
    120
}

fn default_hook_timeout_secs() -> u64 {
    30
}

fn default_hook_max_pending_bytes() -> usize {
    100 * 1024 * 1024
}

fn default_hook_failure_threshold() -> usize {
    3
}

fn default_hook_cooldown_secs() -> u64 {
    300
}

fn default_debug_trace_max_text_chars() -> usize {
    16384
}

fn default_max_request_body_bytes() -> usize {
    16 * 1024 * 1024
}

fn default_max_non_stream_response_bytes() -> usize {
    64 * 1024 * 1024
}

fn default_max_upstream_error_body_bytes() -> usize {
    64 * 1024
}

fn default_max_sse_frame_bytes() -> usize {
    1024 * 1024
}

fn default_stream_idle_timeout_secs() -> u64 {
    300
}

fn default_stream_max_duration_secs() -> u64 {
    3600
}

fn default_stream_max_events() -> usize {
    100_000
}

fn default_max_accumulated_stream_state_bytes() -> usize {
    8 * 1024 * 1024
}

/// Build full upstream POST URL for a format.
///
/// Best practice is to configure the official API root with an explicit version suffix:
/// - OpenAI: `https://api.openai.com/v1`
/// - Anthropic: `https://api.anthropic.com/v1`
/// - Google: `https://generativelanguage.googleapis.com/v1beta`
pub fn build_upstream_url(
    api_root: &str,
    format: UpstreamFormat,
    model: Option<&str>,
    stream: bool,
) -> String {
    let base = api_root.trim_end_matches('/');
    match format {
        UpstreamFormat::OpenAiCompletion => format!("{base}/chat/completions"),
        UpstreamFormat::OpenAiResponses => format!("{base}/responses"),
        UpstreamFormat::Anthropic => format!("{base}/messages"),
        UpstreamFormat::Google => {
            let model = model.filter(|s| !s.is_empty()).unwrap_or("gemini-1.5");
            if stream {
                format!("{base}/models/{model}:streamGenerateContent?alt=sse")
            } else {
                format!("{base}/models/{model}:generateContent")
            }
        }
    }
}

fn url_has_userinfo(url: &url::Url) -> bool {
    !url.username().is_empty() || url.password().is_some()
}

/// Build a full resource URL for auxiliary upstream endpoints.
pub fn build_upstream_resource_url(api_root: &str, resource_path: &str) -> String {
    let base = api_root.trim_end_matches('/');
    let path = resource_path.trim_start_matches('/');
    format!("{base}/{path}")
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
                true
            ),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.0-flash:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn build_upstream_resource_url_trims_slashes() {
        assert_eq!(
            build_upstream_resource_url("https://api.openai.com/v1/", "/responses/resp_1/cancel"),
            "https://api.openai.com/v1/responses/resp_1/cancel"
        );
    }

    #[test]
    fn build_upstream_url_handles_versioned_roots() {
        assert_eq!(
            build_upstream_url(
                "https://api.openai.com/v1",
                UpstreamFormat::OpenAiCompletion,
                None,
                false
            ),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            build_upstream_url(
                "https://api.anthropic.com/v1/",
                UpstreamFormat::Anthropic,
                None,
                false
            ),
            "https://api.anthropic.com/v1/messages"
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
                "https://open.bigmodel.cn/api/paas/v4",
                UpstreamFormat::OpenAiCompletion,
                None,
                false
            ),
            "https://open.bigmodel.cn/api/paas/v4/chat/completions"
        );
    }

    #[test]
    fn config_from_yaml_str_parses_multi_upstream_and_aliases() {
        let c = Config::from_yaml_str(
            r#"
listen: 127.0.0.1:8080
upstream_timeout_secs: 45
upstreams:
  GLM-OFFICIAL:
    api_root: https://open.bigmodel.cn/api/anthropic/v1
    format: anthropic
    provider_key_env: GLM_APIKEY
  OPENAI:
    api_root: https://api.openai.com/v1
    format: openai-responses
model_aliases:
  GLM-5: GLM-OFFICIAL:GLM-5
"#,
        )
        .unwrap();

        assert_eq!(c.listen, "127.0.0.1:8080");
        assert_eq!(c.upstream_timeout.as_secs(), 45);
        assert_eq!(c.upstreams.len(), 2);
        let glm = c.upstream("GLM-OFFICIAL").unwrap();
        assert_eq!(glm.api_root, "https://open.bigmodel.cn/api/anthropic/v1");
        assert_eq!(glm.fixed_upstream_format, Some(UpstreamFormat::Anthropic));
        assert_eq!(glm.provider_key_env.as_deref(), Some("GLM_APIKEY"));
        let alias = c.model_aliases.get("GLM-5").unwrap();
        assert_eq!(alias.upstream_name, "GLM-OFFICIAL");
        assert_eq!(alias.upstream_model, "GLM-5");
    }

    #[test]
    fn config_from_yaml_str_parses_namespace_and_upstream_proxy_config() {
        let c = Config::from_yaml_str(
            r#"
proxy:
  url: socks5h://global-user:global-pass@global-proxy.example:1080/global-hop?token=secret#frag
upstreams:
  demo:
    api_root: https://api.openai.com/v1
    format: openai-completion
    proxy: direct
  proxied:
    api_root: https://api.anthropic.com/v1
    format: anthropic
    proxy:
      url: https://up-user:up-pass@regional-proxy.example:8443/egress?sig=secret#frag
"#,
        )
        .unwrap();

        assert_eq!(
            c.proxy,
            Some(ProxyConfig::Proxy {
                url: "socks5h://global-user:global-pass@global-proxy.example:1080/global-hop?token=secret#frag".to_string(),
            })
        );
        assert_eq!(c.upstream("demo").unwrap().proxy, Some(ProxyConfig::Direct));
        assert_eq!(
            c.upstream("proxied").unwrap().proxy,
            Some(ProxyConfig::Proxy {
                url: "https://up-user:up-pass@regional-proxy.example:8443/egress?sig=secret#frag"
                    .to_string(),
            })
        );
    }

    #[test]
    fn config_defaults_proxy_to_inherit_env_when_omitted() {
        let c = Config::from_yaml_str(
            r#"
upstreams:
  demo:
    api_root: https://api.openai.com/v1
    format: openai-completion
"#,
        )
        .unwrap();

        assert_eq!(Config::default().proxy, None);
        assert_eq!(c.proxy, None);
        assert_eq!(c.upstream("demo").unwrap().proxy, None);
    }

    #[test]
    fn runtime_config_round_trip_preserves_optional_proxy_layers() {
        let payload = RuntimeConfigPayload {
            listen: "127.0.0.1:0".to_string(),
            upstream_timeout_secs: 30,
            compatibility_mode: CompatibilityMode::Balanced,
            proxy: Some(ProxyConfig::Direct),
            upstreams: vec![RuntimeUpstreamConfig {
                name: "default".to_string(),
                api_root: "https://api.openai.com/v1".to_string(),
                fixed_upstream_format: Some(UpstreamFormat::OpenAiCompletion),
                provider_key_env: None,
                upstream_headers: Vec::new(),
                proxy: Some(ProxyConfig::Proxy {
                    url: "http://regional-proxy.example:8080".to_string(),
                }),
                limits: None,
                surface_defaults: None,
            }],
            model_aliases: BTreeMap::new(),
            hooks: RuntimeHookConfig::default(),
            debug_trace: DebugTraceConfig::default(),
            resource_limits: ResourceLimits {
                max_request_body_bytes: 4096,
                max_non_stream_response_bytes: 8192,
                max_upstream_error_body_bytes: 1024,
                max_sse_frame_bytes: 2048,
                stream_idle_timeout_secs: 7,
                stream_max_duration_secs: 11,
                stream_max_events: 13,
                max_accumulated_stream_state_bytes: 16384,
            },
        };

        let config = Config::try_from(payload).unwrap();
        let round_trip = RuntimeConfigPayload::from(&config);

        assert_eq!(round_trip.proxy, Some(ProxyConfig::Direct));
        assert_eq!(
            round_trip.upstreams[0].proxy,
            Some(ProxyConfig::Proxy {
                url: "http://regional-proxy.example:8080".to_string(),
            })
        );
        assert_eq!(round_trip.resource_limits.max_request_body_bytes, 4096);
        assert_eq!(round_trip.resource_limits.max_sse_frame_bytes, 2048);
        assert_eq!(round_trip.resource_limits.stream_max_events, 13);
    }

    #[test]
    fn config_defaults_compatibility_mode_to_max_compat() {
        assert_eq!(CompatibilityMode::default(), CompatibilityMode::MaxCompat);
        assert_eq!(
            Config::default().compatibility_mode,
            CompatibilityMode::MaxCompat
        );
    }

    #[test]
    fn config_from_yaml_str_defaults_compatibility_mode_when_omitted() {
        let c = Config::from_yaml_str(
            r#"
upstreams:
  demo:
    api_root: https://api.openai.com/v1
    format: openai-completion
"#,
        )
        .unwrap();

        assert_eq!(c.compatibility_mode, CompatibilityMode::MaxCompat);
    }

    #[test]
    fn config_from_yaml_requires_upstreams() {
        let c = Config::from_yaml_str("listen: 127.0.0.1:8080").unwrap();
        assert!(c.validate().is_err());
    }

    #[test]
    fn config_from_yaml_rejects_legacy_upstream_auth_fields() {
        for field in [
            "credential_env: OLD_PROVIDER_KEY",
            "fallback_credential_env: OLD_PROVIDER_KEY",
            "credential_actual: inline-secret",
            "fallback_credential_actual: inline-secret",
            "fallback_api_key: inline-secret",
            "auth_policy: force_server",
        ] {
            let error = Config::from_yaml_str(&format!(
                r#"
upstreams:
  demo:
    api_root: https://api.openai.com/v1
    format: openai-completion
    {field}
"#
            ))
            .expect_err("legacy upstream auth field must fail parsing");
            let field_name = field.split(':').next().unwrap();
            assert!(
                error.contains(field_name),
                "error should name legacy field `{field_name}`: {error}"
            );
        }
    }

    #[test]
    fn runtime_config_payload_rejects_legacy_upstream_auth_fields() {
        for field in [
            "fallback_credential_env",
            "fallback_credential_actual",
            "auth_policy",
        ] {
            let mut upstream = serde_json::json!({
                "name": "default",
                "api_root": "https://api.openai.com/v1",
                "fixed_upstream_format": "openai-completion"
            });
            upstream[field] = serde_json::json!("legacy");
            let payload = serde_json::json!({
                "listen": "127.0.0.1:0",
                "upstreams": [upstream]
            });
            let error = serde_json::from_value::<RuntimeConfigPayload>(payload)
                .expect_err("legacy runtime upstream auth field must fail JSON parsing");
            assert!(
                error.to_string().contains(field),
                "error should name legacy field `{field}`: {error}"
            );
        }
    }

    #[test]
    fn runtime_config_payload_rejects_unknown_nested_fields() {
        let base_payload = || {
            serde_json::json!({
                "listen": "127.0.0.1:0",
                "proxy": "direct",
                "upstreams": [{
                    "name": "default",
                    "api_root": "https://api.openai.com/v1",
                    "fixed_upstream_format": "openai-completion"
                }],
                "hooks": {
                    "exchange": {
                        "url": "https://example.com/hooks/exchange"
                    }
                },
                "debug_trace": {
                    "path": null,
                    "max_text_chars": 16384
                },
                "resource_limits": {
                    "max_request_body_bytes": 4096,
                    "max_non_stream_response_bytes": 8192,
                    "max_upstream_error_body_bytes": 1024,
                    "max_sse_frame_bytes": 2048,
                    "stream_idle_timeout_secs": 7,
                    "stream_max_duration_secs": 11,
                    "stream_max_events": 13,
                    "max_accumulated_stream_state_bytes": 16384
                }
            })
        };

        for (case, field, payload) in [
            {
                let mut payload = base_payload();
                payload["proxy"] = serde_json::json!({
                    "url": "http://proxy.example:8080",
                    "future_proxy_option": true
                });
                ("proxy", "future_proxy_option", payload)
            },
            {
                let mut payload = base_payload();
                payload["hooks"]["future_hook_option"] = serde_json::json!(true);
                ("hooks", "future_hook_option", payload)
            },
            {
                let mut payload = base_payload();
                payload["hooks"]["exchange"]["future_endpoint_option"] = serde_json::json!(true);
                ("hook endpoint", "future_endpoint_option", payload)
            },
            {
                let mut payload = base_payload();
                payload["debug_trace"]["future_debug_option"] = serde_json::json!(true);
                ("debug trace", "future_debug_option", payload)
            },
            {
                let mut payload = base_payload();
                payload["resource_limits"]["future_limit"] = serde_json::json!(1);
                ("resource limits", "future_limit", payload)
            },
            {
                let mut payload = base_payload();
                payload["upstreams"][0]["limits"] = serde_json::json!({
                    "context_window": 1024,
                    "future_limit": 1
                });
                ("model limits", "future_limit", payload)
            },
            {
                let mut payload = base_payload();
                payload["upstreams"][0]["surface_defaults"] = serde_json::json!({
                    "tools": {
                        "supports_search": true,
                        "future_tool": true
                    }
                });
                ("model surface tools", "future_tool", payload)
            },
            {
                let mut payload = base_payload();
                payload["model_aliases"] = serde_json::json!({
                    "demo": {
                        "upstream_name": "default",
                        "upstream_model": "gpt-4",
                        "future_alias_option": true
                    }
                });
                ("model alias", "future_alias_option", payload)
            },
        ] {
            let error = serde_json::from_value::<RuntimeConfigPayload>(payload)
                .expect_err(&format!("{case} unknown field must fail JSON parsing"));
            assert!(
                error.to_string().contains(field),
                "{case} error should name unknown field `{field}`: {error}"
            );
        }
    }

    #[test]
    fn config_from_yaml_rejects_unknown_nested_fields() {
        for (case, field, yaml) in [
            (
                "model alias",
                "typo",
                r#"
upstreams:
  default:
    api_root: https://api.openai.com/v1
    format: openai-completion
model_aliases:
  demo:
    target: default:gpt-4
    typo: true
"#,
            ),
            (
                "hooks",
                "typo",
                r#"
hooks:
  timeout_secs: 8
  typo: true
upstreams:
  default:
    api_root: https://api.openai.com/v1
    format: openai-completion
"#,
            ),
            (
                "hook endpoint",
                "typo",
                r#"
hooks:
  exchange:
    url: https://example.com/hooks/exchange
    typo: true
upstreams:
  default:
    api_root: https://api.openai.com/v1
    format: openai-completion
"#,
            ),
        ] {
            let error = Config::from_yaml_str(yaml)
                .expect_err(&format!("{case} unknown field must fail YAML parsing"));
            assert!(
                error.contains(field),
                "{case} error should name unknown field `{field}`: {error}"
            );
        }
    }

    #[test]
    fn config_upstream_url_for_format() {
        let c = Config {
            upstreams: vec![UpstreamConfig {
                name: "default".to_string(),
                api_root: "https://api.openai.com/v1".to_string(),
                fixed_upstream_format: Some(UpstreamFormat::OpenAiResponses),
                provider_key_env: None,
                upstream_headers: Vec::new(),
                proxy: None,
                limits: None,
                surface_defaults: None,
            }],
            ..Config::default()
        };
        let upstream = &c.upstreams[0];
        assert!(c
            .upstream_url_for_format(upstream, UpstreamFormat::OpenAiCompletion, None, false)
            .ends_with("/v1/chat/completions"));
        assert!(c
            .upstream_url_for_format(upstream, UpstreamFormat::OpenAiResponses, None, false)
            .ends_with("/v1/responses"));
        assert!(c
            .upstream_url_for_format(
                upstream,
                UpstreamFormat::Google,
                Some("gemini-2.0-flash"),
                true
            )
            .ends_with(":streamGenerateContent?alt=sse"));
    }

    #[test]
    fn resolve_model_uses_explicit_upstream_prefix() {
        let c = Config {
            upstreams: vec![
                UpstreamConfig {
                    name: "glm".to_string(),
                    api_root: "https://example.com/v1".to_string(),
                    fixed_upstream_format: Some(UpstreamFormat::Anthropic),
                    provider_key_env: None,
                    upstream_headers: Vec::new(),
                    proxy: None,
                    limits: None,
                    surface_defaults: None,
                },
                UpstreamConfig {
                    name: "openai".to_string(),
                    api_root: "https://api.openai.com/v1".to_string(),
                    fixed_upstream_format: Some(UpstreamFormat::OpenAiResponses),
                    provider_key_env: None,
                    upstream_headers: Vec::new(),
                    proxy: None,
                    limits: None,
                    surface_defaults: None,
                },
            ],
            ..Config::default()
        };
        let resolved = c.resolve_model("glm:GLM-5").unwrap();
        assert_eq!(resolved.upstream_name, "glm");
        assert_eq!(resolved.upstream_model, "GLM-5");
    }

    #[test]
    fn resolve_model_uses_alias() {
        let mut c = Config {
            upstreams: vec![UpstreamConfig {
                name: "glm".to_string(),
                api_root: "https://example.com/v1".to_string(),
                fixed_upstream_format: Some(UpstreamFormat::Anthropic),
                provider_key_env: None,
                upstream_headers: Vec::new(),
                proxy: None,
                limits: None,
                surface_defaults: None,
            }],
            ..Config::default()
        };
        c.model_aliases.insert(
            "GLM-5".to_string(),
            ModelAlias {
                upstream_name: "glm".to_string(),
                upstream_model: "GLM-5".to_string(),
                limits: None,
                surface: None,
            },
        );
        let resolved = c.resolve_model("GLM-5").unwrap();
        assert_eq!(resolved.upstream_name, "glm");
        assert_eq!(resolved.upstream_model, "GLM-5");
    }

    #[test]
    fn resolve_model_single_upstream_uses_model_as_is() {
        let c = Config {
            upstreams: vec![UpstreamConfig {
                name: "default".to_string(),
                api_root: "https://api.openai.com/v1".to_string(),
                fixed_upstream_format: Some(UpstreamFormat::OpenAiResponses),
                provider_key_env: None,
                upstream_headers: Vec::new(),
                proxy: None,
                limits: None,
                surface_defaults: None,
            }],
            ..Config::default()
        };
        let resolved = c.resolve_model("gpt-4o").unwrap();
        assert_eq!(resolved.upstream_name, "default");
        assert_eq!(resolved.upstream_model, "gpt-4o");
    }

    #[test]
    fn resolve_model_multi_upstream_requires_alias_or_prefix() {
        let c = Config {
            upstreams: vec![
                UpstreamConfig {
                    name: "a".to_string(),
                    api_root: "https://a.example.com/v1".to_string(),
                    fixed_upstream_format: Some(UpstreamFormat::Anthropic),
                    provider_key_env: None,
                    upstream_headers: Vec::new(),
                    proxy: None,
                    limits: None,
                    surface_defaults: None,
                },
                UpstreamConfig {
                    name: "b".to_string(),
                    api_root: "https://b.example.com/v1".to_string(),
                    fixed_upstream_format: Some(UpstreamFormat::OpenAiCompletion),
                    provider_key_env: None,
                    upstream_headers: Vec::new(),
                    proxy: None,
                    limits: None,
                    surface_defaults: None,
                },
            ],
            ..Config::default()
        };
        assert!(c.resolve_model("shared-model").is_err());
    }

    #[test]
    fn validate_rejects_alias_to_unknown_upstream() {
        let mut c = Config::default();
        c.model_aliases.insert(
            "GLM-5".to_string(),
            ModelAlias {
                upstream_name: "missing".to_string(),
                upstream_model: "GLM-5".to_string(),
                limits: None,
                surface: None,
            },
        );
        assert!(c.validate().is_err());
    }

    #[test]
    fn config_from_yaml_str_parses_hooks_and_provider_key_env() {
        let c = Config::from_yaml_str(
            r#"
hooks:
  timeout_secs: 8
  exchange:
    url: https://example.com/exchange
    authorization: Bearer hook-token
  usage:
    url: https://example.com/usage
upstreams:
  GLM-OFFICIAL:
    api_root: https://open.bigmodel.cn/api/anthropic/v1
    format: anthropic
    provider_key_env: GLM_APIKEY
"#,
        )
        .unwrap();

        assert_eq!(
            c.hooks.exchange.as_ref().unwrap().url,
            "https://example.com/exchange"
        );
        assert_eq!(c.hooks.timeout.as_secs(), 8);
        assert_eq!(
            c.hooks.exchange.as_ref().unwrap().authorization.as_deref(),
            Some("Bearer hook-token")
        );
        assert_eq!(
            c.hooks.usage.as_ref().unwrap().url,
            "https://example.com/usage"
        );
        let upstream = c.upstream("GLM-OFFICIAL").unwrap();
        assert_eq!(upstream.provider_key_env.as_deref(), Some("GLM_APIKEY"));
    }

    #[test]
    fn validate_rejects_empty_provider_key_env() {
        let c = Config::from_yaml_str(
            r#"
upstreams:
  demo:
    api_root: https://api.openai.com/v1
    format: openai-completion
    provider_key_env: "   "
"#,
        )
        .unwrap();
        assert!(c.validate().is_err());
    }

    #[test]
    fn validate_rejects_forbidden_upstream_auth_header_overrides() {
        for forbidden in [
            "authorization",
            "proxy-authorization",
            "x-api-key",
            "api-key",
            "openai-api-key",
            "x-goog-api-key",
            "anthropic-api-key",
        ] {
            let c = Config::from_yaml_str(&format!(
                r#"
listen: 127.0.0.1:8080
upstreams:
  demo:
    api_root: https://api.openai.com/v1
    format: openai-completion
    upstream_headers:
      {forbidden}: secret
"#
            ))
            .unwrap();
            let error = c
                .validate()
                .expect_err("forbidden upstream auth header must be rejected");
            assert!(
                error.contains(forbidden),
                "error should name forbidden header `{forbidden}`: {error}"
            );
        }
    }

    #[test]
    fn runtime_config_rejects_forbidden_upstream_auth_header_overrides() {
        let mut payload = RuntimeConfigPayload {
            listen: "127.0.0.1:0".to_string(),
            upstream_timeout_secs: 30,
            compatibility_mode: CompatibilityMode::Balanced,
            proxy: Some(ProxyConfig::Direct),
            upstreams: vec![RuntimeUpstreamConfig {
                name: "default".to_string(),
                api_root: "https://api.openai.com/v1".to_string(),
                fixed_upstream_format: Some(UpstreamFormat::OpenAiCompletion),
                provider_key_env: None,
                upstream_headers: vec![("openai-api-key".to_string(), "secret".to_string())],
                proxy: None,
                limits: None,
                surface_defaults: None,
            }],
            model_aliases: BTreeMap::new(),
            hooks: RuntimeHookConfig::default(),
            debug_trace: DebugTraceConfig::default(),
            resource_limits: ResourceLimits::default(),
        };

        let error = Config::try_from(payload.clone())
            .expect_err("runtime config must reject forbidden auth header override");
        assert!(error.contains("openai-api-key"));

        payload.upstreams[0].upstream_headers = vec![("x-tenant".to_string(), "demo".to_string())];
        Config::try_from(payload).expect("non-secret upstream header should remain allowed");
    }

    #[test]
    fn config_from_yaml_str_parses_resource_limits() {
        let c = Config::from_yaml_str(
            r#"
listen: 127.0.0.1:8080
resource_limits:
  max_request_body_bytes: 4096
  max_non_stream_response_bytes: 8192
  max_upstream_error_body_bytes: 1024
  max_sse_frame_bytes: 2048
  stream_idle_timeout_secs: 7
  stream_max_duration_secs: 11
  stream_max_events: 13
  max_accumulated_stream_state_bytes: 16384
upstreams:
  demo:
    api_root: https://api.openai.com/v1
    format: openai-completion
"#,
        )
        .unwrap();

        assert_eq!(c.resource_limits.max_request_body_bytes, 4096);
        assert_eq!(c.resource_limits.max_non_stream_response_bytes, 8192);
        assert_eq!(c.resource_limits.max_upstream_error_body_bytes, 1024);
        assert_eq!(c.resource_limits.max_sse_frame_bytes, 2048);
        assert_eq!(c.resource_limits.stream_idle_timeout_secs, 7);
        assert_eq!(c.resource_limits.stream_max_duration_secs, 11);
        assert_eq!(c.resource_limits.stream_max_events, 13);
        assert_eq!(c.resource_limits.max_accumulated_stream_state_bytes, 16384);
        c.validate().expect("positive resource limits are valid");
    }

    #[test]
    fn validate_rejects_zero_resource_limits() {
        let mut c = Config {
            upstreams: vec![UpstreamConfig {
                name: "demo".to_string(),
                api_root: "https://api.openai.com/v1".to_string(),
                fixed_upstream_format: Some(UpstreamFormat::OpenAiCompletion),
                provider_key_env: None,
                upstream_headers: Vec::new(),
                proxy: None,
                limits: None,
                surface_defaults: None,
            }],
            ..Config::default()
        };

        c.resource_limits.max_request_body_bytes = 0;
        let error = c.validate().expect_err("zero resource limit must fail");
        assert!(
            error.contains("resource_limits.max_request_body_bytes"),
            "error = {error}"
        );
    }

    #[test]
    fn validate_rejects_invalid_hook_url() {
        let c = Config::from_yaml_str(
            r#"
hooks:
  usage:
    url: ftp://example.com/usage
upstreams:
  demo:
    api_root: https://api.openai.com/v1
    format: openai-completion
"#,
        )
        .unwrap();
        assert!(c.validate().is_err());
    }

    #[test]
    fn validate_rejects_invalid_proxy_urls() {
        let invalid_scheme = Config::from_yaml_str(
            r#"
proxy:
  url: ftp://proxy.example:21/egress
upstreams:
  demo:
    api_root: https://api.openai.com/v1
    format: openai-completion
"#,
        )
        .unwrap();
        assert_eq!(
            invalid_scheme.validate().unwrap_err(),
            "proxy url must use http, https, socks5, or socks5h, got `ftp`"
        );

        let missing_host = Config::from_yaml_str(
            r#"
upstreams:
  demo:
    api_root: https://api.openai.com/v1
    format: openai-completion
    proxy:
      url: http:///missing-host
"#,
        )
        .unwrap();
        assert_eq!(
            missing_host.validate().unwrap_err(),
            "upstream `demo` proxy url must include a host"
        );
    }

    #[test]
    fn validate_rejects_userinfo_in_upstream_api_root_and_hook_url() {
        let upstream = Config::from_yaml_str(
            r#"
upstreams:
  demo:
    api_root: https://user:pass@example.com/v1
    format: openai-completion
"#,
        )
        .unwrap();
        assert!(upstream.validate().is_err());

        let hook = Config::from_yaml_str(
            r#"
hooks:
  usage:
    url: https://user:pass@example.com/usage
upstreams:
  demo:
    api_root: https://api.openai.com/v1
    format: openai-completion
"#,
        )
        .unwrap();
        assert!(hook.validate().is_err());
    }

    #[test]
    fn admin_config_view_redacts_inline_credentials_hook_authorization_and_sensitive_headers() {
        let config = Config {
            listen: "127.0.0.1:0".to_string(),
            upstream_timeout: Duration::from_secs(30),
            compatibility_mode: CompatibilityMode::Balanced,
            proxy: Some(ProxyConfig::Proxy {
                url: "http://global-user:global-pass@proxy.example:8080/global-hop?api_key=proxy-secret#frag".to_string(),
            }),
            upstreams: vec![UpstreamConfig {
                name: "default".to_string(),
                api_root: "https://user:pass@api.openai.com/v1?api_key=inline-secret#frag"
                    .to_string(),
                fixed_upstream_format: Some(UpstreamFormat::OpenAiResponses),
                provider_key_env: Some("DEMO_KEY".to_string()),
                upstream_headers: vec![
                    ("x-tenant".to_string(), "demo".to_string()),
                    (
                        "authorization".to_string(),
                        "Bearer upstream-secret".to_string(),
                    ),
                    (
                        "proxy-authorization".to_string(),
                        "Bearer proxy-secret".to_string(),
                    ),
                    ("cookie".to_string(), "session=secret".to_string()),
                    ("set-cookie".to_string(), "session=secret".to_string()),
                    ("x-service-token".to_string(), "token-secret".to_string()),
                    ("x-client-secret".to_string(), "secret-secret".to_string()),
                    (
                        "x-client-credential".to_string(),
                        "credential-secret".to_string(),
                    ),
                    ("x-service-apikey".to_string(), "apikey-secret".to_string()),
                ],
                proxy: Some(ProxyConfig::Proxy {
                    url: "socks5h://up-user:up-pass@regional-proxy.example:1080/egress?sig=proxy-secret#frag".to_string(),
                }),
                limits: None,
                surface_defaults: None,
            }],
            model_aliases: Default::default(),
            hooks: HookConfig {
                exchange: Some(HookEndpointConfig {
                    url: "https://user:pass@example.com/hooks/exchange?token=exchange-secret#frag"
                        .to_string(),
                    authorization: Some("Bearer exchange-secret".to_string()),
                }),
                usage: Some(HookEndpointConfig {
                    url: "https://example.com/hooks/usage?sig=keep-out#frag".to_string(),
                    authorization: None,
                }),
                ..HookConfig::default()
            },
            debug_trace: DebugTraceConfig::default(),
            resource_limits: ResourceLimits {
                max_request_body_bytes: 12_345,
                ..ResourceLimits::default()
            },
        };

        let view = AdminConfigView::from(&config);
        let json = serde_json::to_value(&view).unwrap();

        assert_eq!(
            view.upstreams[0].provider_key_env.as_deref(),
            Some("DEMO_KEY")
        );
        assert_eq!(view.upstreams[0].api_root, "https://api.openai.com/v1");
        assert_eq!(
            view.proxy,
            Some(ProxyConfig::Proxy {
                url: "http://proxy.example:8080/global-hop".to_string(),
            })
        );
        assert_eq!(
            view.upstreams[0].proxy,
            Some(ProxyConfig::Proxy {
                url: "socks5h://regional-proxy.example:1080/egress".to_string(),
            })
        );
        assert!(
            view.hooks
                .exchange
                .as_ref()
                .unwrap()
                .authorization_configured
        );
        assert_eq!(
            view.hooks.exchange.as_ref().unwrap().url,
            "https://example.com/hooks/exchange"
        );
        assert!(!view.hooks.usage.as_ref().unwrap().authorization_configured);
        assert_eq!(view.upstreams[0].upstream_headers[0].name, "x-tenant");
        assert_eq!(
            view.upstreams[0].upstream_headers[0].value.as_deref(),
            Some("demo")
        );
        assert!(!view.upstreams[0].upstream_headers[0].value_redacted);
        assert_eq!(view.upstreams[0].upstream_headers[1].name, "authorization");
        assert!(view.upstreams[0].upstream_headers[1].value.is_none());
        assert!(view.upstreams[0].upstream_headers[1].value_redacted);
        assert_eq!(
            view.upstreams[0].upstream_headers[2].name,
            "proxy-authorization"
        );
        assert!(view.upstreams[0].upstream_headers[2].value.is_none());
        assert!(view.upstreams[0].upstream_headers[2].value_redacted);
        assert_eq!(view.upstreams[0].upstream_headers[3].name, "cookie");
        assert!(view.upstreams[0].upstream_headers[3].value.is_none());
        assert!(view.upstreams[0].upstream_headers[3].value_redacted);
        assert_eq!(view.upstreams[0].upstream_headers[4].name, "set-cookie");
        assert!(view.upstreams[0].upstream_headers[4].value.is_none());
        assert!(view.upstreams[0].upstream_headers[4].value_redacted);
        assert_eq!(
            view.upstreams[0].upstream_headers[5].name,
            "x-service-token"
        );
        assert!(view.upstreams[0].upstream_headers[5].value.is_none());
        assert!(view.upstreams[0].upstream_headers[5].value_redacted);
        assert_eq!(
            view.upstreams[0].upstream_headers[6].name,
            "x-client-secret"
        );
        assert!(view.upstreams[0].upstream_headers[6].value.is_none());
        assert!(view.upstreams[0].upstream_headers[6].value_redacted);
        assert_eq!(
            view.upstreams[0].upstream_headers[7].name,
            "x-client-credential"
        );
        assert!(view.upstreams[0].upstream_headers[7].value.is_none());
        assert!(view.upstreams[0].upstream_headers[7].value_redacted);
        assert_eq!(
            view.upstreams[0].upstream_headers[8].name,
            "x-service-apikey"
        );
        assert!(view.upstreams[0].upstream_headers[8].value.is_none());
        assert!(view.upstreams[0].upstream_headers[8].value_redacted);
        assert!(json["upstreams"][0]
            .get("fallback_credential_actual")
            .is_none());
        assert_eq!(json["proxy"]["url"], "http://proxy.example:8080/global-hop");
        assert_eq!(
            json["upstreams"][0]["proxy"]["url"],
            "socks5h://regional-proxy.example:1080/egress"
        );
        assert_eq!(view.resource_limits.max_request_body_bytes, 12_345);
        assert_eq!(
            json["resource_limits"]["max_request_body_bytes"],
            serde_json::json!(12_345)
        );
        assert!(json["hooks"]["exchange"].get("authorization").is_none());
        assert!(json["upstreams"][0]["upstream_headers"][1]["value"].is_null());
        assert!(!json.to_string().contains("inline-secret"));
        assert!(!json.to_string().contains("exchange-secret"));
        assert!(!json.to_string().contains("upstream-secret"));
        assert!(!json.to_string().contains("token-secret"));
        assert!(!json.to_string().contains("secret-secret"));
        assert!(!json.to_string().contains("proxy-secret"));
        assert!(!json.to_string().contains("session=secret"));
        assert!(!json.to_string().contains("credential-secret"));
        assert!(!json.to_string().contains("apikey-secret"));
        assert!(!json.to_string().contains("user:pass@"));
        assert!(!json.to_string().contains("global-user:global-pass@"));
        assert!(!json.to_string().contains("up-user:up-pass@"));
        assert!(!json.to_string().contains("api_key="));
        assert!(!json.to_string().contains("token="));
        assert!(!json.to_string().contains("sig="));
        assert!(!json.to_string().contains("#frag"));
    }

    #[test]
    fn sanitize_url_for_admin_strips_userinfo_query_and_fragment() {
        let sanitized =
            sanitize_url_for_admin("https://user:pass@example.com/v1/messages?token=secret#frag");

        assert_eq!(sanitized, "https://example.com/v1/messages");
        assert!(!sanitized.contains("user:pass@"));
        assert!(!sanitized.contains("?token="));
        assert!(!sanitized.contains("#frag"));
    }

    #[test]
    fn sanitize_url_for_admin_best_effort_scrubs_userinfo_query_and_fragment_when_parsing_fails() {
        let sanitized =
            sanitize_url_for_admin("https://user:pass@example.com:bad/v1?api_key=secret#frag");

        assert_eq!(sanitized, "https://example.com:bad/v1");
        assert!(!sanitized.contains("user:pass@"));
        assert!(!sanitized.contains("?api_key="));
        assert!(!sanitized.contains("#frag"));
    }
}
