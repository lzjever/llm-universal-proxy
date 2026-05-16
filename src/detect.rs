//! Detect client request format from path and body (reference: 9router detectFormat / detectFormatByEndpoint).

use serde_json::Value;

use crate::formats::UpstreamFormat;

/// Detects the client request format from the request path and JSON body.
/// Used to decide whether to translate request/response and in which direction.
pub fn detect_request_format(path: &str, body: &Value) -> UpstreamFormat {
    if let Some(by_path) = detect_by_path(path, body) {
        return by_path;
    }
    detect_by_body(body)
}

/// Path-based detection.
fn detect_by_path(path: &str, body: &Value) -> Option<UpstreamFormat> {
    if path.contains("/openai/v1/responses") {
        return Some(UpstreamFormat::OpenAiResponses);
    }
    if path.contains("/anthropic/v1/messages") {
        return Some(UpstreamFormat::Anthropic);
    }
    // /openai/v1/chat/completions with input[] can be OpenAI Responses body on chat endpoint
    if path.contains("/openai/v1/chat/completions")
        && body.get("input").and_then(Value::as_array).is_some()
    {
        return Some(UpstreamFormat::OpenAiResponses);
    }
    None
}

/// Body-based detection (reference: 9router open-sse/services/provider.js detectFormat).
fn detect_by_body(body: &Value) -> UpstreamFormat {
    // OpenAI Responses: input (array or string), no messages
    if body.get("input").is_some() && body.get("messages").is_none() {
        let input = body.get("input").unwrap();
        if input.is_array() || input.is_string() {
            return UpstreamFormat::OpenAiResponses;
        }
    }

    // OpenAI-specific fields (check before Claude)
    if body.get("stream_options").is_some()
        || body.get("response_format").is_some()
        || body.get("logprobs").is_some()
        || body.get("top_logprobs").is_some()
        || body.get("n").is_some()
        || body.get("presence_penalty").is_some()
        || body.get("frequency_penalty").is_some()
        || body.get("logit_bias").is_some()
        || body.get("user").is_some()
    {
        return UpstreamFormat::OpenAiCompletion;
    }

    // Claude/Anthropic: messages with content array, or system / anthropic_version
    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        if body.get("system").is_some() || body.get("anthropic_version").is_some() {
            return UpstreamFormat::Anthropic;
        }
        if let Some(first) = messages.first() {
            let content = first.get("content");
            if content.is_some() && content.and_then(Value::as_array).is_some() {
                let arr = content.and_then(Value::as_array).unwrap();
                let has_claude_image = arr.iter().any(|c| {
                    c.get("type").and_then(Value::as_str) == Some("image")
                        && c.get("source")
                            .and_then(|s| s.get("type").and_then(Value::as_str))
                            == Some("base64")
                });
                let has_claude_tool = arr.iter().any(|c| {
                    let t = c.get("type").and_then(Value::as_str);
                    t == Some("tool_use") || t == Some("tool_result")
                });
                if has_claude_image || has_claude_tool {
                    return UpstreamFormat::Anthropic;
                }
            }
        }
    }

    // Default: OpenAI Chat Completions
    UpstreamFormat::OpenAiCompletion
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn detect_openai_responses_by_path() {
        let path = "/openai/v1/responses";
        let body = json!({});
        assert_eq!(
            detect_request_format(path, &body),
            UpstreamFormat::OpenAiResponses
        );
    }

    #[test]
    fn detect_anthropic_by_messages_path() {
        let path = "/anthropic/v1/messages";
        let body = json!({ "messages": [{ "role": "user", "content": "Hi" }] });
        assert_eq!(
            detect_request_format(path, &body),
            UpstreamFormat::Anthropic
        );
    }

    #[test]
    fn detect_openai_responses_by_body_input_array() {
        let path = "/openai/v1/chat/completions";
        let body = json!({ "input": [], "model": "gpt-4o" });
        assert_eq!(
            detect_request_format(path, &body),
            UpstreamFormat::OpenAiResponses
        );
    }

    #[test]
    fn contents_body_no_longer_selects_removed_google_format() {
        let path = "/openai/v1/chat/completions";
        let body = json!({ "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }] });
        assert_eq!(
            detect_request_format(path, &body),
            UpstreamFormat::OpenAiCompletion
        );
    }

    #[test]
    fn detect_openai_by_specific_fields() {
        let path = "/openai/v1/chat/completions";
        let body = json!({ "messages": [{ "role": "user", "content": "Hi" }], "response_format": { "type": "json_object" } });
        assert_eq!(
            detect_request_format(path, &body),
            UpstreamFormat::OpenAiCompletion
        );
    }

    #[test]
    fn detect_anthropic_by_system() {
        let path = "/openai/v1/chat/completions";
        let body = json!({ "messages": [{ "role": "user", "content": "Hi" }], "system": "You are helpful." });
        assert_eq!(
            detect_request_format(path, &body),
            UpstreamFormat::Anthropic
        );
    }

    #[test]
    fn default_openai_completion() {
        let path = "/openai/v1/chat/completions";
        let body = json!({ "messages": [{ "role": "user", "content": "Hi" }], "model": "gpt-4o" });
        assert_eq!(
            detect_request_format(path, &body),
            UpstreamFormat::OpenAiCompletion
        );
    }

    #[test]
    fn detect_anthropic_by_anthropic_version() {
        let path = "/openai/v1/chat/completions";
        let body = json!({ "messages": [{ "role": "user", "content": "Hi" }], "anthropic_version": "2023-01-01" });
        assert_eq!(
            detect_request_format(path, &body),
            UpstreamFormat::Anthropic
        );
    }

    #[test]
    fn detect_anthropic_by_content_array_with_image() {
        let path = "/openai/v1/chat/completions";
        let body = json!({
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "abc" } }
                ]
            }]
        });
        assert_eq!(
            detect_request_format(path, &body),
            UpstreamFormat::Anthropic
        );
    }

    #[test]
    fn detect_anthropic_by_content_array_with_tool_use() {
        let path = "/openai/v1/chat/completions";
        let body = json!({
            "messages": [{ "role": "user", "content": [{ "type": "tool_use", "id": "x", "name": "f", "input": {} }] }]
        });
        assert_eq!(
            detect_request_format(path, &body),
            UpstreamFormat::Anthropic
        );
    }

    #[test]
    fn detect_responses_by_input_string() {
        let path = "/openai/v1/chat/completions";
        let body = json!({ "input": "Hello", "model": "gpt-4o" });
        assert_eq!(
            detect_request_format(path, &body),
            UpstreamFormat::OpenAiResponses
        );
    }

    #[test]
    fn path_priority_over_body() {
        let path = "/openai/v1/responses";
        let body = json!({ "messages": [{ "role": "user", "content": "Hi" }] });
        assert_eq!(
            detect_request_format(path, &body),
            UpstreamFormat::OpenAiResponses
        );
    }
}
