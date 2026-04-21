//! Runtime proxy startup helpers shared by integration tests.

use llm_universal_proxy::config::Config;
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
