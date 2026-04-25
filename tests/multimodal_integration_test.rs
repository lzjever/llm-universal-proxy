//! Multimodal e2e contract tests backed by first-party spec-aware mock upstreams.

mod common;

use common::mock_upstream::{
    spawn_asserting_anthropic_mock, spawn_asserting_google_mock, CapturedMockRequest,
};
use common::proxy_helpers::proxy_config;
use common::runtime_proxy::start_proxy;
use llm_universal_proxy::formats::UpstreamFormat;
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;

const PNG_B64: &str = "iVBORw0KGgo=";
const PNG_DATA_URI: &str = "data:image/png;base64,iVBORw0KGgo=";
const AUDIO_WAV_B64: &str = "UklGRiQAAABXQVZFZm10IBAAAAABAAEAESsAACJWAAACABAAZGF0YQAAAAA=";
const PDF_B64: &str = "JVBERi0x";
const PDF_DATA_URI: &str = "data:application/pdf;base64,JVBERi0x";
const GEMINI_FILE_URI: &str = "gs://llmup-test/doc.pdf";

#[tokio::test]
async fn multimodal_openai_chat_to_gemini_maps_inline_and_file_data() {
    let (mock_base, _mock, captured) =
        spawn_asserting_google_mock(assert_gemini_multimodal_parts).await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let response = Client::new()
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gemini-2.5-flash",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "Inspect these multimodal inputs" },
                    { "type": "image_url", "image_url": { "url": PNG_DATA_URI } },
                    { "type": "input_audio", "input_audio": { "data": AUDIO_WAV_B64, "format": "wav" } },
                    { "type": "file", "file": { "file_data": PDF_DATA_URI, "filename": "fixture.pdf" } },
                    { "type": "file", "file": { "file_data": GEMINI_FILE_URI, "filename": "remote.pdf" } }
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
async fn multimodal_responses_to_gemini_maps_inline_and_file_data() {
    let (mock_base, _mock, captured) =
        spawn_asserting_google_mock(assert_gemini_multimodal_parts).await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let response = Client::new()
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "gemini-2.5-flash",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [
                    { "type": "input_text", "text": "Inspect these multimodal inputs" },
                    { "type": "input_image", "image_url": PNG_DATA_URI },
                    { "type": "input_audio", "input_audio": { "data": AUDIO_WAV_B64, "format": "wav" } },
                    { "type": "input_file", "file_data": PDF_DATA_URI, "filename": "fixture.pdf" },
                    { "type": "input_file", "file_data": GEMINI_FILE_URI, "filename": "remote.pdf" }
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
async fn multimodal_openai_chat_to_anthropic_maps_data_uri_image_to_base64_source() {
    let (mock_base, _mock, captured) =
        spawn_asserting_anthropic_mock(assert_anthropic_image_base64_source).await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let response = Client::new()
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
async fn multimodal_openai_remote_image_to_gemini_fails_closed_before_upstream() {
    let (mock_base, _mock, captured) =
        spawn_asserting_google_mock(|_| Err("remote image reached Gemini upstream".to_string()))
            .await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let response = send_remote_image_chat_request(&proxy_base).await;
    assert_failure_response(response).await;
    assert_no_upstream_request(&captured).await;
}

#[tokio::test]
async fn multimodal_openai_remote_image_to_anthropic_fails_closed_before_upstream() {
    let (mock_base, _mock, captured) = spawn_asserting_anthropic_mock(|_| {
        Err("remote image reached Anthropic upstream".to_string())
    })
    .await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let response = send_remote_image_chat_request(&proxy_base).await;
    assert_failure_response(response).await;
    assert_no_upstream_request(&captured).await;
}

#[tokio::test]
async fn multimodal_openai_audio_and_file_to_anthropic_fail_closed_before_upstream() {
    for (label, content) in [
        (
            "audio",
            json!([
                { "type": "text", "text": "Transcribe this audio" },
                { "type": "input_audio", "input_audio": { "data": AUDIO_WAV_B64, "format": "wav" } }
            ]),
        ),
        (
            "file",
            json!([
                { "type": "text", "text": "Summarize this file" },
                { "type": "file", "file": { "file_data": PDF_DATA_URI, "filename": "fixture.pdf" } }
            ]),
        ),
    ] {
        let (mock_base, _mock, captured) = spawn_asserting_anthropic_mock(move |_| {
            Err(format!("{label} reached Anthropic upstream"))
        })
        .await;
        let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
        let (proxy_base, _proxy) = start_proxy(config).await;

        let response = Client::new()
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

async fn send_remote_image_chat_request(proxy_base: &str) -> reqwest::Response {
    Client::new()
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "multimodal-test",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "Describe this remote image" },
                    { "type": "image_url", "image_url": { "url": "https://example.com/cat.png" } }
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

fn assert_gemini_multimodal_parts(request: &CapturedMockRequest) -> Result<(), String> {
    expect_pointer(&request.body, "/contents/0/role", json!("user"))?;
    expect_pointer(
        &request.body,
        "/contents/0/parts/0/text",
        json!("Inspect these multimodal inputs"),
    )?;
    expect_pointer(
        &request.body,
        "/contents/0/parts/1/inlineData/mimeType",
        json!("image/png"),
    )?;
    expect_pointer(
        &request.body,
        "/contents/0/parts/1/inlineData/data",
        json!(PNG_B64),
    )?;
    expect_pointer(
        &request.body,
        "/contents/0/parts/2/inlineData/mimeType",
        json!("audio/wav"),
    )?;
    expect_pointer(
        &request.body,
        "/contents/0/parts/2/inlineData/data",
        json!(AUDIO_WAV_B64),
    )?;
    expect_pointer(
        &request.body,
        "/contents/0/parts/3/inlineData/mimeType",
        json!("application/pdf"),
    )?;
    expect_pointer(
        &request.body,
        "/contents/0/parts/3/inlineData/data",
        json!(PDF_B64),
    )?;
    expect_pointer(
        &request.body,
        "/contents/0/parts/4/fileData/fileUri",
        json!(GEMINI_FILE_URI),
    )?;
    expect_pointer(
        &request.body,
        "/contents/0/parts/4/fileData/mimeType",
        json!("application/pdf"),
    )?;
    expect_pointer(
        &request.body,
        "/contents/0/parts/4/fileData/displayName",
        json!("remote.pdf"),
    )
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
