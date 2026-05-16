//! Multimodal e2e contract tests backed by first-party spec-aware mock upstreams.

mod common;

use common::mock_upstream::{
    spawn_asserting_anthropic_mock, spawn_asserting_openai_completion_mock,
    spawn_asserting_openai_responses_mock, CapturedMockRequest,
};
use common::proxy_helpers::proxy_config;
use common::runtime_proxy::start_proxy;
use llm_universal_proxy::formats::UpstreamFormat;
use reqwest::{
    header::{HeaderMap, HeaderValue},
    Client as ReqwestClient,
};
use serde_json::{json, Value};
use std::time::Duration;

const TEST_PROVIDER_KEY: &str = "provider-secret";
const PNG_B64: &str = "iVBORw0KGgo=";
const PNG_DATA_URI: &str = "data:image/png;base64,iVBORw0KGgo=";
const AUDIO_WAV_B64: &str = "UklGRiQAAABXQVZFZm10IBAAAAABAAEAESsAACJWAAACABAAZGF0YQAAAAA=";
const REMOTE_IMAGE_URL: &str = "https://example.com/cat.png";
const POLLUTED_REMOTE_IMAGE_URL: &str = "https://example.com/cat.png\nfile:///tmp/cat.png";
const LEADING_WHITESPACE_REMOTE_IMAGE_URL: &str = " https://example.com/cat.png";
const TRAILING_CONTROL_REMOTE_IMAGE_URL: &str = "https://example.com/cat.png\n";
const ENCODED_CONTROL_REMOTE_IMAGE_URL: &str = "https://example.com/cat%0A.png";
const ENCODED_CONTROL_REMOTE_PDF_URL: &str = "https://example.com/doc%00.pdf";
const TEXT_DATA_URI: &str = "data:text/plain;base64,SGVsbG8=";

fn authenticated_reqwest_client() -> ReqwestClient {
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {TEST_PROVIDER_KEY}")).unwrap(),
    );
    ReqwestClient::builder()
        .default_headers(headers)
        .build()
        .unwrap()
}

#[tokio::test]
async fn multimodal_openai_chat_to_anthropic_maps_data_uri_image_to_base64_source() {
    let (mock_base, _mock, captured) =
        spawn_asserting_anthropic_mock(assert_anthropic_image_base64_source).await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let response = authenticated_reqwest_client()
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "claude-3-5-sonnet",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "Describe this image" },
                    { "type": "image_url", "image_url": { "url": PNG_DATA_URI } }
                ]
            }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();

    assert_success_response(response).await;
    assert_eq!(
        captured
            .wait_for_count(1, Duration::from_secs(1))
            .await
            .len(),
        1
    );
}

#[tokio::test]
async fn multimodal_openai_remote_image_to_anthropic_maps_to_url_source() {
    let (mock_base, _mock, captured) =
        spawn_asserting_anthropic_mock(assert_anthropic_image_url_source).await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let response = send_remote_image_chat_request(&proxy_base).await;
    assert_success_response(response).await;
    assert_eq!(
        captured
            .wait_for_count(1, Duration::from_secs(1))
            .await
            .len(),
        1
    );
}

#[tokio::test]
async fn multimodal_anthropic_polluted_url_image_to_openai_targets_fails_closed_before_upstream() {
    for upstream_format in [
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
    ] {
        for image_url in [
            POLLUTED_REMOTE_IMAGE_URL,
            LEADING_WHITESPACE_REMOTE_IMAGE_URL,
            TRAILING_CONTROL_REMOTE_IMAGE_URL,
        ] {
            match upstream_format {
                UpstreamFormat::OpenAiCompletion => {
                    let (mock_base, _mock, captured) =
                        spawn_asserting_openai_completion_mock(|_| {
                            Err("polluted Anthropic image URL reached OpenAI Chat upstream"
                                .to_string())
                        })
                        .await;
                    let config = proxy_config(&mock_base, upstream_format);
                    let (proxy_base, _proxy) = start_proxy(config).await;

                    let response = send_anthropic_url_image_request(&proxy_base, image_url).await;
                    assert_failure_response(response).await;
                    assert_no_upstream_request(&captured).await;
                }
                UpstreamFormat::OpenAiResponses => {
                    let (mock_base, _mock, captured) =
                        spawn_asserting_openai_responses_mock(|_| {
                            Err(
                                "polluted Anthropic image URL reached OpenAI Responses upstream"
                                    .to_string(),
                            )
                        })
                        .await;
                    let config = proxy_config(&mock_base, upstream_format);
                    let (proxy_base, _proxy) = start_proxy(config).await;

                    let response = send_anthropic_url_image_request(&proxy_base, image_url).await;
                    assert_failure_response(response).await;
                    assert_no_upstream_request(&captured).await;
                }
                _ => unreachable!("only OpenAI targets are covered here"),
            }
        }
    }
}

#[tokio::test]
async fn multimodal_openai_chat_responses_polluted_media_sources_fail_closed_before_upstream() {
    let (mock_base, _mock, captured) = spawn_asserting_openai_responses_mock(|_| {
        Err("polluted Chat media source reached OpenAI Responses upstream".to_string())
    })
    .await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let response = authenticated_reqwest_client()
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4o",
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "image_url",
                    "image_url": { "url": ENCODED_CONTROL_REMOTE_IMAGE_URL }
                }]
            }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_failure_response(response).await;
    assert_no_upstream_request(&captured).await;

    let (mock_base, _mock, captured) = spawn_asserting_openai_completion_mock(|_| {
        Err("polluted Responses media source reached OpenAI Chat upstream".to_string())
    })
    .await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let response = authenticated_reqwest_client()
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "gpt-4o",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{
                    "type": "input_file",
                    "file_url": ENCODED_CONTROL_REMOTE_PDF_URL,
                    "filename": "doc.pdf"
                }]
            }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_failure_response(response).await;
    assert_no_upstream_request(&captured).await;
}

#[tokio::test]
async fn multimodal_openai_audio_and_non_pdf_file_to_anthropic_fail_closed_before_upstream() {
    for (label, content) in [
        (
            "audio",
            json!([
                { "type": "text", "text": "Transcribe this audio" },
                { "type": "input_audio", "input_audio": { "data": AUDIO_WAV_B64, "format": "wav" } }
            ]),
        ),
        (
            "non-PDF file",
            json!([
                { "type": "text", "text": "Summarize this text file" },
                { "type": "file", "file": {
                    "file_data": TEXT_DATA_URI,
                    "filename": "notes.txt",
                    "mime_type": "text/plain"
                } }
            ]),
        ),
    ] {
        let (mock_base, _mock, captured) = spawn_asserting_anthropic_mock(move |_| {
            Err(format!("{label} reached Anthropic upstream"))
        })
        .await;
        let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
        let (proxy_base, _proxy) = start_proxy(config).await;

        let response = authenticated_reqwest_client()
            .post(format!("{proxy_base}/openai/v1/chat/completions"))
            .json(&json!({
                "model": "claude-3-5-sonnet",
                "messages": [{ "role": "user", "content": content }],
                "stream": false
            }))
            .send()
            .await
            .unwrap();

        assert_failure_response(response).await;
        assert_no_upstream_request(&captured).await;
    }
}

async fn send_anthropic_url_image_request(proxy_base: &str, image_url: &str) -> reqwest::Response {
    authenticated_reqwest_client()
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .header("anthropic-version", "2023-06-01")
        .json(&json!({
            "model": "multimodal-test",
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "Describe this remote image" },
                    { "type": "image", "source": {
                        "type": "url",
                        "url": image_url,
                        "media_type": "image/png"
                    }}
                ]
            }],
            "stream": false
        }))
        .send()
        .await
        .unwrap()
}

async fn send_remote_image_chat_request(proxy_base: &str) -> reqwest::Response {
    send_openai_chat_image_request(proxy_base, REMOTE_IMAGE_URL).await
}

async fn send_openai_chat_image_request(proxy_base: &str, image_url: &str) -> reqwest::Response {
    authenticated_reqwest_client()
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "multimodal-test",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "Describe this remote image" },
                    { "type": "image_url", "image_url": { "url": image_url } }
                ]
            }],
            "stream": false
        }))
        .send()
        .await
        .unwrap()
}

async fn assert_success_response(response: reqwest::Response) -> Value {
    let status = response.status();
    let body = response.text().await.unwrap();
    assert!(status.is_success(), "status: {status}, body: {body}");
    serde_json::from_str(&body).unwrap_or_else(|err| panic!("invalid JSON response: {err}; {body}"))
}

async fn assert_failure_response(response: reqwest::Response) -> Value {
    let status = response.status();
    let body = response.text().await.unwrap();
    assert!(
        !status.is_success(),
        "request should fail closed before upstream; status: {status}, body: {body}"
    );
    serde_json::from_str(&body).unwrap_or(Value::String(body))
}

async fn assert_no_upstream_request(captured: &common::mock_upstream::CapturedMockRequests) {
    let requests = captured.wait_for_count(1, Duration::from_millis(150)).await;
    assert!(
        requests.is_empty(),
        "fail-closed path must not contact upstream: {requests:?}"
    );
}

fn assert_anthropic_image_base64_source(request: &CapturedMockRequest) -> Result<(), String> {
    expect_pointer(&request.body, "/messages/0/role", json!("user"))?;
    expect_pointer(&request.body, "/messages/0/content/0/type", json!("text"))?;
    expect_pointer(
        &request.body,
        "/messages/0/content/0/text",
        json!("Describe this image"),
    )?;
    expect_pointer(&request.body, "/messages/0/content/1/type", json!("image"))?;
    expect_pointer(
        &request.body,
        "/messages/0/content/1/source/type",
        json!("base64"),
    )?;
    expect_pointer(
        &request.body,
        "/messages/0/content/1/source/media_type",
        json!("image/png"),
    )?;
    expect_pointer(
        &request.body,
        "/messages/0/content/1/source/data",
        json!(PNG_B64),
    )
}

fn assert_anthropic_image_url_source(request: &CapturedMockRequest) -> Result<(), String> {
    expect_pointer(&request.body, "/messages/0/role", json!("user"))?;
    expect_pointer(&request.body, "/messages/0/content/0/type", json!("text"))?;
    expect_pointer(
        &request.body,
        "/messages/0/content/0/text",
        json!("Describe this remote image"),
    )?;
    expect_pointer(&request.body, "/messages/0/content/1/type", json!("image"))?;
    expect_pointer(
        &request.body,
        "/messages/0/content/1/source/type",
        json!("url"),
    )?;
    expect_pointer(
        &request.body,
        "/messages/0/content/1/source/url",
        json!(REMOTE_IMAGE_URL),
    )
}

fn expect_pointer(body: &Value, pointer: &str, expected: Value) -> Result<(), String> {
    let actual = body
        .pointer(pointer)
        .ok_or_else(|| format!("missing JSON pointer `{pointer}` in body: {body}"))?;
    if actual == &expected {
        Ok(())
    } else {
        Err(format!(
            "JSON pointer `{pointer}` expected {expected}, got {actual}; body: {body}"
        ))
    }
}
