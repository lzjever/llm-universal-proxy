#[path = "common/forward_proxy.rs"]
mod forward_proxy;
#[path = "common/mock_upstream.rs"]
mod mock_upstream;
#[path = "common/runtime_proxy.rs"]
mod runtime_proxy;

use forward_proxy::spawn_http_forward_proxy;
use llm_universal_proxy::config::Config;
use llm_universal_proxy::formats::UpstreamFormat;
use mock_upstream::spawn_openai_completion_mock;
use reqwest::{
    header::{HeaderMap as ReqwestHeaderMap, HeaderValue},
    Client,
};
use runtime_proxy::{start_proxy, upstream_api_root};
use serde_json::json;
use std::sync::LazyLock;
use std::time::Duration;

static UPSTREAM_PROXY_ENV_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));
const TEST_PROVIDER_KEY: &str = "provider-secret";

fn direct_data_client() -> Client {
    let mut headers = ReqwestHeaderMap::new();
    headers.insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {TEST_PROVIDER_KEY}")).unwrap(),
    );
    Client::builder()
        .no_proxy()
        .default_headers(headers)
        .build()
        .unwrap()
}

#[derive(Clone, Copy)]
enum ProxyLayer<'a> {
    Missing,
    Direct,
    Url(&'a str),
}

struct ScopedEnvVar {
    key: &'static str,
    previous: Option<String>,
}

impl ScopedEnvVar {
    fn set(key: &'static str, value: impl AsRef<str>) -> Self {
        let previous = std::env::var(key).ok();
        std::env::set_var(key, value.as_ref());
        Self { key, previous }
    }

    fn remove(key: &'static str) -> Self {
        let previous = std::env::var(key).ok();
        std::env::remove_var(key);
        Self { key, previous }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        if let Some(value) = &self.previous {
            std::env::set_var(self.key, value);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

fn openai_completion_proxy_yaml(
    api_root: &str,
    namespace_proxy: ProxyLayer<'_>,
    upstream_override: ProxyLayer<'_>,
) -> String {
    let mut yaml = String::from("listen: 127.0.0.1:0\n");
    match namespace_proxy {
        ProxyLayer::Missing => {}
        ProxyLayer::Direct => yaml.push_str("proxy: direct\n"),
        ProxyLayer::Url(url) => {
            yaml.push_str("proxy:\n");
            yaml.push_str(&format!("  url: \"{url}\"\n"));
        }
    }
    yaml.push_str("upstreams:\n");
    yaml.push_str("  OPENAI:\n");
    yaml.push_str(&format!("    api_root: \"{api_root}\"\n"));
    yaml.push_str("    format: openai-completion\n");
    match upstream_override {
        ProxyLayer::Missing => {}
        ProxyLayer::Direct => yaml.push_str("    proxy: direct\n"),
        ProxyLayer::Url(url) => {
            yaml.push_str("    proxy:\n");
            yaml.push_str(&format!("      url: \"{url}\"\n"));
        }
    }
    yaml
}

fn install_http_proxy_env(proxy_url: &str) -> Vec<ScopedEnvVar> {
    vec![
        ScopedEnvVar::set("HTTP_PROXY", proxy_url),
        ScopedEnvVar::set("http_proxy", proxy_url),
        ScopedEnvVar::remove("HTTPS_PROXY"),
        ScopedEnvVar::remove("https_proxy"),
        ScopedEnvVar::remove("ALL_PROXY"),
        ScopedEnvVar::remove("all_proxy"),
        ScopedEnvVar::remove("NO_PROXY"),
        ScopedEnvVar::remove("no_proxy"),
        ScopedEnvVar::remove("REQUEST_METHOD"),
    ]
}

async fn send_chat_completion(proxy_base: &str) -> serde_json::Value {
    let client = direct_data_client();
    let response = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4o-mini",
            "messages": [
                { "role": "user", "content": "Say hi" }
            ]
        }))
        .send()
        .await
        .unwrap();

    assert!(response.status().is_success());
    response.json().await.unwrap()
}

#[tokio::test]
async fn missing_proxy_layers_should_inherit_http_proxy_environment() {
    let _env_guard = UPSTREAM_PROXY_ENV_LOCK.lock().await;
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let (env_proxy_base, _env_proxy, captured_env_proxy) = spawn_http_forward_proxy().await;
    let _env = install_http_proxy_env(&env_proxy_base);

    let config = Config::from_yaml_str(&openai_completion_proxy_yaml(
        &upstream_api_root(&mock_base, UpstreamFormat::OpenAiCompletion),
        ProxyLayer::Missing,
        ProxyLayer::Missing,
    ))
    .unwrap();
    let (proxy_base, _proxy) = start_proxy(config).await;

    let body = send_chat_completion(&proxy_base).await;
    assert_eq!(body["choices"][0]["message"]["content"], "Hi");

    let captured = captured_env_proxy
        .wait_for_count(1, Duration::from_secs(1))
        .await;
    assert_eq!(captured.len(), 1, "captured = {captured:?}");
    assert_eq!(captured[0].method, "POST");
    assert!(
        captured[0].uri.contains("/v1/chat/completions"),
        "captured = {captured:?}"
    );
}

#[tokio::test]
async fn namespace_proxy_should_route_requests_when_no_per_upstream_override_is_present() {
    let _env_guard = UPSTREAM_PROXY_ENV_LOCK.lock().await;
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let (env_proxy_base, _env_proxy, captured_env_proxy) = spawn_http_forward_proxy().await;
    let (top_level_proxy_base, _top_level_proxy, captured_top_level_proxy) =
        spawn_http_forward_proxy().await;
    let _env = install_http_proxy_env(&env_proxy_base);

    let config = Config::from_yaml_str(&openai_completion_proxy_yaml(
        &upstream_api_root(&mock_base, UpstreamFormat::OpenAiCompletion),
        ProxyLayer::Url(&top_level_proxy_base),
        ProxyLayer::Missing,
    ))
    .unwrap();
    let (proxy_base, _proxy) = start_proxy(config).await;

    let body = send_chat_completion(&proxy_base).await;
    assert_eq!(body["choices"][0]["message"]["content"], "Hi");

    let top_level_seen = captured_top_level_proxy
        .wait_for_count(1, Duration::from_secs(1))
        .await;
    let env_seen = captured_env_proxy.snapshot();

    assert_eq!(top_level_seen.len(), 1, "captured = {top_level_seen:?}");
    assert!(
        env_seen.is_empty(),
        "top-level proxy should shadow env proxy: {env_seen:?}"
    );
}

#[tokio::test]
async fn per_upstream_override_should_override_environment_when_namespace_proxy_is_missing() {
    let _env_guard = UPSTREAM_PROXY_ENV_LOCK.lock().await;
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let (env_proxy_base, _env_proxy, captured_env_proxy) = spawn_http_forward_proxy().await;
    let (override_proxy_base, _override_proxy, captured_override_proxy) =
        spawn_http_forward_proxy().await;
    let _env = install_http_proxy_env(&env_proxy_base);

    let config = Config::from_yaml_str(&openai_completion_proxy_yaml(
        &upstream_api_root(&mock_base, UpstreamFormat::OpenAiCompletion),
        ProxyLayer::Missing,
        ProxyLayer::Url(&override_proxy_base),
    ))
    .unwrap();
    let (proxy_base, _proxy) = start_proxy(config).await;

    let body = send_chat_completion(&proxy_base).await;
    assert_eq!(body["choices"][0]["message"]["content"], "Hi");

    let override_seen = captured_override_proxy
        .wait_for_count(1, Duration::from_secs(1))
        .await;
    let env_seen = captured_env_proxy.snapshot();

    assert_eq!(override_seen.len(), 1, "captured = {override_seen:?}");
    assert!(
        env_seen.is_empty(),
        "per-upstream override should shadow env proxy: {env_seen:?}"
    );
}

#[tokio::test]
async fn explicit_namespace_direct_should_cut_environment_proxy() {
    let _env_guard = UPSTREAM_PROXY_ENV_LOCK.lock().await;
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let (env_proxy_base, _env_proxy, captured_env_proxy) = spawn_http_forward_proxy().await;
    let _env = install_http_proxy_env(&env_proxy_base);

    let config = Config::from_yaml_str(&openai_completion_proxy_yaml(
        &upstream_api_root(&mock_base, UpstreamFormat::OpenAiCompletion),
        ProxyLayer::Direct,
        ProxyLayer::Missing,
    ))
    .unwrap();
    let (proxy_base, _proxy) = start_proxy(config).await;

    let body = send_chat_completion(&proxy_base).await;
    assert_eq!(body["choices"][0]["message"]["content"], "Hi");

    let env_seen = captured_env_proxy.snapshot();
    assert!(
        env_seen.is_empty(),
        "explicit direct should bypass env proxy: {env_seen:?}"
    );
}
