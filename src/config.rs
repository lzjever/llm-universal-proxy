//! Configuration: YAML-backed multi-upstream routing, model aliases, and upstream URL building.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::formats::UpstreamFormat;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum AuthPolicy {
    #[serde(rename = "client_or_fallback", alias = "client-or-fallback")]
    #[default]
    ClientOrFallback,
    #[serde(rename = "force_server", alias = "force-server")]
    ForceServer,
}

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
pub struct HookEndpointConfig {
    pub url: String,
    pub authorization: Option<String>,
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
    /// Optional fallback credential env var name, for example `GLM_APIKEY`.
    pub fallback_credential_env: Option<String>,
    /// Optional fallback credential value loaded directly from config.
    pub fallback_credential_actual: Option<String>,
    /// Resolved fallback credential value loaded from the env var above.
    pub fallback_api_key: Option<String>,
    /// Credential policy for this upstream.
    pub auth_policy: AuthPolicy,
    /// Optional static headers to inject into every upstream request.
    pub upstream_headers: Vec<(String, String)>,
}

/// One local model alias that resolves to a named upstream and upstream model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelAlias {
    pub upstream_name: String,
    pub upstream_model: String,
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
    /// Named upstreams available to the proxy.
    pub upstreams: Vec<UpstreamConfig>,
    /// Local unique model names mapped to named upstream models.
    pub model_aliases: BTreeMap<String, ModelAlias>,
    /// Optional audit and metering hooks.
    pub hooks: HookConfig,
    /// Optional local debug trace sink.
    pub debug_trace: DebugTraceConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeHookEndpointConfig {
    pub url: String,
    #[serde(default)]
    pub authorization: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
pub struct RuntimeUpstreamConfig {
    pub name: String,
    pub api_root: String,
    #[serde(default)]
    pub fixed_upstream_format: Option<UpstreamFormat>,
    #[serde(default)]
    pub fallback_credential_env: Option<String>,
    #[serde(default)]
    pub fallback_credential_actual: Option<String>,
    #[serde(default)]
    pub auth_policy: AuthPolicy,
    #[serde(default)]
    pub upstream_headers: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeConfigPayload {
    #[serde(default = "default_listen")]
    pub listen: String,
    #[serde(default = "default_upstream_timeout_secs")]
    pub upstream_timeout_secs: u64,
    #[serde(default)]
    pub upstreams: Vec<RuntimeUpstreamConfig>,
    #[serde(default)]
    pub model_aliases: BTreeMap<String, ModelAlias>,
    #[serde(default)]
    pub hooks: RuntimeHookConfig,
    #[serde(default)]
    pub debug_trace: DebugTraceConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeConfigSnapshot {
    pub revision: String,
    pub config: RuntimeConfigPayload,
}

#[derive(Debug, Clone, Deserialize)]
struct FileConfig {
    #[serde(default = "default_listen")]
    listen: String,
    #[serde(default = "default_upstream_timeout_secs")]
    upstream_timeout_secs: u64,
    #[serde(default)]
    upstreams: BTreeMap<String, UpstreamConfigFile>,
    #[serde(default)]
    model_aliases: BTreeMap<String, String>,
    #[serde(default)]
    hooks: HooksFileConfig,
    #[serde(default)]
    debug_trace: DebugTraceConfig,
}

#[derive(Debug, Clone, Deserialize)]
struct UpstreamConfigFile {
    #[serde(alias = "url", alias = "upstream_url", alias = "base_url")]
    api_root: String,
    #[serde(default, alias = "upstream_format", alias = "format")]
    fixed_upstream_format: Option<UpstreamFormat>,
    #[serde(
        default,
        alias = "fallback_credential_key",
        alias = "fallback_credential_env",
        alias = "credential_env",
        alias = "api_key_env"
    )]
    fallback_credential_env: Option<String>,
    #[serde(
        default,
        alias = "credential_actual",
        alias = "fallback_credential_actual",
        alias = "api_key"
    )]
    fallback_credential_actual: Option<String>,
    #[serde(default)]
    auth_policy: AuthPolicy,
    #[serde(default, alias = "headers", alias = "upstream_headers")]
    upstream_headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
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
            upstreams: Vec::new(),
            model_aliases: BTreeMap::new(),
            hooks: HookConfig::default(),
            debug_trace: DebugTraceConfig::default(),
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
            serde_yaml::from_str(raw).map_err(|e| format!("failed to parse YAML config: {}", e))?;

        let upstreams = parsed
            .upstreams
            .into_iter()
            .map(|(name, item)| {
                let fallback_api_key = item.fallback_credential_actual.clone().or_else(|| {
                    item.fallback_credential_env
                        .as_deref()
                        .and_then(|env_name| std::env::var(env_name).ok())
                });
                UpstreamConfig {
                    name,
                    api_root: item.api_root,
                    fixed_upstream_format: item.fixed_upstream_format,
                    fallback_credential_env: item.fallback_credential_env,
                    fallback_credential_actual: item.fallback_credential_actual,
                    fallback_api_key,
                    auth_policy: item.auth_policy,
                    upstream_headers: item.upstream_headers.into_iter().collect(),
                }
            })
            .collect();

        let model_aliases = parsed
            .model_aliases
            .into_iter()
            .filter_map(|(alias, target)| {
                let (upstream_name, upstream_model) = target.split_once(':')?;
                Some((
                    alias,
                    ModelAlias {
                        upstream_name: upstream_name.to_string(),
                        upstream_model: upstream_model.to_string(),
                    },
                ))
            })
            .collect();

        Ok(Self {
            listen: parsed.listen,
            upstream_timeout: Duration::from_secs(parsed.upstream_timeout_secs),
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
        })
    }

    /// Validate config and return a descriptive error for invalid settings.
    pub fn validate(&self) -> Result<(), String> {
        if self.upstreams.is_empty() {
            return Err("at least one upstream must be configured".to_string());
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
            if upstream.fallback_credential_env.is_some()
                && upstream.fallback_credential_actual.is_some()
            {
                return Err(format!(
                    "upstream `{}` cannot set both credential_env and credential_actual",
                    upstream.name
                ));
            }
            if upstream.auth_policy == AuthPolicy::ForceServer
                && upstream.fallback_api_key.is_none()
            {
                return Err(format!(
                    "upstream `{}` auth_policy=force-server requires a server credential",
                    upstream.name
                ));
            }
            if !seen.insert(upstream.name.clone()) {
                return Err(format!("duplicate upstream name `{}`", upstream.name));
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
                    "model alias `{}` must point to a non-empty upstream model",
                    alias
                ));
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
            return Err(format!("{} hook url must not be empty", hook_name));
        }
        let parsed = url::Url::parse(&hook.url)
            .map_err(|e| format!("{} hook url is invalid: {}", hook_name, e))?;
        match parsed.scheme() {
            "http" | "https" => {}
            scheme => {
                return Err(format!(
                    "{} hook url must use http or https, got `{}`",
                    hook_name, scheme
                ));
            }
        }
        Ok(())
    }

    pub fn upstream(&self, name: &str) -> Option<&UpstreamConfig> {
        self.upstreams.iter().find(|u| u.name == name)
    }

    /// Resolve a client-visible model string to a named upstream and upstream model.
    pub fn resolve_model(&self, requested_model: &str) -> Result<ResolvedModel, String> {
        if let Some((upstream_name, upstream_model)) = requested_model.split_once(':') {
            if self.upstream(upstream_name).is_some() {
                if upstream_model.trim().is_empty() {
                    return Err(format!(
                        "model `{}` must include a non-empty upstream model after `:`",
                        requested_model
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
            "model `{}` is ambiguous; use `upstream:model` or configure model_aliases",
            requested_model
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
            .map(|item| {
                let fallback_api_key = item.fallback_credential_actual.clone().or_else(|| {
                    item.fallback_credential_env
                        .as_deref()
                        .and_then(|env_name| std::env::var(env_name).ok())
                });
                UpstreamConfig {
                    name: item.name,
                    api_root: item.api_root,
                    fixed_upstream_format: item.fixed_upstream_format,
                    fallback_credential_env: item.fallback_credential_env,
                    fallback_credential_actual: item.fallback_credential_actual,
                    fallback_api_key,
                    auth_policy: item.auth_policy,
                    upstream_headers: item.upstream_headers,
                }
            })
            .collect::<Vec<_>>();

        let config = Self {
            listen: value.listen,
            upstream_timeout: Duration::from_secs(value.upstream_timeout_secs),
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
            upstreams: value
                .upstreams
                .iter()
                .map(|item| RuntimeUpstreamConfig {
                    name: item.name.clone(),
                    api_root: item.api_root.clone(),
                    fixed_upstream_format: item.fixed_upstream_format,
                    fallback_credential_env: item.fallback_credential_env.clone(),
                    fallback_credential_actual: item.fallback_credential_actual.clone(),
                    auth_policy: item.auth_policy,
                    upstream_headers: item.upstream_headers.clone(),
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
        }
    }
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

/// Build a full resource URL for auxiliary upstream endpoints.
pub fn build_upstream_resource_url(api_root: &str, resource_path: &str) -> String {
    let base = api_root.trim_end_matches('/');
    let path = resource_path.trim_start_matches('/');
    format!("{}/{}", base, path)
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
        std::env::set_var("GLM_APIKEY", "glm-secret");
        let c = Config::from_yaml_str(
            r#"
listen: 127.0.0.1:8080
upstream_timeout_secs: 45
upstreams:
  GLM-OFFICIAL:
    api_root: https://open.bigmodel.cn/api/anthropic/v1
    format: anthropic
    credential_env: GLM_APIKEY
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
        assert_eq!(glm.fallback_credential_env.as_deref(), Some("GLM_APIKEY"));
        assert_eq!(glm.fallback_api_key.as_deref(), Some("glm-secret"));
        assert_eq!(glm.auth_policy, AuthPolicy::ClientOrFallback);
        let alias = c.model_aliases.get("GLM-5").unwrap();
        assert_eq!(alias.upstream_name, "GLM-OFFICIAL");
        assert_eq!(alias.upstream_model, "GLM-5");
        std::env::remove_var("GLM_APIKEY");
    }

    #[test]
    fn config_from_yaml_requires_upstreams() {
        let c = Config::from_yaml_str("listen: 127.0.0.1:8080").unwrap();
        assert!(c.validate().is_err());
    }

    #[test]
    fn config_upstream_url_for_format() {
        let c = Config {
            upstreams: vec![UpstreamConfig {
                name: "default".to_string(),
                api_root: "https://api.openai.com/v1".to_string(),
                fixed_upstream_format: Some(UpstreamFormat::OpenAiResponses),
                fallback_credential_env: None,
                fallback_credential_actual: None,
                fallback_api_key: None,
                auth_policy: AuthPolicy::ClientOrFallback,
                upstream_headers: Vec::new(),
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
                    fallback_credential_env: None,
                    fallback_credential_actual: None,
                    fallback_api_key: None,
                    auth_policy: AuthPolicy::ClientOrFallback,
                    upstream_headers: Vec::new(),
                },
                UpstreamConfig {
                    name: "openai".to_string(),
                    api_root: "https://api.openai.com/v1".to_string(),
                    fixed_upstream_format: Some(UpstreamFormat::OpenAiResponses),
                    fallback_credential_env: None,
                    fallback_credential_actual: None,
                    fallback_api_key: None,
                    auth_policy: AuthPolicy::ClientOrFallback,
                    upstream_headers: Vec::new(),
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
                fallback_credential_env: None,
                fallback_credential_actual: None,
                fallback_api_key: None,
                auth_policy: AuthPolicy::ClientOrFallback,
                upstream_headers: Vec::new(),
            }],
            ..Config::default()
        };
        c.model_aliases.insert(
            "GLM-5".to_string(),
            ModelAlias {
                upstream_name: "glm".to_string(),
                upstream_model: "GLM-5".to_string(),
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
                fallback_credential_env: None,
                fallback_credential_actual: None,
                fallback_api_key: None,
                auth_policy: AuthPolicy::ClientOrFallback,
                upstream_headers: Vec::new(),
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
                    fallback_credential_env: None,
                    fallback_credential_actual: None,
                    fallback_api_key: None,
                    auth_policy: AuthPolicy::ClientOrFallback,
                    upstream_headers: Vec::new(),
                },
                UpstreamConfig {
                    name: "b".to_string(),
                    api_root: "https://b.example.com/v1".to_string(),
                    fixed_upstream_format: Some(UpstreamFormat::OpenAiCompletion),
                    fallback_credential_env: None,
                    fallback_credential_actual: None,
                    fallback_api_key: None,
                    auth_policy: AuthPolicy::ClientOrFallback,
                    upstream_headers: Vec::new(),
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
            },
        );
        assert!(c.validate().is_err());
    }

    #[test]
    fn config_from_yaml_str_parses_hooks_and_force_server_policy() {
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
    credential_actual: secret
    auth_policy: force_server
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
        assert_eq!(
            upstream.fallback_credential_actual.as_deref(),
            Some("secret")
        );
        assert_eq!(upstream.fallback_api_key.as_deref(), Some("secret"));
        assert_eq!(upstream.auth_policy, AuthPolicy::ForceServer);
    }

    #[test]
    fn validate_rejects_conflicting_credential_sources() {
        let c = Config::from_yaml_str(
            r#"
upstreams:
  demo:
    api_root: https://api.openai.com/v1
    format: openai-completion
    credential_env: OPENAI_API_KEY
    credential_actual: secret
"#,
        )
        .unwrap();
        assert!(c.validate().is_err());
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
}
