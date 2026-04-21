//! Shared proxy configuration and startup helpers for integration tests.

use llm_universal_proxy::config::{
    AuthPolicy, CompatibilityMode, Config, DebugTraceConfig, UpstreamConfig,
};
use llm_universal_proxy::formats::UpstreamFormat;
use llm_universal_proxy::server::run_with_listener;
use std::time::Duration;
use tokio::net::TcpListener;

pub fn upstream_api_root(upstream_base: &str, format: UpstreamFormat) -> String {
    let upstream_base = upstream_base.trim_end_matches('/');
    match format {
        UpstreamFormat::Google => format!("{upstream_base}/v1beta"),
        _ => format!("{upstream_base}/v1"),
    }
}

pub fn proxy_config(upstream_base: &str, format: UpstreamFormat) -> Config {
    Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: Duration::from_secs(30),
        compatibility_mode: CompatibilityMode::Balanced,
        upstreams: vec![UpstreamConfig {
            name: "default".to_string(),
            api_root: upstream_api_root(upstream_base, format),
            fixed_upstream_format: Some(format),
            fallback_credential_env: None,
            fallback_credential_actual: None,
            fallback_api_key: None,
            auth_policy: AuthPolicy::ClientOrFallback,
            upstream_headers: Vec::new(),
            limits: None,
            surface_defaults: None,
        }],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: DebugTraceConfig::default(),
    }
}

/// Start proxy with config; returns (proxy_base_url, _handle).
pub async fn start_proxy(
    config: Config,
) -> (
    String,
    tokio::task::JoinHandle<Result<(), Box<dyn std::error::Error + Send + Sync>>>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let handle = tokio::spawn(async move { run_with_listener(config, listener).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    (base, handle)
}
