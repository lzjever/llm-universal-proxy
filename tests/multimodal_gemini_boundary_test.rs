//! Multimodal boundary e2e coverage for Gemini-facing translations.

mod common;

use common::mock_upstream::{
    spawn_asserting_google_mock, spawn_asserting_openai_completion_mock,
    spawn_asserting_openai_responses_mock, CapturedMockRequest, CapturedMockRequests,
};
use common::proxy_helpers::proxy_config;
use common::runtime_proxy::start_proxy;
use llm_universal_proxy::formats::UpstreamFormat;
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;

const GEMINI_FILE_URI: &str = "gs://llmup-test/policy.pdf";
const PNG_B64: &str = "iVBORw0KGgo=";
const PDF_B64: &str = "JVBERi0x";
const REMOTE_IMAGE_URL: &str = "https://example.test/assets/cat.png";
const REMOTE_PDF_URL: &str = "https://example.test/papers/policy.pdf";

#[tokio::test]
async fn anthropic_image_base64_to_gemini_uses_inline_data() {
    let (mock_base, _mock, captured) =
        spawn_asserting_google_mock(assert_gemini_inline_image_part).await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let response = Client::new()
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .header("anthropic-version", "2023-06-01")
        .json(&json!({
            "model": "gemini-2.5-flash",
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "Describe this image" },
                    { "type": "image", "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": PNG_B64
                    }}
                ]
            }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();

    assert_success_response(response).await;
    assert_upstream_called_once(&captured).await;
}

#[tokio::test]
async fn anthropic_remote_image_url_to_gemini_fails_closed_before_upstream() {
    let (mock_base, _mock, captured) =
        spawn_asserting_google_mock(|_| Err("remote image reached Gemini upstream".to_string()))
            .await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let response = Client::new()
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .header("anthropic-version", "2023-06-01")
        .json(&json!({
            "model": "gemini-2.5-flash",
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "Describe this remote image" },
                    { "type": "image", "source": {
                        "type": "url",
                        "url": REMOTE_IMAGE_URL,
                        "media_type": "image/png"
                    }}
                ]
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
async fn gemini_file_data_gs_uri_to_openai_chat_fails_closed_before_upstream() {
    let (mock_base, _mock, captured) = spawn_asserting_openai_completion_mock(|_| {
        Err("Gemini fileData reached OpenAI Chat upstream".to_string())
    })
    .await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let response = Client::new()
        .post(format!(
            "{proxy_base}/google/v1beta/models/gpt-4o:generateContent"
        ))
        .json(&json!({
            "model": "gpt-4o",
            "contents": [{
                "role": "user",
                "parts": [
                    { "text": "Read this PDF" },
                    { "fileData": {
                        "mimeType": "application/pdf",
                        "fileUri": GEMINI_FILE_URI,
                        "displayName": "policy.pdf"
                    }}
                ]
            }]
        }))
        .send()
        .await
        .unwrap();

    assert_failure_response(response).await;
    assert_no_upstream_request(&captured).await;
}

#[tokio::test]
async fn gemini_file_data_http_uri_to_openai_chat_and_responses_fails_closed_before_upstream() {
    for (label, target, upstream_format) in [
        (
            "Gemini HTTP fileData reached OpenAI Chat upstream",
            "OpenAI Chat",
            UpstreamFormat::OpenAiCompletion,
        ),
        (
            "Gemini HTTP fileData reached OpenAI Responses upstream",
            "OpenAI Responses",
            UpstreamFormat::OpenAiResponses,
        ),
    ] {
        match upstream_format {
            UpstreamFormat::OpenAiCompletion => {
                let (mock_base, _mock, captured) =
                    spawn_asserting_openai_completion_mock(move |_| Err(label.to_string())).await;
                let config = proxy_config(&mock_base, upstream_format);
                let (proxy_base, _proxy) = start_proxy(config).await;

                let response = Client::new()
                    .post(format!(
                        "{proxy_base}/google/v1beta/models/gpt-4o:generateContent"
                    ))
                    .json(&json!({
                        "model": "gpt-4o",
                        "contents": [{
                            "role": "user",
                            "parts": [
                                { "text": format!("Read this PDF with {target}") },
                                { "fileData": {
                                    "mimeType": "application/pdf",
                                    "fileUri": REMOTE_PDF_URL,
                                    "displayName": "policy.pdf"
                                }}
                            ]
                        }]
                    }))
                    .send()
                    .await
                    .unwrap();

                assert_failure_response(response).await;
                assert_no_upstream_request(&captured).await;
            }
            UpstreamFormat::OpenAiResponses => {
                let (mock_base, _mock, captured) =
                    spawn_asserting_openai_responses_mock(move |_| Err(label.to_string())).await;
                let config = proxy_config(&mock_base, upstream_format);
                let (proxy_base, _proxy) = start_proxy(config).await;

                let response = Client::new()
                    .post(format!(
                        "{proxy_base}/google/v1beta/models/gpt-4o:generateContent"
                    ))
                    .json(&json!({
                        "model": "gpt-4o",
                        "contents": [{
                            "role": "user",
                            "parts": [
                                { "text": format!("Read this PDF with {target}") },
                                { "fileData": {
                                    "mimeType": "application/pdf",
                                    "fileUri": REMOTE_PDF_URL,
                                    "displayName": "policy.pdf"
                                }}
                            ]
                        }]
                    }))
                    .send()
                    .await
                    .unwrap();

                assert_failure_response(response).await;
                assert_no_upstream_request(&captured).await;
            }
            _ => unreachable!("only OpenAI targets are covered here"),
        }
    }
}

#[tokio::test]
async fn gemini_file_data_gs_uri_to_openai_responses_fails_closed_before_upstream() {
    let (mock_base, _mock, captured) = spawn_asserting_openai_responses_mock(|_| {
        Err("Gemini fileData reached OpenAI Responses upstream".to_string())
    })
    .await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let response = Client::new()
        .post(format!(
            "{proxy_base}/google/v1beta/models/gpt-4o:generateContent"
        ))
        .json(&json!({
            "model": "gpt-4o",
            "contents": [{
                "role": "user",
                "parts": [
                    { "text": "Read this PDF" },
                    { "fileData": {
                        "mimeType": "application/pdf",
                        "fileUri": GEMINI_FILE_URI,
                        "displayName": "policy.pdf"
                    }}
                ]
            }]
        }))
        .send()
        .await
        .unwrap();

    assert_failure_response(response).await;
    assert_no_upstream_request(&captured).await;
}

#[tokio::test]
async fn gemini_inline_data_image_and_pdf_to_openai_chat_still_succeeds() {
    let (mock_base, _mock, captured) =
        spawn_asserting_openai_completion_mock(assert_openai_chat_inline_data_media).await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let response = Client::new()
        .post(format!(
            "{proxy_base}/google/v1beta/models/gpt-4o:generateContent"
        ))
        .json(&json!({
            "model": "gpt-4o",
            "contents": [{
                "role": "user",
                "parts": [
                    { "text": "Inspect these inline files" },
                    { "inlineData": { "mimeType": "image/png", "data": PNG_B64 } },
                    { "inlineData": { "mimeType": "application/pdf", "data": PDF_B64 } }
                ]
            }]
        }))
        .send()
        .await
        .unwrap();

    assert_success_response(response).await;
    assert_upstream_called_once(&captured).await;
}

#[tokio::test]
async fn gemini_inline_data_image_and_pdf_to_openai_responses_still_succeeds() {
    let (mock_base, _mock, captured) =
        spawn_asserting_openai_responses_mock(assert_openai_responses_inline_data_media).await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let response = Client::new()
        .post(format!(
            "{proxy_base}/google/v1beta/models/gpt-4o:generateContent"
        ))
        .json(&json!({
            "model": "gpt-4o",
            "contents": [{
                "role": "user",
                "parts": [
                    { "text": "Inspect these inline files" },
                    { "inlineData": { "mimeType": "image/png", "data": PNG_B64 } },
                    { "inlineData": { "mimeType": "application/pdf", "data": PDF_B64 } }
                ]
            }]
        }))
        .send()
        .await
        .unwrap();

    assert_success_response(response).await;
    assert_upstream_called_once(&captured).await;
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

async fn assert_upstream_called_once(captured: &CapturedMockRequests) {
    let requests = captured.wait_for_count(1, Duration::from_secs(1)).await;
    assert_eq!(requests.len(), 1, "upstream request count: {requests:?}");
}

async fn assert_no_upstream_request(captured: &CapturedMockRequests) {
    let requests = captured.wait_for_count(1, Duration::from_millis(150)).await;
    assert!(
        requests.is_empty(),
        "fail-closed path must not contact upstream: {requests:?}"
    );
}

fn assert_gemini_inline_image_part(request: &CapturedMockRequest) -> Result<(), String> {
    expect_pointer(&request.body, "/contents/0/role", json!("user"))?;
    expect_pointer(
        &request.body,
        "/contents/0/parts/0/text",
        json!("Describe this image"),
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
    )
}

fn assert_openai_chat_inline_data_media(request: &CapturedMockRequest) -> Result<(), String> {
    expect_pointer(&request.body, "/messages/0/role", json!("user"))?;
    expect_pointer(
        &request.body,
        "/messages/0/content/0/text",
        json!("Inspect these inline files"),
    )?;
    expect_pointer(
        &request.body,
        "/messages/0/content/1/type",
        json!("image_url"),
    )?;
    expect_pointer(
        &request.body,
        "/messages/0/content/1/image_url/url",
        json!(format!("data:image/png;base64,{PNG_B64}")),
    )?;
    expect_pointer(&request.body, "/messages/0/content/2/type", json!("file"))?;
    expect_pointer(
        &request.body,
        "/messages/0/content/2/file/file_data",
        json!(format!("data:application/pdf;base64,{PDF_B64}")),
    )?;
    expect_pointer(
        &request.body,
        "/messages/0/content/2/file/mime_type",
        json!("application/pdf"),
    )
}

fn assert_openai_responses_inline_data_media(request: &CapturedMockRequest) -> Result<(), String> {
    expect_pointer(&request.body, "/input/0/type", json!("message"))?;
    expect_pointer(&request.body, "/input/0/role", json!("user"))?;
    expect_pointer(
        &request.body,
        "/input/0/content/0/text",
        json!("Inspect these inline files"),
    )?;
    expect_pointer(
        &request.body,
        "/input/0/content/1/type",
        json!("input_image"),
    )?;
    expect_pointer(
        &request.body,
        "/input/0/content/1/image_url",
        json!(format!("data:image/png;base64,{PNG_B64}")),
    )?;
    expect_pointer(
        &request.body,
        "/input/0/content/2/type",
        json!("input_file"),
    )?;
    expect_pointer(
        &request.body,
        "/input/0/content/2/file_data",
        json!(format!("data:application/pdf;base64,{PDF_B64}")),
    )?;
    expect_pointer(
        &request.body,
        "/input/0/content/2/mime_type",
        json!("application/pdf"),
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
