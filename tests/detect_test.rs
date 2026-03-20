//! Integration tests for format detection (TDD).

use llm_universal_proxy::detect::detect_request_format;
use llm_universal_proxy::formats::UpstreamFormat;
use serde_json::json;

#[test]
fn path_openai_responses_is_openai_responses() {
    let body = json!({});
    assert_eq!(
        detect_request_format("/openai/v1/responses", &body),
        UpstreamFormat::OpenAiResponses
    );
}

#[test]
fn path_chat_completions_with_input_array_is_openai_responses() {
    let body = json!({ "input": [], "model": "gpt-4o" });
    assert_eq!(
        detect_request_format("/openai/v1/chat/completions", &body),
        UpstreamFormat::OpenAiResponses
    );
}

#[test]
fn body_contents_is_google() {
    let body = json!({
        "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }]
    });
    assert_eq!(
        detect_request_format("/google/v1beta/models/gemini-local:generateContent", &body),
        UpstreamFormat::Google
    );
}

#[test]
fn body_system_with_messages_is_anthropic() {
    let body = json!({
        "messages": [{ "role": "user", "content": "Hi" }],
        "system": "You are helpful."
    });
    assert_eq!(
        detect_request_format("/openai/v1/chat/completions", &body),
        UpstreamFormat::Anthropic
    );
}

#[test]
fn body_response_format_is_openai_completion() {
    let body = json!({
        "messages": [{ "role": "user", "content": "Hi" }],
        "response_format": { "type": "json_object" }
    });
    assert_eq!(
        detect_request_format("/openai/v1/chat/completions", &body),
        UpstreamFormat::OpenAiCompletion
    );
}

#[test]
fn default_messages_only_is_openai_completion() {
    let body = json!({
        "messages": [{ "role": "user", "content": "Hi" }],
        "model": "gpt-4o"
    });
    assert_eq!(
        detect_request_format("/openai/v1/chat/completions", &body),
        UpstreamFormat::OpenAiCompletion
    );
}
