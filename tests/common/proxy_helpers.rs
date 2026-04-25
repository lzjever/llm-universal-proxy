//! Shared proxy configuration helpers for integration tests.

use super::runtime_proxy::upstream_api_root;
use llm_universal_proxy::config::{
    AuthPolicy, CompatibilityMode, Config, DebugTraceConfig, ProxyConfig, UpstreamConfig,
};
use llm_universal_proxy::formats::UpstreamFormat;
use std::time::Duration;

pub fn proxy_config(upstream_base: &str, format: UpstreamFormat) -> Config {
    Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: Duration::from_secs(30),
        compatibility_mode: CompatibilityMode::Balanced,
        proxy: Some(ProxyConfig::Direct),
        upstreams: vec![UpstreamConfig {
            name: "default".to_string(),
            api_root: upstream_api_root(upstream_base, format),
            fixed_upstream_format: Some(format),
            fallback_credential_env: None,
            fallback_credential_actual: None,
            fallback_api_key: None,
            auth_policy: AuthPolicy::ClientOrFallback,
            upstream_headers: Vec::new(),
            proxy: None,
            limits: None,
            surface_defaults: None,
        }],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: DebugTraceConfig::default(),
        resource_limits: Default::default(),
    }
}
