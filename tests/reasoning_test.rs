//! Cross-Format Reasoning/Thinking Integration Tests
//!
//! Tests the full matrix of reasoning/thinking translation across all 4 protocols:
//! - OpenAI Chat Completions (reasoning_content)
//! - OpenAI Responses (reasoning output items)
//! - Anthropic Messages (thinking blocks)
//! - Google Gemini (thought parts)
//!
//! Also tests Gap 1-4 fixes from the reasoning audit.

mod common;

use common::*;
use llm_universal_proxy::formats::UpstreamFormat;
use reqwest::Client;
use serde_json::json;
use serde_json::Value;

fn parse_sse_payloads(body: &str) -> Vec<Value> {
    body.split("\n\n")
        .filter_map(|event| {
            let data = event.lines().find_map(|line| line.strip_prefix("data: "))?;
            serde_json::from_str::<Value>(data).ok()
        })
        .collect()
}

async fn assert_reasoning_to_anthropic_preserved_as_unsigned_thinking(
    res: reqwest::Response,
    expected_thinking: &str,
    expected_text: &str,
    expected_output_tokens: u64,
) {
    let status = res.status();
    let body: Value = res.json().await.unwrap();
    assert_eq!(status, reqwest::StatusCode::OK, "body = {body:?}");
    assert_eq!(body["type"], "message", "body = {body:?}");
    assert_eq!(body["role"], "assistant", "body = {body:?}");
    assert_eq!(body["stop_reason"], "end_turn", "body = {body:?}");
    assert!(body.get("error").is_none(), "body = {body:?}");

    let content = body["content"].as_array().expect("anthropic content array");
    assert_eq!(content.len(), 2, "body = {body:?}");
    assert_eq!(content[0]["type"], "thinking", "body = {body:?}");
    assert_eq!(content[0]["thinking"], expected_thinking, "body = {body:?}");
    assert!(content[0].get("signature").is_none(), "body = {body:?}");
    assert_eq!(content[1]["type"], "text", "body = {body:?}");
    assert_eq!(content[1]["text"], expected_text, "body = {body:?}");
    assert_eq!(body["usage"]["input_tokens"], 1, "body = {body:?}");
    assert_eq!(
        body["usage"]["output_tokens"], expected_output_tokens,
        "body = {body:?}"
    );
}

async fn assert_reasoning_to_anthropic_stream_preserved_as_unsigned_thinking(
    res: reqwest::Response,
    expected_thinking: &str,
    expected_text: &str,
) {
    let status = res.status();
    let body = res.text().await.unwrap();
    assert_eq!(status, reqwest::StatusCode::OK, "body = {body}");
    let events = parse_sse_payloads(&body);
    assert_eq!(events[0]["type"], "message_start", "body = {body}");
    assert_eq!(events[1]["type"], "content_block_start", "body = {body}");
    assert_eq!(
        events[1]["content_block"]["type"], "thinking",
        "body = {body}"
    );
    assert_eq!(events[2]["type"], "content_block_delta", "body = {body}");
    assert_eq!(
        events[2]["delta"]["type"], "thinking_delta",
        "body = {body}"
    );
    assert_eq!(
        events[2]["delta"]["thinking"], expected_thinking,
        "body = {body}"
    );
    assert_eq!(events[3]["type"], "content_block_stop", "body = {body}");
    assert_eq!(events[4]["type"], "content_block_start", "body = {body}");
    assert_eq!(events[4]["content_block"]["type"], "text", "body = {body}");
    assert_eq!(events[5]["type"], "content_block_delta", "body = {body}");
    assert_eq!(events[5]["delta"]["type"], "text_delta", "body = {body}");
    assert_eq!(events[5]["delta"]["text"], expected_text, "body = {body}");
    assert_eq!(events[6]["type"], "content_block_stop", "body = {body}");
    assert_eq!(events[7]["type"], "message_delta", "body = {body}");
    assert_eq!(
        events[7]["delta"]["stop_reason"], "end_turn",
        "body = {body}"
    );
    assert_eq!(events[8]["type"], "message_stop", "body = {body}");
    assert!(
        events.iter().all(|event| event["type"] != "error"),
        "body = {body}"
    );
}

// ============================================================
// A. Non-Streaming Cross-Format Reasoning
// ============================================================

#[tokio::test]
async fn anthropic_thinking_to_openai_chat_non_streaming() {
    let (mock_base, _mock) = spawn_anthropic_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "status: {status}",
        status = res.status()
    );
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["choices"][0]["message"]["reasoning_content"], "think");
    assert_eq!(body["choices"][0]["message"]["content"], "Hi");
    assert_eq!(body["choices"][0]["finish_reason"], "stop");
}

#[tokio::test]
async fn anthropic_thinking_to_gemini_non_streaming() {
    let (mock_base, _mock) = spawn_anthropic_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!(
            "{proxy_base}/google/v1beta/models/test:generateContent"
        ))
        .json(&json!({ "contents": [{ "parts": [{ "text": "Hi" }] }] }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "status: {status}",
        status = res.status()
    );
    let body: Value = res.json().await.unwrap();
    let parts = body["candidates"][0]["content"]["parts"]
        .as_array()
        .unwrap();
    assert_eq!(parts[0]["thought"], true);
    assert_eq!(parts[0]["text"], "think");
    assert_eq!(parts[1]["text"], "Hi");
}

#[tokio::test]
async fn anthropic_thinking_to_responses_non_streaming() {
    let (mock_base, _mock) = spawn_anthropic_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "claude-3",
            "input": "Hi",
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "status: {status}",
        status = res.status()
    );
    let body: Value = res.json().await.unwrap();
    let output = body["output"].as_array().unwrap();
    assert_eq!(output[0]["type"], "reasoning");
    assert_eq!(output[0]["summary"][0]["text"], "think");
    assert!(output.iter().any(|o| o["type"] == "message"));
}

#[tokio::test]
async fn anthropic_signed_thinking_to_responses_non_streaming_returns_carrier() {
    let (mock_base, _mock) = spawn_anthropic_signed_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "claude-3",
            "input": "Hi",
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "status: {status}",
        status = res.status()
    );
    let body: Value = res.json().await.unwrap();
    let output = body["output"].as_array().unwrap();
    assert_eq!(output[0]["type"], "reasoning");
    assert_eq!(output[0]["summary"][0]["text"], "internal reasoning");
    assert!(
        output[0]["encrypted_content"].is_string(),
        "body = {body:?}"
    );
    assert_eq!(output[1]["type"], "message");
    assert_eq!(output[1]["content"][0]["text"], "Visible answer");
}

#[tokio::test]
async fn anthropic_signed_thinking_responses_round_trip_non_streaming_replays_carrier_to_upstream()
{
    let (source_base, _source_mock) = spawn_anthropic_signed_thinking_mock().await;
    let source_config = proxy_config(&source_base, UpstreamFormat::Anthropic);
    let (source_proxy_base, _source_proxy) = start_proxy(source_config).await;

    let client = Client::new();
    let first_response = client
        .post(format!("{source_proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "claude-3",
            "input": "Hi",
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(first_response.status().is_success());
    let first_body: Value = first_response.json().await.unwrap();
    let output = first_body["output"].as_array().expect("responses output");
    let reasoning_item = output[0].clone();
    let message_item = output[1].clone();

    let (capture_base, _capture_mock, mut captured) = spawn_capture_anthropic_mock().await;
    let capture_config = proxy_config(&capture_base, UpstreamFormat::Anthropic);
    let (capture_proxy_base, _capture_proxy) = start_proxy(capture_config).await;

    let second_response = client
        .post(format!("{capture_proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "claude-3",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": "Think about it" }]
                },
                reasoning_item,
                message_item,
                {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": "Continue" }]
                }
            ],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(
        second_response.status().is_success(),
        "status: {status}",
        status = second_response.status()
    );
    let _: Value = second_response.json().await.unwrap();

    captured.changed().await.unwrap();
    let request = captured
        .borrow()
        .clone()
        .expect("captured anthropic request");
    let messages = request["messages"].as_array().expect("anthropic messages");
    assert_eq!(messages[1]["role"], "assistant");
    let assistant_content = messages[1]["content"]
        .as_array()
        .expect("assistant content");
    assert_eq!(assistant_content[0]["type"], "thinking");
    assert_eq!(assistant_content[0]["thinking"], "internal reasoning");
    assert_eq!(assistant_content[0]["signature"], "sig_123");
    assert_eq!(assistant_content[1]["type"], "text");
    assert_eq!(assistant_content[1]["text"], "Visible answer");
    assert_eq!(messages[2]["role"], "user");
    assert_eq!(messages[2]["content"][0]["text"], "Continue");
}

#[tokio::test]
async fn anthropic_omitted_thinking_responses_round_trip_non_streaming_replays_carrier_to_upstream()
{
    let (source_base, _source_mock) = spawn_anthropic_omitted_thinking_mock().await;
    let source_config = proxy_config(&source_base, UpstreamFormat::Anthropic);
    let (source_proxy_base, _source_proxy) = start_proxy(source_config).await;

    let client = Client::new();
    let first_response = client
        .post(format!("{source_proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "claude-3",
            "input": "Hi",
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(first_response.status().is_success());
    let first_body: Value = first_response.json().await.unwrap();
    let output = first_body["output"].as_array().expect("responses output");
    assert_eq!(output[0]["type"], "reasoning");
    assert_eq!(output[0]["summary"], json!([]));
    assert!(
        output[0]["encrypted_content"].is_string(),
        "body = {first_body:?}"
    );
    let reasoning_item = output[0].clone();
    let message_item = output[1].clone();

    let (capture_base, _capture_mock, mut captured) = spawn_capture_anthropic_mock().await;
    let capture_config = proxy_config(&capture_base, UpstreamFormat::Anthropic);
    let (capture_proxy_base, _capture_proxy) = start_proxy(capture_config).await;

    let second_response = client
        .post(format!("{capture_proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "claude-3",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": "Think about it" }]
                },
                reasoning_item,
                message_item,
                {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": "Continue" }]
                }
            ],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(
        second_response.status().is_success(),
        "status: {status}",
        status = second_response.status()
    );
    let _: Value = second_response.json().await.unwrap();

    captured.changed().await.unwrap();
    let request = captured
        .borrow()
        .clone()
        .expect("captured anthropic request");
    let messages = request["messages"].as_array().expect("anthropic messages");
    assert_eq!(messages[1]["role"], "assistant");
    let assistant_content = messages[1]["content"]
        .as_array()
        .expect("assistant content");
    assert_eq!(assistant_content[0]["type"], "thinking");
    assert_eq!(
        assistant_content[0]["thinking"],
        json!({ "display": "omitted" })
    );
    assert_eq!(assistant_content[0]["signature"], "sig_omitted");
    assert_eq!(assistant_content[1]["type"], "text");
    assert_eq!(assistant_content[1]["text"], "Visible answer");
    assert_eq!(messages[2]["role"], "user");
    assert_eq!(messages[2]["content"][0]["text"], "Continue");
}

#[tokio::test]
async fn openai_reasoning_to_anthropic_non_streaming_preserves_unsigned_thinking_without_provenance()
{
    let (mock_base, _mock) = spawn_openai_completion_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "mock",
            "max_tokens": 256,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_reasoning_to_anthropic_preserved_as_unsigned_thinking(res, "think", "Hi", 3).await;
}

#[tokio::test]
async fn openai_reasoning_to_responses_non_streaming() {
    let (mock_base, _mock) = spawn_openai_completion_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({ "model": "mock", "input": "Hi", "stream": false }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "status: {status}",
        status = res.status()
    );
    let body: Value = res.json().await.unwrap();
    let output = body["output"].as_array().unwrap();
    assert_eq!(output[0]["type"], "reasoning");
    assert_eq!(output[0]["summary"][0]["text"], "think");
    assert!(output.iter().any(|o| o["type"] == "message"));
}

#[tokio::test]
async fn openai_reasoning_to_gemini_non_streaming() {
    let (mock_base, _mock) = spawn_openai_completion_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!(
            "{proxy_base}/google/v1beta/models/test:generateContent"
        ))
        .json(&json!({ "contents": [{ "parts": [{ "text": "Hi" }] }] }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "status: {status}",
        status = res.status()
    );
    let body: Value = res.json().await.unwrap();
    let parts = body["candidates"][0]["content"]["parts"]
        .as_array()
        .unwrap();
    assert_eq!(parts[0]["thought"], true);
    assert_eq!(parts[0]["text"], "think");
    assert_eq!(parts[1]["text"], "Hi");
}

// Gap 1 fix: Responses API reasoning → other formats
#[tokio::test]
async fn responses_reasoning_to_openai_chat_non_streaming() {
    let (mock_base, _mock) = spawn_openai_responses_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "mock",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "status: {status}",
        status = res.status()
    );
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["choices"][0]["message"]["reasoning_content"], "think");
    assert_eq!(body["choices"][0]["message"]["content"], "Hi");
}

#[tokio::test]
async fn responses_reasoning_to_anthropic_non_streaming_preserves_unsigned_thinking_without_provenance()
{
    let (mock_base, _mock) = spawn_openai_responses_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "mock",
            "max_tokens": 256,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_reasoning_to_anthropic_preserved_as_unsigned_thinking(res, "think", "Hi", 2).await;
}

#[tokio::test]
async fn responses_reasoning_to_gemini_non_streaming() {
    let (mock_base, _mock) = spawn_openai_responses_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!(
            "{proxy_base}/google/v1beta/models/test:generateContent"
        ))
        .json(&json!({ "contents": [{ "parts": [{ "text": "Hi" }] }] }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "status: {status}",
        status = res.status()
    );
    let body: Value = res.json().await.unwrap();
    let parts = body["candidates"][0]["content"]["parts"]
        .as_array()
        .unwrap();
    assert_eq!(parts[0]["thought"], true);
    assert_eq!(parts[0]["text"], "think");
    assert_eq!(parts[1]["text"], "Hi");
}

#[tokio::test]
async fn gemini_thinking_to_openai_chat_non_streaming() {
    let (mock_base, _mock) = spawn_google_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gemini-test",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "status: {status}",
        status = res.status()
    );
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["choices"][0]["message"]["reasoning_content"], "think");
    assert_eq!(body["choices"][0]["message"]["content"], "Hi");
}

#[tokio::test]
async fn gemini_thinking_to_anthropic_non_streaming_preserves_unsigned_thinking_without_provenance()
{
    let (mock_base, _mock) = spawn_google_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "gemini-test",
            "max_tokens": 256,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_reasoning_to_anthropic_preserved_as_unsigned_thinking(res, "think", "Hi", 2).await;
}

#[tokio::test]
async fn gemini_thinking_to_responses_non_streaming() {
    let (mock_base, _mock) = spawn_google_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({ "model": "gemini-test", "input": "Hi", "stream": false }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "status: {status}",
        status = res.status()
    );
    let body: Value = res.json().await.unwrap();
    let output = body["output"].as_array().unwrap();
    assert_eq!(output[0]["type"], "reasoning");
    assert_eq!(output[0]["summary"][0]["text"], "think");
    assert!(output.iter().any(|o| o["type"] == "message"));
}

// ============================================================
// B. Streaming Cross-Format Reasoning
// ============================================================

#[tokio::test]
async fn anthropic_thinking_to_openai_chat_streaming() {
    let (mock_base, _mock) = spawn_anthropic_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let text = res.text().await.unwrap();
    assert!(text.contains("reasoning_content"), "body = {text}");
    assert!(text.contains("\"content\":\"Hi\""), "body = {text}");
    assert!(
        !text.contains("\"finish_reason\":\"error\""),
        "body = {text}"
    );
}

#[tokio::test]
async fn anthropic_thinking_to_responses_streaming() {
    let (mock_base, _mock) = spawn_anthropic_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({ "model": "claude-3", "input": "Hi", "stream": true }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let text = res.text().await.unwrap();
    assert!(
        text.contains("response.reasoning_summary_text.delta"),
        "body = {text}"
    );
    assert!(text.contains("response.output_text.delta"), "body = {text}");
    assert!(text.contains("response.completed"), "body = {text}");
    assert!(!text.contains("response.failed"), "body = {text}");
}

#[tokio::test]
async fn anthropic_signed_thinking_to_openai_chat_streaming_drops_signature_and_continues() {
    let (mock_base, _mock) = spawn_anthropic_signed_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let text = res.text().await.unwrap();
    assert!(text.contains("reasoning_content"), "body = {text}");
    assert!(text.contains("internal reasoning"), "body = {text}");
    assert!(
        text.contains("\"content\":\"Visible answer\""),
        "body = {text}"
    );
    assert!(
        !text.contains("\"finish_reason\":\"error\""),
        "body = {text}"
    );
}

#[tokio::test]
async fn anthropic_omitted_thinking_to_openai_chat_streaming_skips_hidden_reasoning_and_continues()
{
    let (mock_base, _mock) = spawn_anthropic_omitted_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let text = res.text().await.unwrap();
    assert!(
        text.contains("\"content\":\"Visible answer\""),
        "body = {text}"
    );
    assert!(!text.contains("reasoning_content"), "body = {text}");
    assert!(
        !text.contains("\"finish_reason\":\"error\""),
        "body = {text}"
    );
}

#[tokio::test]
async fn openai_reasoning_to_anthropic_streaming_preserves_unsigned_thinking_without_provenance() {
    let (mock_base, _mock) = spawn_openai_completion_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "mock",
            "max_tokens": 256,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_reasoning_to_anthropic_stream_preserved_as_unsigned_thinking(res, "think", "Hi").await;
}

#[tokio::test]
async fn openai_reasoning_to_responses_streaming() {
    let (mock_base, _mock) = spawn_openai_completion_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({ "model": "mock", "input": "Hi", "stream": true }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let text = res.text().await.unwrap();
    assert!(
        text.contains("response.reasoning_summary_text.delta"),
        "body = {text}"
    );
}

#[tokio::test]
async fn responses_reasoning_to_openai_chat_streaming() {
    let (mock_base, _mock) = spawn_openai_responses_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "mock",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let text = res.text().await.unwrap();
    assert!(text.contains("reasoning_content"), "body = {text}");
}

#[tokio::test]
async fn responses_reasoning_to_anthropic_streaming_preserves_unsigned_thinking_without_provenance()
{
    let (mock_base, _mock) = spawn_openai_responses_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "mock",
            "max_tokens": 256,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_reasoning_to_anthropic_stream_preserved_as_unsigned_thinking(res, "think", "Hi").await;
}

#[tokio::test]
async fn responses_reasoning_with_encrypted_carrier_to_anthropic_streaming_degrades_without_error()
{
    let (mock_base, _mock) = spawn_openai_responses_reasoning_with_encrypted_carrier_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "mock",
            "max_tokens": 256,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_reasoning_to_anthropic_stream_preserved_as_unsigned_thinking(res, "think", "Hi").await;
}

#[tokio::test]
async fn gemini_thinking_to_openai_chat_streaming() {
    let (mock_base, _mock) = spawn_google_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gemini-test",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let text = res.text().await.unwrap();
    assert!(text.contains("reasoning_content"), "body = {text}");
}

#[tokio::test]
async fn gemini_thinking_to_anthropic_streaming_preserves_unsigned_thinking_without_provenance() {
    let (mock_base, _mock) = spawn_google_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "gemini-test",
            "max_tokens": 256,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_reasoning_to_anthropic_stream_preserved_as_unsigned_thinking(res, "think", "Hi").await;
}

#[tokio::test]
async fn gemini_thinking_to_responses_streaming() {
    let (mock_base, _mock) = spawn_google_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({ "model": "gemini-test", "input": "Hi", "stream": true }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let text = res.text().await.unwrap();
    assert!(
        text.contains("response.reasoning_summary_text.delta")
            || text.contains("response.completed"),
        "body = {text}"
    );
}

#[tokio::test]
async fn anthropic_thinking_to_gemini_streaming() {
    let (mock_base, _mock) = spawn_anthropic_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!(
            "{proxy_base}/google/v1beta/models/test:streamGenerateContent"
        ))
        .json(&json!({ "contents": [{ "parts": [{ "text": "Hi" }] }] }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let text = res.text().await.unwrap();
    assert!(text.contains("\"thought\":true"), "body = {text}");
    assert!(text.contains("\"text\":\"think\""), "body = {text}");
    assert!(text.contains("\"text\":\"Hi\""), "body = {text}");
}

// ============================================================
// C. Thinking + Tool Use Combined
// ============================================================

#[tokio::test]
async fn anthropic_thinking_with_tools_to_openai_chat_non_streaming() {
    let (mock_base, _mock) = spawn_anthropic_thinking_with_tools_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Weather?" }],
            "tools": [{ "type": "function", "function": { "name": "get_weather", "parameters": { "type": "object", "properties": { "city": { "type": "string" } } } } }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "status: {status}",
        status = res.status()
    );
    let body: Value = res.json().await.unwrap();
    let msg = &body["choices"][0]["message"];
    assert_eq!(msg["reasoning_content"], "need to call tool");
    assert_eq!(msg["content"], "Calling tool.");
    let tool_calls = msg["tool_calls"].as_array().expect("tool_calls");
    assert_eq!(tool_calls[0]["function"]["name"], "get_weather");
    assert_eq!(body["choices"][0]["finish_reason"], "tool_calls");
}

#[tokio::test]
async fn anthropic_thinking_with_tools_to_responses_non_streaming() {
    let (mock_base, _mock) = spawn_anthropic_thinking_with_tools_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "claude-3",
            "input": "Weather?",
            "tools": [{ "type": "function", "name": "get_weather", "parameters": { "type": "object", "properties": { "city": { "type": "string" } } } }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "status: {status}",
        status = res.status()
    );
    let body: Value = res.json().await.unwrap();
    let output = body["output"].as_array().unwrap();
    assert!(
        output.iter().any(|o| o["type"] == "reasoning"),
        "missing reasoning: {output:?}"
    );
    assert!(
        output.iter().any(|o| o["type"] == "message"),
        "missing message: {output:?}"
    );
    assert!(
        output.iter().any(|o| o["type"] == "function_call"),
        "missing function_call: {output:?}"
    );
}

#[tokio::test]
async fn anthropic_thinking_with_tools_to_openai_streaming() {
    let (mock_base, _mock) = spawn_anthropic_thinking_with_tools_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Weather?" }],
            "tools": [{ "type": "function", "function": { "name": "get_weather", "parameters": {} } }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let text = res.text().await.unwrap();
    assert!(text.contains("reasoning_content"), "body = {text}");
    assert!(text.contains("tool_calls"), "body = {text}");
    assert!(text.contains("get_weather"), "body = {text}");
    assert!(
        !text.contains("\"finish_reason\":\"error\""),
        "body = {text}"
    );
}

// ============================================================
// D. Multi-Turn Thinking Preservation (Gap 3 fix)
// ============================================================

#[tokio::test]
async fn multi_turn_anthropic_thinking_preserved_in_history() {
    // Thinking blocks in assistant messages → reasoning_content on OpenAI upstream
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "mock",
            "max_tokens": 256,
            "messages": [
                { "role": "user", "content": "Think about 2+2" },
                { "role": "assistant", "content": [
                    { "type": "thinking", "thinking": "2+2 equals 4" },
                    { "type": "text", "text": "The answer is 4" }
                ]},
                { "role": "user", "content": "Now what about 3+3?" }
            ],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "status: {status}",
        status = res.status()
    );
    let body: Value = res.json().await.unwrap();
    assert!(body.get("content").is_some());
}

#[tokio::test]
async fn multi_turn_openai_reasoning_in_history_to_claude_preserves_unsigned_thinking() {
    let (mock_base, _mock) = spawn_anthropic_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "claude-3",
            "messages": [
                { "role": "user", "content": "Think about 2+2" },
                { "role": "assistant", "reasoning_content": "2+2 equals 4", "content": "The answer is 4" },
                { "role": "user", "content": "Now what about 3+3?" }
            ],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "status: {status}",
        status = res.status()
    );
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["choices"][0]["message"]["content"], "Hi");
}

#[tokio::test]
async fn multi_turn_openai_reasoning_to_claude_replays_as_unsigned_thinking_without_provenance() {
    let (mock_base, _mock, mut captured) = spawn_capture_anthropic_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "claude-3",
            "messages": [
                { "role": "user", "content": "Think about 2+2" },
                { "role": "assistant", "reasoning_content": "2+2 equals 4", "content": "The answer is 4" },
                { "role": "user", "content": "Now what about 3+3?" }
            ],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());

    captured.changed().await.unwrap();
    let request = captured
        .borrow()
        .clone()
        .expect("captured anthropic request");
    let messages = request["messages"].as_array().expect("anthropic messages");
    assert_eq!(messages[1]["role"], "assistant");
    let assistant_content = messages[1]["content"]
        .as_array()
        .expect("assistant content");
    assert_eq!(assistant_content[0]["type"], "thinking");
    assert_eq!(assistant_content[0]["thinking"], "2+2 equals 4");
    assert!(assistant_content[0].get("signature").is_none());
    assert_eq!(assistant_content[1]["type"], "text");
    assert_eq!(assistant_content[1]["text"], "The answer is 4");
    assert_eq!(messages[2]["role"], "user");
    assert_eq!(messages[2]["content"][0]["text"], "Now what about 3+3?");
}

#[tokio::test]
async fn multi_turn_gemini_thought_in_history_to_claude_preserves_unsigned_thinking() {
    let (mock_base, _mock, mut captured) = spawn_capture_anthropic_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!(
            "{proxy_base}/google/v1beta/models/test:generateContent"
        ))
        .json(&json!({
            "model": "claude-3",
            "contents": [
                { "role": "user", "parts": [{ "text": "Think about 2+2" }] },
                {
                    "role": "model",
                    "parts": [
                        { "thought": true, "text": "2+2 equals 4" },
                        { "text": "The answer is 4" }
                    ]
                },
                { "role": "user", "parts": [{ "text": "Now what about 3+3?" }] }
            ]
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());

    captured.changed().await.unwrap();
    let request = captured
        .borrow()
        .clone()
        .expect("captured anthropic request");
    let messages = request["messages"].as_array().expect("anthropic messages");
    assert_eq!(messages[1]["role"], "assistant");
    let assistant_content = messages[1]["content"]
        .as_array()
        .expect("assistant content");
    assert_eq!(assistant_content[0]["type"], "thinking");
    assert_eq!(assistant_content[0]["thinking"], "2+2 equals 4");
    assert!(assistant_content[0].get("signature").is_none());
    assert_eq!(assistant_content[1]["type"], "text");
    assert_eq!(assistant_content[1]["text"], "The answer is 4");
}

#[tokio::test]
async fn openai_reasoning_tool_turns_replay_to_gemini_with_dummy_signature() {
    let (mock_base, _mock, mut captured) = spawn_capture_google_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gemini-test",
            "messages": [
                { "role": "user", "content": "Check Tokyo weather" },
                {
                    "role": "assistant",
                    "reasoning_content": "Need to call weather tool first.",
                    "content": "Calling weather tool.",
                    "tool_calls": [{
                        "id": "call_weather",
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "arguments": "{\"city\":\"Tokyo\"}"
                        }
                    }]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_weather",
                    "content": "{\"temp_c\":22}"
                }
            ],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "get_weather",
                    "description": "Look up weather.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "city": { "type": "string" }
                        },
                        "required": ["city"]
                    }
                }
            }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let _body: Value = res.json().await.unwrap();

    captured.changed().await.unwrap();
    let request = captured.borrow().clone().expect("captured gemini request");
    let assistant_parts = request["contents"][1]["parts"]
        .as_array()
        .expect("assistant parts");
    assert!(assistant_parts
        .iter()
        .all(|part| part.get("thought").is_none()));
    assert_eq!(assistant_parts[0]["text"], "Calling weather tool.");
    assert_eq!(assistant_parts[1]["functionCall"]["name"], "get_weather");
    assert_eq!(
        assistant_parts[1]["thoughtSignature"],
        "skip_thought_signature_validator"
    );
    let tool_parts = request["contents"][2]["parts"]
        .as_array()
        .expect("tool parts");
    assert_eq!(tool_parts[0]["functionResponse"]["name"], "get_weather");
}

// ============================================================
// E. Usage/Token Count Tests
// ============================================================

#[tokio::test]
async fn anthropic_thinking_usage_translated_to_openai() {
    let (mock_base, _mock) = spawn_anthropic_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["usage"]["prompt_tokens"], 1);
    assert_eq!(body["usage"]["completion_tokens"], 2);
}

#[tokio::test]
async fn openai_reasoning_usage_to_anthropic_preserves_unsigned_thinking_without_provenance() {
    let (mock_base, _mock) = spawn_openai_completion_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "mock",
            "max_tokens": 256,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_reasoning_to_anthropic_preserved_as_unsigned_thinking(res, "think", "Hi", 3).await;
}

#[tokio::test]
async fn gemini_thinking_usage_translated_to_openai() {
    let (mock_base, _mock) = spawn_google_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gemini-test",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["usage"]["prompt_tokens"], 1);
    assert_eq!(body["usage"]["completion_tokens"], 2);
    assert_eq!(body["usage"]["total_tokens"], 3);
}

#[tokio::test]
async fn openai_reasoning_with_completion_tokens_details() {
    let (mock_base, _mock) = spawn_openai_completion_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({ "model": "mock", "input": "Hi", "stream": false }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body: Value = res.json().await.unwrap();
    assert_eq!(
        body["usage"]["output_tokens_details"]["reasoning_tokens"],
        1
    );
}

// ============================================================
// F. Edge Cases & Gap Fixes
// ============================================================

#[tokio::test]
async fn empty_thinking_block_no_crash() {
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "mock",
            "max_tokens": 256,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body: Value = res.json().await.unwrap();
    let content = body["content"].as_array().unwrap();
    assert!(content.iter().all(|c| c["type"] != "thinking"));
}

#[tokio::test]
async fn reasoning_and_text_both_present_in_response() {
    let (mock_base, _mock) = spawn_openai_completion_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({ "model": "mock", "input": "Hi", "stream": false }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["status"], "completed");
    let output = body["output"].as_array().unwrap();
    assert!(output.iter().any(|o| o["type"] == "reasoning"));
    assert!(output.iter().any(|o| o["type"] == "message"));
}

// Gap 2 fix: Gemini thought without thoughtSignature
#[tokio::test]
async fn gemini_thinking_no_signature_streaming_to_openai() {
    let (mock_base, _mock) = spawn_google_thinking_no_signature_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gemini-test",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let text = res.text().await.unwrap();
    assert!(
        text.contains("reasoning_content"),
        "should have reasoning_content without thoughtSignature: {text}"
    );
}

#[tokio::test]
async fn gemini_thinking_no_signature_non_streaming_to_openai() {
    let (mock_base, _mock) = spawn_google_thinking_no_signature_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gemini-test",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["choices"][0]["message"]["reasoning_content"], "think");
    assert_eq!(body["choices"][0]["message"]["content"], "Hi");
}
