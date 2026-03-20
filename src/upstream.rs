//! Upstream HTTP client: build request URL, POST body, return response (non-streaming or stream).

use reqwest::Client;
use serde_json::Value;

use crate::config::{Config, UpstreamConfig};
use crate::formats::UpstreamFormat;

/// Build a reqwest client with timeout from config.
pub fn build_client(config: &Config) -> Client {
    Client::builder()
        .timeout(config.upstream_timeout)
        .build()
        .unwrap_or_else(|_| Client::new())
}

/// Call upstream with JSON body; for non-streaming, read full body and return (status, body bytes).
/// For streaming, returns the response so caller can forward the stream.
pub async fn call_upstream(
    client: &Client,
    url: &str,
    body: &Value,
    stream: bool,
    headers: &[(String, String)],
) -> Result<reqwest::Response, reqwest::Error> {
    let mut req = client.post(url).json(body);
    if stream {
        req = req.header("Accept", "text/event-stream");
    }
    // Forward auth headers
    for (name, value) in headers {
        req = req.header(name, value);
    }
    req.send().await
}

/// Resolve upstream URL for the given format using config base URL.
/// For Google (Gemini), pass the model so the path is .../models/{model}:generateContent.
pub fn upstream_url(
    config: &Config,
    upstream: &UpstreamConfig,
    format: UpstreamFormat,
    model: Option<&str>,
    stream: bool,
) -> String {
    config.upstream_url_for_format(upstream, format, model, stream)
}
