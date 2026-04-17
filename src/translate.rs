//! Request/response translation between formats (pivot: OpenAI Chat Completions).
//!
//! Reference: 9router open-sse/translator/index.js — source → openai → target.

use serde_json::Value;

use crate::formats::UpstreamFormat;

/// Translate response body from upstream format to client format.
/// Converts via OpenAI pivot: upstream → openai → client when formats differ.
pub fn translate_response(
    upstream_format: UpstreamFormat,
    client_format: UpstreamFormat,
    body: &Value,
) -> Result<Value, String> {
    if upstream_format == client_format {
        return Ok(body.clone());
    }
    let openai = upstream_response_to_openai(upstream_format, body)?;
    if client_format == UpstreamFormat::OpenAiCompletion {
        return Ok(openai);
    }
    openai_response_to_client(client_format, &openai)
}

/// Convert upstream non-streaming response to OpenAI completion shape.
fn upstream_response_to_openai(
    upstream_format: UpstreamFormat,
    body: &Value,
) -> Result<Value, String> {
    match upstream_format {
        UpstreamFormat::OpenAiCompletion => Ok(normalize_openai_completion_response(body)),
        UpstreamFormat::Anthropic => claude_response_to_openai(body),
        UpstreamFormat::Google => gemini_response_to_openai(body),
        UpstreamFormat::OpenAiResponses => responses_response_to_openai(body),
    }
}

/// Convert OpenAI completion response to client format (Responses, Claude, Gemini).
fn openai_response_to_client(client_format: UpstreamFormat, body: &Value) -> Result<Value, String> {
    match client_format {
        UpstreamFormat::OpenAiCompletion => Ok(body.clone()),
        UpstreamFormat::OpenAiResponses => openai_response_to_responses(body),
        UpstreamFormat::Anthropic => openai_response_to_claude(body),
        UpstreamFormat::Google => openai_response_to_gemini(body),
    }
}

pub(crate) fn anthropic_tool_use_type_for_openai_tool_call(tool_call: &Value) -> &'static str {
    match tool_call.get("proxied_tool_kind").and_then(Value::as_str) {
        Some("anthropic_server_tool_use") => "server_tool_use",
        _ => "tool_use",
    }
}

fn claude_response_to_openai(body: &Value) -> Result<Value, String> {
    let content = body
        .get("content")
        .and_then(Value::as_array)
        .ok_or("missing content")?;
    let mut text_content = String::new();
    let mut reasoning_content = String::new();
    let mut tool_calls: Vec<Value> = vec![];
    for block in content {
        let ty = block.get("type").and_then(Value::as_str);
        if ty == Some("text") {
            text_content.push_str(block.get("text").and_then(Value::as_str).unwrap_or(""));
        } else if ty == Some("thinking") {
            reasoning_content.push_str(block.get("thinking").and_then(Value::as_str).unwrap_or(""));
        } else if ty == Some("tool_use") {
            tool_calls.push(serde_json::json!({
                "id": block.get("id"),
                "type": "function",
                "function": {
                    "name": block.get("name"),
                    "arguments": block.get("input").map(|i| serde_json::to_string(i).unwrap_or_else(|_| "{}".into())).unwrap_or_else(|| "{}".to_string())
                }
            }));
        }
    }
    let mut message = serde_json::json!({ "role": "assistant" });
    if !text_content.is_empty() {
        message["content"] = Value::String(text_content);
    }
    if !reasoning_content.is_empty() {
        message["reasoning_content"] = Value::String(reasoning_content);
    }
    if !tool_calls.is_empty() {
        message["tool_calls"] = Value::Array(tool_calls);
    }
    if message.get("content").is_none() && message.get("tool_calls").is_none() {
        message["content"] = Value::String(String::new());
    }
    let mut finish_reason = body
        .get("stop_reason")
        .and_then(Value::as_str)
        .unwrap_or("stop")
        .to_string();
    if finish_reason == "end_turn" {
        finish_reason = "stop".to_string();
    }
    if finish_reason == "tool_use" {
        finish_reason = "tool_calls".to_string();
    }
    if finish_reason == "model_context_window_exceeded" {
        finish_reason = "context_length_exceeded".to_string();
    }
    if finish_reason == "pause_turn" {
        finish_reason = "pause_turn".to_string();
    }
    if finish_reason == "refusal" {
        finish_reason = "content_filter".to_string();
    }
    let mut result = serde_json::json!({
        "id": body.get("id").cloned().unwrap_or_else(|| serde_json::json!(format!("chatcmpl-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()))),
        "object": "chat.completion",
        "created": body.get("created").cloned().unwrap_or_else(|| serde_json::json!(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs())),
        "model": body.get("model").cloned().unwrap_or(serde_json::json!("claude")),
        "choices": [{ "index": 0, "message": message, "finish_reason": finish_reason }]
    });
    // Usage with cache token reporting
    // Reference: 9router claude-to-openai.js - include cache_read_input_tokens, cache_creation_input_tokens
    if let Some(usage) = body.get("usage") {
        let input_tokens = usage
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let output_tokens = usage
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let cache_read = usage
            .get("cache_read_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let cache_creation = usage
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);

        // prompt_tokens = input_tokens + cache_read + cache_creation (matches 9router)
        let prompt_tokens = input_tokens + cache_read + cache_creation;

        let mut usage_json = serde_json::json!({
            "prompt_tokens": prompt_tokens,
            "completion_tokens": output_tokens,
            "total_tokens": prompt_tokens + output_tokens
        });

        // Add cache details if present
        if cache_read > 0 {
            usage_json["cache_read_input_tokens"] = Value::Number(cache_read.into());
        }
        if cache_creation > 0 {
            usage_json["cache_creation_input_tokens"] = Value::Number(cache_creation.into());
        }

        result["usage"] = usage_json;
    }
    Ok(result)
}

pub(crate) fn classify_portable_non_success_terminal(code_or_reason: Option<&str>) -> &'static str {
    let Some(code_or_reason) = code_or_reason else {
        return "error";
    };

    let lower = code_or_reason.to_ascii_lowercase();
    let upper = code_or_reason.to_ascii_uppercase();

    if lower == "context_length_exceeded" {
        return "context_length_exceeded";
    }

    if matches!(
        upper.as_str(),
        "SAFETY"
            | "RECITATION"
            | "BLOCKLIST"
            | "PROHIBITED_CONTENT"
            | "SPII"
            | "IMAGE_SAFETY"
            | "IMAGE_PROHIBITED_CONTENT"
            | "IMAGE_RECITATION"
    ) || lower == "content_filter"
        || lower.contains("safety")
        || lower.contains("policy")
        || lower.contains("block")
        || lower.contains("prohibited")
        || lower.contains("recitation")
        || lower.contains("spii")
    {
        return "content_filter";
    }

    if matches!(
        upper.as_str(),
        "MALFORMED_FUNCTION_CALL"
            | "UNEXPECTED_TOOL_CALL"
            | "TOO_MANY_TOOL_CALLS"
            | "MISSING_THOUGHT_SIGNATURE"
    ) || lower.contains("tool")
        || lower.contains("function")
        || lower.contains("signature")
        || lower.contains("schema")
        || lower.contains("validation")
    {
        return "tool_error";
    }

    "error"
}

pub(crate) fn gemini_finish_reason_to_openai(
    finish_reason: Option<&str>,
    has_tool_calls: bool,
) -> String {
    match finish_reason.unwrap_or("STOP") {
        "STOP" => {
            if has_tool_calls {
                "tool_calls"
            } else {
                "stop"
            }
        }
        "MAX_TOKENS" => "length",
        other => classify_portable_non_success_terminal(Some(other)),
    }
    .to_string()
}

fn openai_finish_reason_to_gemini(finish_reason: &str) -> &'static str {
    match finish_reason {
        "stop" | "tool_calls" => "STOP",
        "length" => "MAX_TOKENS",
        "content_filter" => "SAFETY",
        "pause_turn" | "context_length_exceeded" | "tool_error" | "error" => "OTHER",
        _ => "STOP",
    }
}

const GEMINI_DUMMY_THOUGHT_SIGNATURE: &str = "skip_thought_signature_validator";

pub(crate) fn responses_failed_code_to_openai_finish(code: Option<&str>) -> &'static str {
    classify_portable_non_success_terminal(code)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AnthropicTerminal {
    StopReason(&'static str),
    Error {
        error_type: &'static str,
        message: &'static str,
    },
}

pub(crate) fn classify_openai_finish_for_anthropic(reason: &str) -> AnthropicTerminal {
    match reason {
        "stop" => AnthropicTerminal::StopReason("end_turn"),
        "length" => AnthropicTerminal::StopReason("max_tokens"),
        "tool_calls" => AnthropicTerminal::StopReason("tool_use"),
        "pause_turn" => AnthropicTerminal::StopReason("pause_turn"),
        "content_filter" => AnthropicTerminal::StopReason("refusal"),
        "context_length_exceeded" => AnthropicTerminal::StopReason("model_context_window_exceeded"),
        "error" => AnthropicTerminal::Error {
            error_type: "api_error",
            message: "The provider returned an error.",
        },
        "tool_error" => AnthropicTerminal::Error {
            error_type: "invalid_request_error",
            message: "The provider reported a tool or protocol error.",
        },
        _ => AnthropicTerminal::StopReason("end_turn"),
    }
}

fn responses_finish_reason_to_openai(body: &Value, has_tool_calls: bool) -> String {
    match body.get("status").and_then(Value::as_str) {
        Some("incomplete") => match body
            .get("incomplete_details")
            .and_then(|details| details.get("reason"))
            .and_then(Value::as_str)
        {
            Some("max_output_tokens") => "length".to_string(),
            Some("content_filter") => "content_filter".to_string(),
            Some("pause_turn") => "pause_turn".to_string(),
            _ => "stop".to_string(),
        },
        Some("failed") => responses_failed_code_to_openai_finish(
            body.get("error")
                .and_then(|error| error.get("code"))
                .and_then(Value::as_str),
        )
        .to_string(),
        _ => {
            if has_tool_calls {
                "tool_calls".to_string()
            } else {
                "stop".to_string()
            }
        }
    }
}

fn push_gemini_function_call_part(parts: &mut Vec<Value>, tool_call: &Value, first_in_step: bool) {
    let args = tool_call
        .get("function")
        .and_then(|f| f.get("arguments"))
        .and_then(Value::as_str);
    let args_val = args
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(serde_json::json!({}));
    let mut part = serde_json::json!({
        "functionCall": {
            "id": tool_call.get("id"),
            "name": tool_call.get("function").and_then(|f| f.get("name")),
            "args": args_val
        }
    });
    if first_in_step {
        part["thoughtSignature"] = Value::String(GEMINI_DUMMY_THOUGHT_SIGNATURE.to_string());
    }
    parts.push(part);
}

fn gemini_response_to_openai(body: &Value) -> Result<Value, String> {
    let response = body.get("response").unwrap_or(body);
    let candidates = response.get("candidates").and_then(Value::as_array);
    let candidate = candidates.and_then(|items| items.first());
    let content = candidate.and_then(|candidate| candidate.get("content"));
    let parts: Vec<&Value> = content
        .and_then(|c| c.get("parts"))
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let mut text_content = String::new();
    let mut reasoning_content = String::new();
    let mut tool_calls: Vec<Value> = vec![];
    for part in parts {
        if part.get("thought").and_then(Value::as_bool) == Some(true) {
            if let Some(t) = part.get("text").and_then(Value::as_str) {
                reasoning_content.push_str(t);
            }
        } else if let Some(t) = part.get("text").and_then(Value::as_str) {
            text_content.push_str(t);
        }
        if let Some(fc) = part.get("functionCall") {
            tool_calls.push(serde_json::json!({
                "id": fc.get("id").cloned().unwrap_or_else(|| serde_json::json!(format!("call_{}_{}", fc.get("name").and_then(Value::as_str).unwrap_or(""), tool_calls.len()))),
                "type": "function",
                "function": {
                    "name": fc.get("name"),
                    "arguments": fc.get("args").map(|a| serde_json::to_string(a).unwrap_or_else(|_| "{}".into())).unwrap_or_else(|| "{}".to_string())
                }
            }));
        }
    }
    let mut message = serde_json::json!({ "role": "assistant" });
    if !text_content.is_empty() {
        message["content"] = Value::String(text_content);
    }
    if !reasoning_content.is_empty() {
        message["reasoning_content"] = Value::String(reasoning_content);
    }
    let has_tool_calls = !tool_calls.is_empty();
    if has_tool_calls {
        message["tool_calls"] = Value::Array(tool_calls);
    }
    if message.get("content").is_none() && message.get("tool_calls").is_none() {
        message["content"] = Value::String(String::new());
    }
    let finish_reason = if let Some(candidate) = candidate {
        gemini_finish_reason_to_openai(
            candidate.get("finishReason").and_then(Value::as_str),
            has_tool_calls,
        )
    } else {
        let prompt_feedback_reason = response
            .get("promptFeedback")
            .or(body.get("promptFeedback"))
            .and_then(|feedback| feedback.get("blockReason"))
            .and_then(Value::as_str);
        classify_portable_non_success_terminal(prompt_feedback_reason).to_string()
    };
    let usage = response.get("usageMetadata").or(body.get("usageMetadata"));
    let mut result = serde_json::json!({
        "id": response.get("responseId").cloned().unwrap_or_else(|| serde_json::json!(format!("chatcmpl-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()))),
        "object": "chat.completion",
        "created": response.get("createTime").and_then(Value::as_u64).unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()),
        "model": response.get("modelVersion").cloned().unwrap_or(serde_json::json!("gemini")),
        "choices": [{ "index": 0, "message": message, "finish_reason": finish_reason }]
    });
    // Usage with cache token reporting
    // Reference: 9router gemini-to-openai.js - include cachedContentTokenCount as prompt_tokens_details.cached_tokens
    if let Some(u) = usage {
        let prompt_tokens = u
            .get("promptTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let candidates_tokens = u
            .get("candidatesTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let thoughts_tokens = u
            .get("thoughtsTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let total_tokens = u
            .get("totalTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let cached_tokens = u
            .get("cachedContentTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);

        // completion_tokens = candidatesTokenCount + thoughtsTokenCount (matches 9router)
        let completion_tokens = candidates_tokens + thoughts_tokens;

        let mut usage_json = serde_json::json!({
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens": total_tokens
        });

        // Add prompt_tokens_details if cached tokens exist
        if cached_tokens > 0 {
            usage_json["prompt_tokens_details"] = serde_json::json!({
                "cached_tokens": cached_tokens
            });
        }
        if thoughts_tokens > 0 {
            usage_json["completion_tokens_details"] = serde_json::json!({
                "reasoning_tokens": thoughts_tokens
            });
        }

        result["usage"] = usage_json;
    }
    Ok(result)
}

fn is_minimax_model(model: &str) -> bool {
    model.starts_with("MiniMax-")
}

fn reasoning_details_to_text(value: Option<&Value>) -> Option<String> {
    let value = value?;
    match value {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Array(items) => {
            let joined = items
                .iter()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("");
            (!joined.is_empty()).then_some(joined)
        }
        _ => None,
    }
}

fn openai_message_reasoning_text(message: &Value) -> Option<String> {
    if let Some(text) = message.get("reasoning_content").and_then(Value::as_str) {
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }
    reasoning_details_to_text(message.get("reasoning_details"))
}

fn normalize_openai_completion_response(body: &Value) -> Value {
    let mut out = body.clone();
    let Some(choices) = out.get_mut("choices").and_then(Value::as_array_mut) else {
        return out;
    };
    for choice in choices.iter_mut() {
        let Some(message) = choice.get_mut("message").and_then(Value::as_object_mut) else {
            continue;
        };
        let reasoning_details = message.get("reasoning_details").cloned();
        let has_reasoning_content = message
            .get("reasoning_content")
            .and_then(Value::as_str)
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        if !has_reasoning_content {
            if let Some(reasoning) = reasoning_details_to_text(reasoning_details.as_ref()) {
                message.insert("reasoning_content".to_string(), Value::String(reasoning));
            }
        }
    }
    out
}

fn openai_response_to_claude(body: &Value) -> Result<Value, String> {
    let choices = body
        .get("choices")
        .and_then(Value::as_array)
        .ok_or("missing choices")?;
    let choice = choices.first().ok_or("empty choices")?;
    let message = choice.get("message").ok_or("missing message")?;
    let mut content: Vec<Value> = vec![];
    if let Some(rc) = openai_message_reasoning_text(message) {
        if !rc.is_empty() {
            content.push(serde_json::json!({ "type": "text", "text": rc }));
        }
    }
    if let Some(t) = message.get("content").and_then(Value::as_str) {
        if !t.is_empty() {
            content.push(serde_json::json!({ "type": "text", "text": t }));
        }
    }
    if content.is_empty() && message.get("tool_calls").is_none() {
        content.push(serde_json::json!({ "type": "text", "text": "" }));
    }
    if let Some(tc) = message.get("tool_calls").and_then(Value::as_array) {
        for t in tc {
            let args = t
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(Value::as_str);
            let input = args
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or(serde_json::json!({}));
            content.push(serde_json::json!({
                "type": anthropic_tool_use_type_for_openai_tool_call(t),
                "id": t.get("id"),
                "name": t.get("function").and_then(|f| f.get("name")),
                "input": input
            }));
        }
    }
    let finish = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .unwrap_or("stop");
    let terminal = classify_openai_finish_for_anthropic(finish);
    if let AnthropicTerminal::Error {
        error_type,
        message,
    } = terminal
    {
        return Ok(serde_json::json!({
            "type": "error",
            "error": {
                "type": error_type,
                "message": message
            }
        }));
    }
    let AnthropicTerminal::StopReason(stop_reason) = terminal else {
        unreachable!("anthropic terminal must be stop reason after error early return");
    };
    let mut result = serde_json::json!({
        "id": body.get("id").cloned().unwrap_or(serde_json::Value::Null),
        "type": "message",
        "role": "assistant",
        "content": content,
        "model": body.get("model").cloned().unwrap_or(serde_json::Value::Null),
        "stop_reason": stop_reason
    });
    if let Some(u) = body.get("usage") {
        result["usage"] = serde_json::json!({
            "input_tokens": u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0),
            "output_tokens": u.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0)
        });
    }
    Ok(result)
}

fn openai_response_to_gemini(body: &Value) -> Result<Value, String> {
    let choices = body
        .get("choices")
        .and_then(Value::as_array)
        .ok_or("missing choices")?;
    let choice = choices.first().ok_or("empty choices")?;
    let message = choice.get("message").ok_or("missing message")?;
    let mut parts: Vec<Value> = vec![];
    if let Some(rc) = openai_message_reasoning_text(message) {
        if !rc.is_empty() {
            parts.push(serde_json::json!({ "thought": true, "text": rc }));
        }
    }
    if let Some(t) = message.get("content").and_then(Value::as_str) {
        if !t.is_empty() {
            parts.push(serde_json::json!({ "text": t }));
        }
    }
    if let Some(tc) = message.get("tool_calls").and_then(Value::as_array) {
        for (idx, t) in tc.iter().enumerate() {
            push_gemini_function_call_part(&mut parts, t, idx == 0);
        }
    }
    if parts.is_empty() {
        parts.push(serde_json::json!({ "text": "" }));
    }
    let finish = openai_finish_reason_to_gemini(
        choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .unwrap_or("stop"),
    );
    let mut result = serde_json::json!({
        "candidates": [{
            "content": { "role": "model", "parts": parts },
            "finishReason": finish
        }],
        "usageMetadata": openai_usage_to_gemini_usage(body.get("usage")),
        "modelVersion": body.get("model").cloned().unwrap_or(serde_json::Value::Null)
    });
    if let Some(id) = body.get("id") {
        result["responseId"] = id.clone();
    }
    Ok(result)
}

fn responses_response_to_openai(body: &Value) -> Result<Value, String> {
    let output = match body.get("output").and_then(Value::as_array) {
        Some(o) => o,
        None => return Ok(body.clone()),
    };
    let mut content = String::new();
    let mut reasoning_content = String::new();
    let mut tool_calls: Vec<Value> = vec![];
    for item in output {
        let ty = item.get("type").and_then(Value::as_str);
        if ty == Some("message") {
            if let Some(arr) = item.get("content").and_then(Value::as_array) {
                for part in arr {
                    if part.get("type").and_then(Value::as_str) == Some("output_text") {
                        content.push_str(part.get("text").and_then(Value::as_str).unwrap_or(""));
                    }
                }
            }
        }
        if ty == Some("function_call") {
            tool_calls.push(serde_json::json!({
                "id": item.get("call_id"),
                "type": "function",
                "function": {
                    "name": item.get("name"),
                    "arguments": item.get("arguments").and_then(Value::as_str).unwrap_or("{}")
                }
            }));
        }
        if ty == Some("reasoning") {
            if let Some(summary) = item.get("summary").and_then(Value::as_array) {
                for s in summary {
                    if let Some(t) = s.get("text").and_then(Value::as_str) {
                        reasoning_content.push_str(t);
                    }
                }
            }
        }
    }
    let mut message = serde_json::json!({ "role": "assistant" });
    if !reasoning_content.is_empty() {
        message["reasoning_content"] = Value::String(reasoning_content);
    }
    let has_tool_calls = !tool_calls.is_empty();
    if !content.is_empty() {
        message["content"] = Value::String(content);
    } else if has_tool_calls || message.get("reasoning_content").is_some() {
        message["content"] = Value::Null;
    } else {
        message["content"] = Value::String(String::new());
    }
    if has_tool_calls {
        message["tool_calls"] = Value::Array(tool_calls);
    }
    let finish = responses_finish_reason_to_openai(body, has_tool_calls);
    let mut result = serde_json::json!({
        "id": body.get("id").cloned().unwrap_or(serde_json::Value::Null),
        "object": "chat.completion",
        "created": body
            .get("created_at")
            .or(body.get("created"))
            .cloned()
            .unwrap_or_else(|| serde_json::json!(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs())),
        "model": body.get("model").cloned().unwrap_or(serde_json::Value::Null),
        "choices": [{ "index": 0, "message": message, "finish_reason": finish }]
    });
    if let Some(u) = body.get("usage") {
        result["usage"] = responses_usage_to_openai_usage(u);
    }
    Ok(result)
}

fn openai_response_to_responses(body: &Value) -> Result<Value, String> {
    let choices = body
        .get("choices")
        .and_then(Value::as_array)
        .ok_or("missing choices")?;
    let choice = choices.first().ok_or("empty choices")?;
    let message = choice.get("message").ok_or("missing message")?;
    let mut output: Vec<Value> = vec![];
    if let Some(reasoning) = openai_message_reasoning_text(message) {
        if !reasoning.is_empty() {
            output.push(serde_json::json!({
                "type": "reasoning",
                "summary": [{ "type": "summary_text", "text": reasoning }]
            }));
        }
    }
    let content = openai_message_content_to_responses_output(message.get("content"));
    if !content.is_empty() {
        output.push(serde_json::json!({
            "type": "message",
            "role": "assistant",
            "content": content
        }));
    } else if message.get("tool_calls").is_none() && output.is_empty() {
        output.push(serde_json::json!({
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": "" }]
        }));
    }
    if let Some(tc) = message.get("tool_calls").and_then(Value::as_array) {
        for t in tc {
            output.push(serde_json::json!({
                "type": "function_call",
                "call_id": t.get("id"),
                "name": t.get("function").and_then(|f| f.get("name")),
                "arguments": t.get("function").and_then(|f| f.get("arguments")).and_then(Value::as_str).unwrap_or("{}")
            }));
        }
    }
    let created_at = body.get("created").cloned().unwrap_or_else(|| {
        serde_json::json!(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs())
    });
    let finish_reason = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .unwrap_or("stop");
    let (status, incomplete_details, error) = match finish_reason {
        "length" => (
            "incomplete",
            serde_json::json!({ "reason": "max_output_tokens" }),
            Value::Null,
        ),
        "content_filter" => (
            "incomplete",
            serde_json::json!({ "reason": "content_filter" }),
            Value::Null,
        ),
        "pause_turn" => (
            "incomplete",
            serde_json::json!({ "reason": "pause_turn" }),
            Value::Null,
        ),
        "context_length_exceeded" => (
            "failed",
            Value::Null,
            serde_json::json!({
                "code": "context_length_exceeded",
                "message": "The conversation exceeded the model context window.",
                "type": "invalid_request_error"
            }),
        ),
        "error" => (
            "failed",
            Value::Null,
            serde_json::json!({
                "code": "error",
                "message": "The provider returned an error.",
                "type": "server_error"
            }),
        ),
        "tool_error" => (
            "failed",
            Value::Null,
            serde_json::json!({
                "code": "tool_error",
                "message": "The provider reported a tool or protocol error.",
                "type": "invalid_request_error"
            }),
        ),
        _ => ("completed", Value::Null, Value::Null),
    };
    let mut result = serde_json::json!({
        "id": body.get("id").cloned().unwrap_or(serde_json::Value::Null),
        "object": "response",
        "created_at": created_at,
        "output": output,
        "status": status,
        "incomplete_details": incomplete_details,
        "error": error
    });
    if let Some(u) = body.get("usage") {
        result["usage"] = openai_usage_to_responses_usage(u);
    }
    Ok(result)
}

/// Translate request body from client format to upstream format.
/// If client_format == upstream_format, returns body as-is (passthrough).
pub fn translate_request(
    client_format: UpstreamFormat,
    upstream_format: UpstreamFormat,
    model: &str,
    body: &mut Value,
    stream: bool,
) -> Result<(), String> {
    if client_format == upstream_format {
        if stream && (client_format != UpstreamFormat::OpenAiCompletion || !is_minimax_model(model))
        {
            normalize_openai_roles_for_compatibility(client_format, body);
        }
        if client_format == UpstreamFormat::OpenAiCompletion {
            apply_openai_completion_compat_overrides(model, body);
        }
        return Ok(());
    }
    let translated_from_openai_completion = client_format == UpstreamFormat::OpenAiCompletion;
    // Step 1: client → openai (if client is not openai)
    if client_format != UpstreamFormat::OpenAiCompletion {
        client_to_openai_completion(client_format, body)?;
    }
    // Step 2: openai → upstream (if upstream is not openai)
    if upstream_format != UpstreamFormat::OpenAiCompletion {
        openai_completion_to_upstream(upstream_format, body)?;
        if stream {
            if upstream_format == UpstreamFormat::OpenAiResponses
                && translated_from_openai_completion
            {
                hoist_and_merge_system_messages(body);
            } else {
                normalize_openai_roles_for_compatibility(upstream_format, body);
            }
        }
    } else {
        if stream
            && (upstream_format != UpstreamFormat::OpenAiCompletion || !is_minimax_model(model))
        {
            normalize_openai_roles_for_compatibility(upstream_format, body);
        }
        apply_openai_completion_compat_overrides(model, body);
    }
    Ok(())
}

fn apply_openai_completion_compat_overrides(model: &str, body: &mut Value) {
    if !is_minimax_model(model) {
        return;
    }

    if let Some(obj) = body.as_object_mut() {
        obj.insert("reasoning_split".to_string(), serde_json::json!(true));
        let stream_options = obj
            .entry("stream_options".to_string())
            .or_insert_with(|| serde_json::json!({}));
        if let Some(stream_options_obj) = stream_options.as_object_mut() {
            stream_options_obj.insert("include_usage".to_string(), serde_json::json!(true));
        }
    }

    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    for message in messages.iter_mut() {
        let is_assistant = message.get("role").and_then(Value::as_str) == Some("assistant");
        if !is_assistant || message.get("reasoning_details").is_some() {
            continue;
        }
        let Some(reasoning) = message.get("reasoning_content").and_then(Value::as_str) else {
            continue;
        };
        if reasoning.is_empty() {
            continue;
        }
        message["reasoning_details"] = serde_json::json!([{ "text": reasoning }]);
    }
}

fn normalize_openai_roles_for_compatibility(format: UpstreamFormat, body: &mut Value) {
    match format {
        UpstreamFormat::OpenAiCompletion => normalize_openai_messages_for_compatibility(body),
        UpstreamFormat::OpenAiResponses => normalize_openai_responses_roles(body),
        _ => {}
    }
}

fn normalize_openai_messages_for_compatibility(body: &mut Value) {
    normalize_openai_message_roles(body);
    coalesce_openai_string_messages(body);
}

fn normalize_openai_message_roles(body: &mut Value) {
    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    for message in messages.iter_mut() {
        if message.get("role").and_then(Value::as_str) == Some("developer") {
            message["role"] = Value::String("system".to_string());
        }
    }
}

fn coalesce_openai_string_messages(body: &mut Value) {
    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };

    let mut coalesced: Vec<Value> = Vec::new();
    for message in std::mem::take(messages) {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .map(str::to_string);
        let content = message
            .get("content")
            .and_then(Value::as_str)
            .map(str::to_string);
        let Some(role) = role else {
            coalesced.push(message);
            continue;
        };
        let Some(content) = content else {
            coalesced.push(message);
            continue;
        };

        if let Some(previous) = coalesced.last_mut() {
            let previous_role = previous.get("role").and_then(Value::as_str);
            let previous_content = previous.get("content").and_then(Value::as_str);
            if previous_role == Some(role.as_str()) {
                if let Some(previous_content) = previous_content {
                    previous["content"] = Value::String(format!("{previous_content}\n\n{content}"));
                    continue;
                }
            }
        }

        coalesced.push(message);
    }

    *messages = coalesced;
}

fn hoist_and_merge_system_messages(body: &mut Value) {
    let Some(input) = body.get_mut("input").and_then(Value::as_array_mut) else {
        return;
    };

    let mut hoisted_segments = Vec::new();
    let mut remainder = Vec::new();
    let mut hoisting = true;

    for item in std::mem::take(input) {
        let role = item.get("role").and_then(Value::as_str);
        let is_message = item.get("type").and_then(Value::as_str) == Some("message");
        if hoisting && is_message && matches!(role, Some("system") | Some("developer")) {
            let text = extract_responses_text_content(item.get("content"));
            if !text.is_empty() {
                hoisted_segments.push(text);
            }
        } else {
            hoisting = false;
            remainder.push(item);
        }
    }

    *input = remainder;
    if hoisted_segments.is_empty() {
        return;
    }

    let mut merged = hoisted_segments.join("\n\n");
    if let Some(existing) = body.get("instructions").and_then(Value::as_str) {
        if !existing.is_empty() {
            merged = format!("{existing}\n\n{merged}");
        }
    }
    body["instructions"] = Value::String(merged);
}

fn normalize_openai_responses_roles(body: &mut Value) {
    let Some(input) = body.get_mut("input").and_then(Value::as_array_mut) else {
        return;
    };
    for item in input.iter_mut() {
        if item.get("type").and_then(Value::as_str) == Some("message")
            && item.get("role").and_then(Value::as_str) == Some("developer")
        {
            item["role"] = Value::String("system".to_string());
        }
    }
}

fn extract_responses_text_content(content: Option<&Value>) -> String {
    let Some(content) = content else {
        return String::new();
    };
    let Some(parts) = content.as_array() else {
        return String::new();
    };

    parts
        .iter()
        .filter_map(|part| match part.get("type").and_then(Value::as_str) {
            Some("input_text") | Some("output_text") => part.get("text").and_then(Value::as_str),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

fn client_to_openai_completion(from: UpstreamFormat, body: &mut Value) -> Result<(), String> {
    match from {
        UpstreamFormat::OpenAiCompletion => {}
        UpstreamFormat::OpenAiResponses => {
            responses_to_messages(body)?;
        }
        UpstreamFormat::Anthropic => {
            claude_to_openai(body)?;
        }
        UpstreamFormat::Google => {
            gemini_to_openai(body)?;
        }
    }
    Ok(())
}

fn openai_completion_to_upstream(to: UpstreamFormat, body: &mut Value) -> Result<(), String> {
    match to {
        UpstreamFormat::OpenAiCompletion => {}
        UpstreamFormat::OpenAiResponses => {
            messages_to_responses(body)?;
        }
        UpstreamFormat::Anthropic => {
            openai_to_claude(body)?;
        }
        UpstreamFormat::Google => {
            openai_to_gemini(body)?;
        }
    }
    Ok(())
}

fn responses_to_messages(body: &mut Value) -> Result<(), String> {
    let input = body.get("input").ok_or("missing input")?;
    let instructions = body.get("instructions").and_then(Value::as_str);
    let mut messages: Vec<Value> = vec![];
    if let Some(s) = instructions {
        messages.push(serde_json::json!({ "role": "system", "content": s }));
    }
    let items: Vec<Value> = if input.is_string() {
        let text = input.as_str().unwrap_or("");
        vec![serde_json::json!({
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": text }]
        })]
    } else {
        body.get("input")
            .and_then(Value::as_array)
            .ok_or("input must be array or string")?
            .to_vec()
    };
    let mut current_assistant: Option<Value> = None;
    let mut deferred_user_after_tool_results: Option<Value> = None;
    let mut idx = 0;
    while idx < items.len() {
        let item = items[idx].clone();
        let item_type = item
            .get("type")
            .and_then(Value::as_str)
            .or_else(|| item.get("role").and_then(Value::as_str).map(|_| "message"));
        let Some(ty) = item_type else {
            idx += 1;
            continue;
        };
        match ty {
            "message" => {
                let role = item.get("role").and_then(Value::as_str).unwrap_or("user");
                let content = item.get("content").cloned();
                let content = map_responses_content_to_openai(content);
                if role == "assistant" {
                    let assistant = current_assistant.get_or_insert_with(|| {
                        serde_json::json!({
                            "role": "assistant",
                            "content": Value::Null
                        })
                    });
                    assistant["role"] = Value::String("assistant".to_string());
                    assistant["content"] = content;
                } else if role == "user"
                    && items
                        .get(idx + 1)
                        .and_then(|next| next.get("type").and_then(Value::as_str))
                        == Some("function_call_output")
                {
                    deferred_user_after_tool_results =
                        Some(serde_json::json!({ "role": "user", "content": content }));
                } else {
                    flush_assistant(&mut messages, &mut current_assistant);
                    messages.push(serde_json::json!({ "role": role, "content": content }));
                }
            }
            "function_call" => {
                let call_id = item.get("call_id").cloned();
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let args = item
                    .get("arguments")
                    .cloned()
                    .unwrap_or(serde_json::json!("{}"));
                let tc = serde_json::json!({
                    "id": call_id,
                    "type": "function",
                    "function": { "name": name, "arguments": args }
                });
                if current_assistant.is_none() {
                    current_assistant = Some(serde_json::json!({
                        "role": "assistant",
                        "content": null,
                        "tool_calls": []
                    }));
                }
                if let Some(ref mut a) = current_assistant {
                    if a.get("tool_calls").is_none() {
                        a["tool_calls"] = Value::Array(Vec::new());
                    }
                    if let Some(arr) = a.get_mut("tool_calls").and_then(Value::as_array_mut) {
                        arr.push(tc);
                    }
                }
            }
            "function_call_output" => {
                flush_assistant(&mut messages, &mut current_assistant);
                let call_id = item.get("call_id").cloned();
                let output = item.get("output").cloned();
                let content = match output {
                    Some(Value::String(s)) => s,
                    Some(o) => serde_json::to_string(&o).unwrap_or_default(),
                    None => String::new(),
                };
                messages.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": call_id,
                    "content": content
                }));
                let next_is_function_output = items
                    .get(idx + 1)
                    .and_then(|next| next.get("type").and_then(Value::as_str))
                    == Some("function_call_output");
                if !next_is_function_output {
                    if let Some(deferred) = deferred_user_after_tool_results.take() {
                        messages.push(deferred);
                    }
                }
            }
            "reasoning" => {
                let summary = item
                    .get("summary")
                    .and_then(Value::as_array)
                    .map(|parts| {
                        parts
                            .iter()
                            .filter_map(|part| match part.get("type").and_then(Value::as_str) {
                                Some("summary_text") => {
                                    part.get("text").and_then(Value::as_str).map(str::to_string)
                                }
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("")
                    })
                    .unwrap_or_default();
                if !summary.is_empty() {
                    if current_assistant.is_none() {
                        current_assistant = Some(serde_json::json!({
                            "role": "assistant",
                            "content": null
                        }));
                    }
                    if let Some(ref mut a) = current_assistant {
                        let existing = a
                            .get("reasoning_content")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        a["reasoning_content"] = Value::String(format!("{existing}{summary}"));
                    }
                }
            }
            _ => {}
        }
        idx += 1;
    }
    if let Some(deferred) = deferred_user_after_tool_results.take() {
        messages.push(deferred);
    }
    flush_assistant(&mut messages, &mut current_assistant);
    let max_tokens = body.get("max_output_tokens").cloned();
    let temperature = body.get("temperature").cloned();
    let top_p = body.get("top_p").cloned();
    let stop = body.get("stop").cloned();
    let tools = body.get("tools").cloned();
    let parallel_tool_calls = body.get("parallel_tool_calls").cloned();
    let stream = body.get("stream").cloned();
    let model = body.get("model").cloned();

    let mut out = serde_json::Map::new();
    if let Some(model) = model {
        out.insert("model".to_string(), model);
    }
    out.insert("messages".to_string(), Value::Array(messages));
    if let Some(stream) = stream {
        out.insert("stream".to_string(), stream);
    }
    if let Some(max_tokens) = max_tokens {
        out.insert("max_completion_tokens".to_string(), max_tokens);
    }
    if let Some(temperature) = temperature {
        out.insert("temperature".to_string(), temperature);
    }
    if let Some(top_p) = top_p {
        out.insert("top_p".to_string(), top_p);
    }
    if let Some(stop) = stop {
        out.insert("stop".to_string(), stop);
    }
    if let Some(tool_choice) = body.get("tool_choice").cloned() {
        if let Some(mapped_tool_choice) = responses_tool_choice_to_openai_tool_choice(&tool_choice)
        {
            out.insert("tool_choice".to_string(), mapped_tool_choice);
        }
    }
    if let Some(parallel_tool_calls) = parallel_tool_calls {
        out.insert("parallel_tool_calls".to_string(), parallel_tool_calls);
    }

    // Convert tools from Responses API format to Chat Completions format
    // Responses: { "name": "...", "description": "...", "parameters": {...} }
    // Chat: { "type": "function", "function": { "name": "...", "description": "...", "parameters": {...} } }
    // Note: Responses API may include non-function tools like web_search which don't have names.
    // We only convert tools that have a "name" field (function tools).
    if let Some(tools) = tools.as_ref().and_then(Value::as_array) {
        let converted_tools: Vec<Value> = tools
            .iter()
            .filter_map(|t| {
                // Check if already in Chat Completions format (has "type" = "function" and nested "function")
                if t.get("type").and_then(Value::as_str) == Some("function") && t.get("function").is_some() {
                    return Some(t.clone());
                }
                // Only convert tools that have a name field (skip non-function tools like web_search)
                let name = match t.get("name").and_then(Value::as_str) {
                    Some(n) if !n.is_empty() => n,
                    _ => return None, // Skip tools without a valid name
                };
                let description = t.get("description").and_then(Value::as_str);
                let parameters = t.get("parameters").cloned();
                Some(serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": name,
                        "description": description,
                        "parameters": parameters.unwrap_or(serde_json::json!({"type": "object", "properties": {}}))
                    }
                }))
            })
            .collect();
        if !converted_tools.is_empty() {
            out.insert("tools".to_string(), Value::Array(converted_tools));
        }
    }
    *body = Value::Object(out);
    Ok(())
}

fn flush_assistant(messages: &mut Vec<Value>, current_assistant: &mut Option<Value>) {
    if let Some(a) = current_assistant.take() {
        messages.push(a);
    }
}

fn map_responses_content_to_openai(content: Option<Value>) -> Value {
    let arr = match content {
        None => return Value::Array(vec![]),
        Some(Value::Array(a)) => a,
        Some(v) => return v,
    };
    let mut plain_text_parts: Vec<String> = Vec::new();
    let mut has_non_text_part = false;
    let out: Vec<Value> = arr
        .into_iter()
        .map(|c| {
            let ty = c.get("type").and_then(Value::as_str);
            if ty == Some("input_text") || ty == Some("output_text") {
                let text = c
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                plain_text_parts.push(text.clone());
                return serde_json::json!({ "type": "text", "text": text });
            }
            if ty == Some("input_image") {
                has_non_text_part = true;
                let mut image_url = match c.get("image_url").cloned().unwrap_or(Value::Null) {
                    Value::Object(obj) => Value::Object(obj),
                    value => serde_json::json!({ "url": value }),
                };
                if image_url.get("detail").is_none() {
                    if let Some(detail) = c.get("detail").cloned() {
                        image_url["detail"] = detail;
                    }
                }
                let image = serde_json::json!({
                    "type": "image_url",
                    "image_url": image_url
                });
                return image;
            }
            if ty == Some("input_audio") {
                has_non_text_part = true;
                return serde_json::json!({
                    "type": "input_audio",
                    "input_audio": c.get("input_audio").cloned().unwrap_or(Value::Null)
                });
            }
            if ty == Some("input_file") {
                has_non_text_part = true;
                let mut file = serde_json::Map::new();
                for key in ["file_id", "file_data", "filename"] {
                    if let Some(value) = c.get(key).cloned() {
                        file.insert(key.to_string(), value);
                    }
                }
                return serde_json::json!({
                    "type": "file",
                    "file": Value::Object(file)
                });
            }
            has_non_text_part = true;
            c
        })
        .collect();
    if !has_non_text_part {
        return Value::String(plain_text_parts.join(""));
    }
    Value::Array(out)
}

fn messages_to_responses(body: &mut Value) -> Result<(), String> {
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .ok_or("missing messages")?;
    let mut input: Vec<Value> = vec![];
    for msg in messages {
        let role = msg.get("role").and_then(Value::as_str).unwrap_or("user");
        if matches!(role, "system" | "developer" | "user" | "assistant") {
            if role == "assistant" {
                if let Some(reasoning) = msg.get("reasoning_content").and_then(Value::as_str) {
                    if !reasoning.is_empty() {
                        input.push(serde_json::json!({
                            "type": "reasoning",
                            "summary": [{ "type": "summary_text", "text": reasoning }]
                        }));
                    }
                }
            }
            let content = msg.get("content").cloned();
            let content_type = if role == "assistant" {
                "output_text"
            } else {
                "input_text"
            };
            let content_arr = map_openai_content_to_responses(content, content_type);
            if !content_arr.is_empty() {
                input.push(serde_json::json!({
                    "type": "message",
                    "role": role,
                    "content": content_arr
                }));
            }
        }
        if role == "assistant" {
            if let Some(tool_calls) = msg.get("tool_calls").and_then(Value::as_array) {
                for tc in tool_calls {
                    input.push(serde_json::json!({
                        "type": "function_call",
                        "call_id": tc.get("id"),
                        "name": tc.get("function").and_then(|f| f.get("name")).unwrap_or(&serde_json::Value::Null),
                        "arguments": tc.get("function").and_then(|f| f.get("arguments")).unwrap_or(&serde_json::json!("{}"))
                    }));
                }
            }
        }
        if role == "tool" {
            input.push(serde_json::json!({
                "type": "function_call_output",
                "call_id": msg.get("tool_call_id"),
                "output": msg.get("content")
            }));
        }
    }
    body["input"] = Value::Array(input);
    if let Some(tool_choice) = body.get("tool_choice").cloned() {
        if let Some(mapped_tool_choice) = openai_tool_choice_to_responses_tool_choice(&tool_choice)
        {
            body["tool_choice"] = mapped_tool_choice;
        } else if let Some(obj) = body.as_object_mut() {
            obj.remove("tool_choice");
        }
    }
    if let Some(parallel_tool_calls) = body.get("parallel_tool_calls").cloned() {
        body["parallel_tool_calls"] = parallel_tool_calls;
    }
    if let Some(max_output_tokens) = body
        .get("max_completion_tokens")
        .cloned()
        .or_else(|| body.get("max_tokens").cloned())
    {
        body["max_output_tokens"] = max_output_tokens;
    }
    if let Some(obj) = body.as_object_mut() {
        obj.remove("instructions");
        obj.remove("messages");
        obj.remove("max_completion_tokens");
        obj.remove("max_tokens");
    }
    Ok(())
}

fn responses_tool_choice_to_openai_tool_choice(choice: &Value) -> Option<Value> {
    if choice.is_string() {
        return Some(choice.clone());
    }
    let obj = choice.as_object()?;
    let ty = obj.get("type").and_then(Value::as_str)?;
    match ty {
        "function" => {
            let name = obj
                .get("name")
                .or_else(|| obj.get("function").and_then(|f| f.get("name")))?;
            Some(serde_json::json!({
                "type": "function",
                "function": { "name": name }
            }))
        }
        _ => None,
    }
}

fn openai_tool_choice_to_responses_tool_choice(choice: &Value) -> Option<Value> {
    if choice.is_string() {
        return Some(choice.clone());
    }
    let obj = choice.as_object()?;
    let ty = obj.get("type").and_then(Value::as_str)?;
    match ty {
        "function" => {
            let name = obj
                .get("name")
                .or_else(|| obj.get("function").and_then(|f| f.get("name")))?;
            Some(serde_json::json!({
                "type": "function",
                "name": name
            }))
        }
        _ => None,
    }
}

fn map_openai_content_to_responses(content: Option<Value>, content_type: &str) -> Vec<Value> {
    let content = match content {
        None => return vec![],
        Some(Value::String(s)) => {
            return vec![serde_json::json!({ "type": content_type, "text": s })]
        }
        Some(Value::Array(a)) => a,
        Some(_) => return vec![],
    };
    content
        .into_iter()
        .map(|c| {
            let ty = c.get("type").and_then(Value::as_str);
            if ty == Some("text") {
                let text = c
                    .get("text")
                    .cloned()
                    .unwrap_or(Value::String(String::new()));
                return serde_json::json!({ "type": content_type, "text": text });
            }
            if ty == Some("image_url") {
                let image_url = c
                    .get("image_url")
                    .and_then(|image| image.get("url").cloned())
                    .or_else(|| c.get("image_url").cloned())
                    .unwrap_or(Value::Null);
                let mut image = serde_json::json!({
                    "type": "input_image",
                    "image_url": image_url
                });
                if let Some(detail) = c
                    .get("image_url")
                    .and_then(|image| image.get("detail").cloned())
                {
                    image["detail"] = detail;
                }
                return image;
            }
            if ty == Some("input_audio") {
                return serde_json::json!({
                    "type": "input_audio",
                    "input_audio": c.get("input_audio").cloned().unwrap_or(Value::Null)
                });
            }
            if ty == Some("file") {
                let file = c.get("file").cloned().unwrap_or(Value::Null);
                let mut out = serde_json::Map::new();
                out.insert("type".to_string(), Value::String("input_file".to_string()));
                if let Some(file_obj) = file.as_object() {
                    for key in ["file_id", "file_data", "filename"] {
                        if let Some(value) = file_obj.get(key).cloned() {
                            out.insert(key.to_string(), value);
                        }
                    }
                }
                return Value::Object(out);
            }
            let text = c.get("text").or(c.get("content")).cloned();
            let text = text
                .and_then(|t| t.as_str().map(String::from))
                .unwrap_or_else(|| serde_json::to_string(&c).unwrap_or_default());
            serde_json::json!({ "type": content_type, "text": text })
        })
        .collect()
}

fn openai_message_content_to_responses_output(content: Option<&Value>) -> Vec<Value> {
    map_openai_content_to_responses(content.cloned(), "output_text")
}

fn responses_usage_to_openai_usage(usage: &Value) -> Value {
    let input_tokens = usage
        .get("input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total_tokens = usage
        .get("total_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(input_tokens + output_tokens);

    let mut mapped = serde_json::json!({
        "prompt_tokens": input_tokens,
        "completion_tokens": output_tokens,
        "total_tokens": total_tokens
    });

    if let Some(details) = usage.get("input_tokens_details") {
        if let Some(cached) = details.get("cached_tokens").and_then(Value::as_u64) {
            mapped["prompt_tokens_details"] = serde_json::json!({ "cached_tokens": cached });
        }
    }
    if let Some(details) = usage.get("output_tokens_details") {
        if let Some(reasoning) = details.get("reasoning_tokens").and_then(Value::as_u64) {
            mapped["completion_tokens_details"] =
                serde_json::json!({ "reasoning_tokens": reasoning });
        }
    }

    mapped
}

fn openai_usage_to_responses_usage(usage: &Value) -> Value {
    let input_tokens = usage
        .get("prompt_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage
        .get("completion_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total_tokens = usage
        .get("total_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(input_tokens + output_tokens);

    let mut mapped = serde_json::json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "total_tokens": total_tokens
    });

    if let Some(details) = usage.get("prompt_tokens_details") {
        if let Some(cached) = details.get("cached_tokens").and_then(Value::as_u64) {
            mapped["input_tokens_details"] = serde_json::json!({ "cached_tokens": cached });
        }
    }
    if let Some(details) = usage.get("completion_tokens_details") {
        if let Some(reasoning) = details.get("reasoning_tokens").and_then(Value::as_u64) {
            mapped["output_tokens_details"] = serde_json::json!({ "reasoning_tokens": reasoning });
        }
    }

    mapped
}

fn openai_usage_to_gemini_usage(usage: Option<&Value>) -> Value {
    let Some(usage) = usage else {
        return serde_json::json!({});
    };

    let prompt_tokens = usage
        .get("prompt_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let completion_tokens = usage
        .get("completion_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total_tokens = usage
        .get("total_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(prompt_tokens + completion_tokens);
    let reasoning_tokens = usage
        .get("completion_tokens_details")
        .and_then(|d| d.get("reasoning_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cached_tokens = usage
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);

    let mut mapped = serde_json::json!({
        "promptTokenCount": prompt_tokens,
        "candidatesTokenCount": completion_tokens.saturating_sub(reasoning_tokens),
        "totalTokenCount": total_tokens
    });

    if reasoning_tokens > 0 {
        mapped["thoughtsTokenCount"] = reasoning_tokens.into();
    }
    if cached_tokens > 0 {
        mapped["cachedContentTokenCount"] = cached_tokens.into();
    }

    mapped
}

fn claude_to_openai(body: &mut Value) -> Result<(), String> {
    let mut result = serde_json::json!({
        "model": body.get("model").cloned().unwrap_or(serde_json::Value::Null),
        "messages": [],
        "stream": body.get("stream").cloned().unwrap_or(serde_json::json!(false))
    });
    if let Some(max_tokens) = body.get("max_tokens") {
        result["max_tokens"] = max_tokens.clone();
    }
    if let Some(t) = body.get("temperature") {
        result["temperature"] = t.clone();
    }
    if let Some(tp) = body.get("top_p") {
        result["top_p"] = tp.clone();
    }
    if let Some(stop_sequences) = body.get("stop_sequences") {
        result["stop"] = if stop_sequences
            .as_array()
            .map(|arr| arr.len() == 1)
            .unwrap_or(false)
        {
            stop_sequences[0].clone()
        } else {
            stop_sequences.clone()
        };
    }
    // System: strip cache_control from blocks
    // Reference: 9router claudeHelper.js - remove all cache_control, add only to last block
    if let Some(system) = body.get("system") {
        let text = if system.is_string() {
            system.as_str().unwrap_or("").to_string()
        } else if let Some(arr) = system.as_array() {
            arr.iter()
                .filter_map(|s| {
                    // Strip cache_control - just extract text
                    s.get("text").and_then(Value::as_str)
                })
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            String::new()
        };
        if !text.is_empty() {
            result["messages"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({ "role": "system", "content": text }));
        }
    }
    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        for msg in messages {
            if let Some(openai_msg) = convert_claude_message_to_openai(msg) {
                for m in openai_msg {
                    result["messages"].as_array_mut().unwrap().push(m);
                }
            }
        }
    }
    // Tools: strip cache_control
    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
        let converted_tools: Vec<Value> = tools
            .iter()
            .filter_map(|t| {
                // Skip if no name (invalid tool)
                let name = t.get("name")?;
                Some(serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": name,
                        "description": t.get("description"),
                        "parameters": t.get("input_schema").or(t.get("parameters")).unwrap_or(&serde_json::json!({ "type": "object", "properties": {} }))
                    }
                }))
            })
            .collect();
        if !converted_tools.is_empty() {
            result["tools"] = Value::Array(converted_tools);
        }
    }
    *body = result;
    Ok(())
}

fn convert_claude_message_to_openai(msg: &Value) -> Option<Vec<Value>> {
    let role = msg.get("role").and_then(Value::as_str)?;
    let openai_role = if role == "user" || role == "tool" {
        "user"
    } else {
        "assistant"
    };
    let content = msg.get("content")?;
    if content.is_string() {
        return Some(vec![
            serde_json::json!({ "role": openai_role, "content": content }),
        ]);
    }
    let arr = content.as_array()?;
    let mut parts: Vec<Value> = vec![];
    let mut tool_calls: Vec<Value> = vec![];
    let mut tool_results: Vec<Value> = vec![];
    let mut reasoning_text: String = String::new();
    for block in arr {
        let ty = block.get("type").and_then(Value::as_str)?;
        match ty {
            // Strip cache_control when converting from Claude to OpenAI
            // Reference: 9router claudeHelper.js - remove all cache_control
            "text" => {
                let text = block.get("text").cloned().unwrap_or(Value::String(String::new()));
                parts.push(serde_json::json!({ "type": "text", "text": text }));
            }
            "image" => {
                if block.get("source").and_then(|s| s.get("type").and_then(Value::as_str)) == Some("base64") {
                    let src = block.get("source").unwrap();
                    let media = src.get("media_type").and_then(Value::as_str).unwrap_or("image/png");
                    let data = src.get("data").and_then(Value::as_str).unwrap_or("");
                    parts.push(serde_json::json!({
                        "type": "image_url",
                        "image_url": { "url": format!("data:{};base64,{}", media, data) }
                    }));
                }
            }
            "tool_use" => tool_calls.push(serde_json::json!({
                "id": block.get("id"),
                "type": "function",
                "function": {
                    "name": block.get("name"),
                    "arguments": block.get("input").map(|i| serde_json::to_string(i).unwrap_or_else(|_| "{}".into())).unwrap_or_else(|| "{}".to_string())
                }
            })),
            "server_tool_use" => tool_calls.push(serde_json::json!({
                "id": block.get("id"),
                "type": "function",
                "proxied_tool_kind": "anthropic_server_tool_use",
                "function": {
                    "name": block.get("name"),
                    "arguments": block.get("input").map(|i| serde_json::to_string(i).unwrap_or_else(|_| "{}".into())).unwrap_or_else(|| "{}".to_string())
                }
            })),
            "tool_result" => {
                let c = block.get("content");
                let content_str = c
                    .and_then(Value::as_str)
                    .map(String::from)
                    .unwrap_or_else(|| c.map(|x| serde_json::to_string(x).unwrap_or_default()).unwrap_or_default());
                tool_results.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": block.get("tool_use_id"),
                    "content": content_str
                }));
            }
            "thinking" => {
                // Convert thinking blocks to reasoning_content on assistant messages
                if let Some(t) = block.get("thinking").and_then(Value::as_str) {
                    if !t.is_empty() {
                        reasoning_text.push_str(t);
                    }
                }
            }
            // redacted_thinking: encrypted thinking cannot be translated; silently drop
            _ => {}
        }
    }
    if !tool_results.is_empty() {
        let mut out: Vec<Value> = tool_results;
        if !parts.is_empty() {
            let content = collapse_claude_text_parts_for_openai(&parts);
            out.push(serde_json::json!({ "role": "user", "content": content }));
        }
        return Some(out);
    }
    if !tool_calls.is_empty() {
        let mut m = serde_json::json!({ "role": "assistant", "tool_calls": tool_calls });
        if !parts.is_empty() {
            m["content"] = collapse_claude_text_parts_for_openai(&parts);
        }
        if !reasoning_text.is_empty() {
            m["reasoning_content"] = Value::String(reasoning_text);
        }
        return Some(vec![m]);
    }
    if parts.is_empty() {
        let mut m = serde_json::json!({ "role": openai_role, "content": "" });
        if !reasoning_text.is_empty() {
            m["reasoning_content"] = Value::String(reasoning_text);
        }
        return Some(vec![m]);
    }
    let content = collapse_claude_text_parts_for_openai(&parts);
    let mut m = serde_json::json!({ "role": openai_role, "content": content });
    if !reasoning_text.is_empty() {
        m["reasoning_content"] = Value::String(reasoning_text);
    }
    Some(vec![m])
}

fn collapse_claude_text_parts_for_openai(parts: &[Value]) -> Value {
    let all_text = parts
        .iter()
        .all(|part| part.get("type").and_then(Value::as_str) == Some("text"));
    if all_text {
        return Value::String(
            parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<String>(),
        );
    }
    Value::Array(parts.to_vec())
}

fn openai_to_claude(body: &mut Value) -> Result<(), String> {
    let mut result = serde_json::json!({
        "model": body.get("model").cloned().unwrap_or(serde_json::Value::Null),
        "max_tokens": body
            .get("max_completion_tokens")
            .cloned()
            .or_else(|| body.get("max_tokens").cloned())
            .unwrap_or(serde_json::json!(4096)),
        "messages": [],
        "stream": body.get("stream").cloned().unwrap_or(serde_json::json!(false))
    });
    if let Some(t) = body.get("temperature") {
        result["temperature"] = t.clone();
    }
    if let Some(tp) = body.get("top_p") {
        result["top_p"] = tp.clone();
    }
    if let Some(stop) = body.get("stop") {
        result["stop_sequences"] = if stop.is_array() {
            stop.clone()
        } else {
            Value::Array(vec![stop.clone()])
        };
    }
    if let Some(metadata) = body.get("metadata") {
        result["metadata"] = metadata.clone();
    }
    if let Some(tool_choice) = body.get("tool_choice") {
        if let Some(mapped_tool_choice) =
            openai_tool_choice_to_claude_tool_choice(tool_choice, body.get("parallel_tool_calls"))
        {
            result["tool_choice"] = mapped_tool_choice;
        }
    } else if body.get("parallel_tool_calls").and_then(Value::as_bool) == Some(false)
        && body
            .get("tools")
            .and_then(Value::as_array)
            .map(|t| !t.is_empty())
            .unwrap_or(false)
    {
        result["tool_choice"] =
            serde_json::json!({ "type": "auto", "disable_parallel_tool_use": true });
    }
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .ok_or("missing messages")?;

    let mut system_blocks: Vec<Value> = vec![];
    for msg in messages {
        let role = msg.get("role").and_then(Value::as_str);
        if !matches!(role, Some("system") | Some("developer")) {
            continue;
        }
        let c = msg.get("content");
        let text = c
            .and_then(Value::as_str)
            .map(String::from)
            .unwrap_or_else(|| extract_text_content(c));
        if !text.is_empty() {
            system_blocks.push(serde_json::json!({ "type": "text", "text": text }));
        }
    }
    if !system_blocks.is_empty() {
        result["system"] = Value::Array(system_blocks);
    }

    let non_system: Vec<_> = messages
        .iter()
        .filter(|m| {
            !matches!(
                m.get("role").and_then(Value::as_str),
                Some("system") | Some("developer")
            )
        })
        .cloned()
        .collect();

    let mut pending_tool_results: Vec<Value> = vec![];
    for msg in non_system {
        let role = msg.get("role").and_then(Value::as_str).unwrap_or("user");

        if role == "tool" {
            if let Some(tool_blocks) = openai_message_to_claude_blocks(&msg) {
                pending_tool_results.extend(tool_blocks);
            }
            continue;
        }

        if let Some(mut claude_blocks) = openai_message_to_claude_blocks(&msg) {
            if role == "user" && !pending_tool_results.is_empty() {
                let mut merged = pending_tool_results.clone();
                merged.append(&mut claude_blocks);
                pending_tool_results.clear();
                claude_blocks = merged;
            } else if !pending_tool_results.is_empty() {
                result["messages"].as_array_mut().unwrap().push(
                    serde_json::json!({ "role": "user", "content": pending_tool_results.clone() }),
                );
                pending_tool_results.clear();
            }
            result["messages"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({
                    "role": if role == "assistant" { "assistant" } else { "user" },
                    "content": claude_blocks
                }));
        }
    }

    if !pending_tool_results.is_empty() {
        result["messages"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({ "role": "user", "content": pending_tool_results }));
    }

    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
        let claude_tools: Vec<Value> = tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.get("function").and_then(|f| f.get("name")),
                    "description": t.get("function").and_then(|f| f.get("description")),
                    "input_schema": t.get("function").and_then(|f| f.get("parameters"))
                })
            })
            .collect();
        if !claude_tools.is_empty() {
            result["tools"] = Value::Array(claude_tools);
        }
    }
    *body = result;
    Ok(())
}

fn openai_tool_choice_to_claude_tool_choice(
    choice: &Value,
    parallel_tool_calls: Option<&Value>,
) -> Option<Value> {
    let disable_parallel_tool_use = parallel_tool_calls.and_then(Value::as_bool) == Some(false);
    let mut mapped = if let Some(choice_str) = choice.as_str() {
        match choice_str {
            "none" => serde_json::json!({ "type": "none" }),
            "auto" => serde_json::json!({ "type": "auto" }),
            "required" => serde_json::json!({ "type": "any" }),
            _ => return None,
        }
    } else {
        let obj = choice.as_object()?;
        let ty = obj.get("type").and_then(Value::as_str)?;
        match ty {
            "function" => {
                let name = obj
                    .get("name")
                    .or_else(|| obj.get("function").and_then(|f| f.get("name")))?;
                serde_json::json!({ "type": "tool", "name": name })
            }
            _ => return None,
        }
    };

    if disable_parallel_tool_use && mapped.get("type").and_then(Value::as_str) != Some("none") {
        mapped["disable_parallel_tool_use"] = Value::Bool(true);
    }
    Some(mapped)
}

#[cfg(test)]
fn can_attach_cache_control_to_content_block(block: &Value) -> bool {
    matches!(
        block.get("type").and_then(Value::as_str),
        Some("text") | Some("thinking") | Some("redacted_thinking")
    )
}

fn extract_text_content(content: Option<&Value>) -> String {
    let content = match content {
        Some(c) => c,
        None => return String::new(),
    };
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    let arr = match content.as_array() {
        Some(a) => a,
        None => return String::new(),
    };
    arr.iter()
        .filter_map(|c| {
            if c.get("type").and_then(Value::as_str) == Some("text") {
                c.get("text").and_then(Value::as_str)
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn collapse_gemini_parts_for_openai(parts: &[Value]) -> Value {
    if parts.len() == 1 && parts[0].get("type").and_then(Value::as_str) == Some("text") {
        return parts[0]
            .get("text")
            .cloned()
            .unwrap_or(Value::String(String::new()));
    }
    Value::Array(parts.to_vec())
}

fn openai_message_to_claude_blocks(msg: &Value) -> Option<Vec<Value>> {
    let role = msg.get("role").and_then(Value::as_str)?;
    if role == "tool" {
        return Some(vec![serde_json::json!({
            "type": "tool_result",
            "tool_use_id": msg.get("tool_call_id"),
            "content": msg.get("content").and_then(Value::as_str).unwrap_or("")
        })]);
    }
    let content = msg.get("content");
    let mut blocks: Vec<Value> = vec![];
    if role == "assistant" {
        if let Some(reasoning) = openai_message_reasoning_text(msg) {
            if !reasoning.is_empty() {
                blocks.push(serde_json::json!({
                    "type": "text",
                    "text": reasoning
                }));
            }
        }
    }
    match content {
        Some(Value::String(s)) => {
            blocks.push(serde_json::json!({ "type": "text", "text": s }));
        }
        Some(Value::Array(arr)) => {
            for c in arr {
                let ty = c.get("type").and_then(Value::as_str);
                if ty == Some("text") {
                    blocks.push(serde_json::json!({ "type": "text", "text": c.get("text") }));
                } else if ty == Some("image_url") {
                    let url = c
                        .get("image_url")
                        .and_then(|u| u.get("url").and_then(Value::as_str))
                        .unwrap_or("");
                    if url.starts_with("data:") {
                        let rest = url.strip_prefix("data:").unwrap_or("");
                        let (media, b64) = rest.split_once(";base64,").unwrap_or(("image/png", ""));
                        blocks.push(serde_json::json!({
                            "type": "image",
                            "source": { "type": "base64", "media_type": media, "data": b64 }
                        }));
                    }
                }
            }
        }
        _ => {}
    }
    if role == "assistant" {
        if let Some(tc) = msg.get("tool_calls").and_then(Value::as_array) {
            for t in tc {
                blocks.push(serde_json::json!({
                    "type": anthropic_tool_use_type_for_openai_tool_call(t),
                    "id": t.get("id"),
                    "name": t.get("function").and_then(|f| f.get("name")),
                    "input": t.get("function").and_then(|f| f.get("arguments")).and_then(|a| serde_json::from_str(a.as_str().unwrap_or("{}")).ok()).unwrap_or(serde_json::json!({}))
                }));
            }
        }
    }
    if blocks.is_empty() && content.is_some() {
        return Some(vec![serde_json::json!({ "type": "text", "text": "" })]);
    }
    if blocks.is_empty() {
        return None;
    }
    Some(blocks)
}

#[cfg(test)]
mod translate_regression_tests {
    use super::{
        can_attach_cache_control_to_content_block, convert_claude_message_to_openai,
        openai_message_to_claude_blocks, openai_to_claude,
    };
    use serde_json::json;

    #[test]
    fn assistant_string_content_preserves_tool_calls_for_claude() {
        let msg = json!({
            "role": "assistant",
            "content": "Let me check that.",
            "tool_calls": [
                {
                    "id": "call_123",
                    "type": "function",
                    "function": {
                        "name": "exec_command",
                        "arguments": "{\"cmd\":\"pwd\"}"
                    }
                }
            ]
        });

        let blocks = openai_message_to_claude_blocks(&msg).expect("assistant blocks");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[1]["type"], "tool_use");
        assert_eq!(blocks[1]["id"], "call_123");
        assert_eq!(blocks[1]["name"], "exec_command");
    }

    #[test]
    fn assistant_reasoning_content_rehydrates_to_claude_thinking_before_text_and_tools() {
        let msg = json!({
            "role": "assistant",
            "reasoning_content": "I should call a tool.",
            "content": "Let me check that.",
            "tool_calls": [
                {
                    "id": "call_123",
                    "type": "function",
                    "function": {
                        "name": "exec_command",
                        "arguments": "{\"cmd\":\"pwd\"}"
                    }
                }
            ]
        });

        let blocks = openai_message_to_claude_blocks(&msg).expect("assistant blocks");
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[0]["text"], "I should call a tool.");
        assert_eq!(blocks[1]["type"], "text");
        assert_eq!(blocks[2]["type"], "tool_use");
        assert!(blocks
            .iter()
            .all(|block| block["type"] != "redacted_thinking"));
        assert!(blocks
            .iter()
            .all(|block| block["type"] != "server_tool_use"));
    }

    #[test]
    fn claude_server_tool_use_is_preserved_as_marked_openai_tool_call() {
        let message = json!({
            "role": "assistant",
            "content": [{
                "type": "server_tool_use",
                "id": "toolu_server_1",
                "name": "web_search",
                "input": { "query": "rust" }
            }]
        });

        let translated = convert_claude_message_to_openai(&message).expect("translated message");
        assert_eq!(translated.len(), 1);
        let tool_calls = translated[0]["tool_calls"].as_array().expect("tool calls");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0]["function"]["name"], "web_search");
        assert_eq!(
            tool_calls[0]["proxied_tool_kind"],
            "anthropic_server_tool_use"
        );
    }

    #[test]
    fn marked_openai_server_tool_call_restores_server_tool_use_block() {
        let message = json!({
            "role": "assistant",
            "tool_calls": [{
                "id": "toolu_server_1",
                "type": "function",
                "proxied_tool_kind": "anthropic_server_tool_use",
                "function": {
                    "name": "web_search",
                    "arguments": "{\"query\":\"rust\"}"
                }
            }]
        });

        let blocks = openai_message_to_claude_blocks(&message).expect("assistant blocks");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["type"], "server_tool_use");
        assert_eq!(blocks[0]["name"], "web_search");
    }

    #[test]
    fn openai_to_claude_merges_tool_results_into_single_user_message() {
        let mut body = json!({
            "model": "codex-anthropic",
            "messages": [
                {
                    "role": "assistant",
                    "content": "I'll run the commands.",
                    "tool_calls": [
                        {
                            "id": "call_a",
                            "type": "function",
                            "function": { "name": "cmd_a", "arguments": "{}" }
                        },
                        {
                            "id": "call_b",
                            "type": "function",
                            "function": { "name": "cmd_b", "arguments": "{}" }
                        }
                    ]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_a",
                    "content": "result-a"
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_b",
                    "content": "result-b"
                }
            ]
        });

        openai_to_claude(&mut body).expect("translate to claude");
        let messages = body["messages"].as_array().expect("messages array");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "assistant");
        assert_eq!(messages[1]["role"], "user");
        let content = messages[1]["content"].as_array().expect("user content");
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[0]["tool_use_id"], "call_a");
        assert_eq!(content[1]["type"], "tool_result");
        assert_eq!(content[1]["tool_use_id"], "call_b");
    }

    #[test]
    fn openai_to_claude_puts_user_text_after_tool_results() {
        let mut body = json!({
            "model": "codex-anthropic",
            "messages": [
                {
                    "role": "assistant",
                    "tool_calls": [
                        {
                            "id": "call_a",
                            "type": "function",
                            "function": { "name": "cmd_a", "arguments": "{}" }
                        }
                    ]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_a",
                    "content": "result-a"
                },
                {
                    "role": "user",
                    "content": "continue"
                }
            ]
        });

        openai_to_claude(&mut body).expect("translate to claude");
        let messages = body["messages"].as_array().expect("messages array");
        assert_eq!(messages.len(), 2);
        let user_content = messages[1]["content"].as_array().expect("user content");
        assert_eq!(user_content[0]["type"], "tool_result");
        assert_eq!(user_content[1]["type"], "text");
        assert_eq!(user_content[1]["text"], "continue");
    }

    #[test]
    fn assistant_tool_use_block_does_not_get_cache_control() {
        let mut body = json!({
            "model": "codex-anthropic",
            "messages": [
                {
                    "role": "assistant",
                    "content": "Let me check.",
                    "tool_calls": [
                        {
                            "id": "call_a",
                            "type": "function",
                            "function": { "name": "cmd_a", "arguments": "{}" }
                        }
                    ]
                }
            ]
        });

        openai_to_claude(&mut body).expect("translate to claude");
        let messages = body["messages"].as_array().expect("messages array");
        let assistant_content = messages[0]["content"]
            .as_array()
            .expect("assistant content");
        assert_eq!(assistant_content[1]["type"], "tool_use");
        assert!(assistant_content[1].get("cache_control").is_none());
        assert!(can_attach_cache_control_to_content_block(
            &assistant_content[0]
        ));
        assert!(!can_attach_cache_control_to_content_block(
            &assistant_content[1]
        ));
    }
}

fn extract_gemini_text(content: &Value) -> String {
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    if let Some(parts) = content.get("parts").and_then(Value::as_array) {
        return parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("");
    }
    String::new()
}

fn gemini_to_openai(body: &mut Value) -> Result<(), String> {
    let mut result = serde_json::json!({
        "model": body.get("model").cloned().unwrap_or(serde_json::Value::Null),
        "messages": [],
        "stream": body.get("stream").cloned().unwrap_or(serde_json::json!(false))
    });
    if let Some(gc) = body.get("generationConfig") {
        if let Some(n) = gc.get("maxOutputTokens") {
            result["max_tokens"] = n.clone();
        }
        if let Some(t) = gc.get("temperature") {
            result["temperature"] = t.clone();
        }
        if let Some(p) = gc.get("topP") {
            result["top_p"] = p.clone();
        }
    }
    if let Some(si) = body.get("systemInstruction") {
        let text = extract_gemini_text(si);
        if !text.is_empty() {
            result["messages"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({ "role": "system", "content": text }));
        }
    }
    if let Some(contents) = body.get("contents").and_then(Value::as_array) {
        for content in contents {
            for msg in convert_gemini_content_to_openai(content) {
                result["messages"].as_array_mut().unwrap().push(msg);
            }
        }
    }
    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
        let mut out = vec![];
        for tool in tools {
            if let Some(decls) = tool.get("functionDeclarations").and_then(Value::as_array) {
                for f in decls {
                    out.push(serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": f.get("name"),
                            "description": f.get("description").unwrap_or(&serde_json::json!("")),
                            "parameters": f.get("parameters").unwrap_or(&serde_json::json!({ "type": "object", "properties": {} }))
                        }
                    }));
                }
            }
        }
        result["tools"] = Value::Array(out);
    }
    if let Some(config) = body
        .get("toolConfig")
        .and_then(|tool_config| tool_config.get("functionCallingConfig"))
    {
        let mode = config.get("mode").and_then(Value::as_str);
        let allowed = config
            .get("allowedFunctionNames")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        match mode {
            Some("NONE") => result["tool_choice"] = serde_json::json!("none"),
            Some("AUTO") => result["tool_choice"] = serde_json::json!("auto"),
            Some("ANY") => {
                if allowed.len() == 1 {
                    result["tool_choice"] = serde_json::json!({
                        "type": "function",
                        "function": { "name": allowed[0].clone() }
                    });
                } else {
                    result["tool_choice"] = serde_json::json!("required");
                    if !allowed.is_empty() {
                        result["allowed_tool_names"] = Value::Array(allowed);
                    }
                }
            }
            _ => {}
        }
    }
    *body = result;
    Ok(())
}

fn gemini_part_field<'a>(part: &'a Value, camel: &str, snake: &str) -> Option<&'a Value> {
    part.get(camel).or_else(|| part.get(snake))
}

fn gemini_nested_field<'a>(value: &'a Value, camel: &str, snake: &str) -> Option<&'a Value> {
    value.get(camel).or_else(|| value.get(snake))
}

fn convert_gemini_content_to_openai(content: &Value) -> Vec<Value> {
    let role = content
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("user");
    let openai_role = if role == "user" { "user" } else { "assistant" };
    let Some(parts) = content.get("parts").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut messages = Vec::new();
    let mut openai_parts: Vec<Value> = vec![];
    let mut tool_calls: Vec<Value> = vec![];
    let mut reasoning_content = String::new();
    for part in parts {
        if part.get("thought").and_then(Value::as_bool) == Some(true) {
            if role != "user" {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    reasoning_content.push_str(text);
                }
            }
            continue;
        }
        if part.get("text").is_some() {
            openai_parts.push(serde_json::json!({ "type": "text", "text": part.get("text") }));
        }
        if let Some(inline) = gemini_part_field(part, "inlineData", "inline_data") {
            let mime = inline
                .get("mimeType")
                .or_else(|| inline.get("mime_type"))
                .and_then(Value::as_str)
                .unwrap_or("image/png");
            let data = inline.get("data").and_then(Value::as_str).unwrap_or("");
            openai_parts.push(serde_json::json!({
                "type": "image_url",
                "image_url": { "url": format!("data:{};base64,{}", mime, data) }
            }));
        }
        if let Some(fc) = gemini_part_field(part, "functionCall", "function_call") {
            let id = fc
                .get("id")
                .cloned()
                .unwrap_or_else(|| serde_json::json!(format!("call_{}", uuid_simple())));
            tool_calls.push(serde_json::json!({
                "id": id,
                "type": "function",
                "function": {
                    "name": fc.get("name"),
                    "arguments": fc.get("args").map(|a| serde_json::to_string(a).unwrap_or_else(|_| "{}".into())).unwrap_or_else(|| "{}".to_string())
                }
            }));
        }
        if let Some(fr) = gemini_part_field(part, "functionResponse", "function_response") {
            if !openai_parts.is_empty() {
                messages.push(serde_json::json!({
                    "role": openai_role,
                    "content": collapse_gemini_parts_for_openai(&openai_parts)
                }));
                openai_parts.clear();
            }
            let call_id = fr.get("id").or(fr.get("name")).cloned();
            let resp = gemini_nested_field(fr, "response", "response");
            let content_str = resp
                .and_then(|r| r.get("result").cloned())
                .or_else(|| resp.cloned())
                .map(|v| serde_json::to_string(&v).unwrap_or_default())
                .unwrap_or_default();
            messages.push(serde_json::json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": content_str
            }));
            continue;
        }
    }
    if !tool_calls.is_empty() {
        let mut m = serde_json::json!({ "role": "assistant", "tool_calls": tool_calls });
        if !openai_parts.is_empty() {
            m["content"] = collapse_gemini_parts_for_openai(&openai_parts);
        }
        if !reasoning_content.is_empty() {
            m["reasoning_content"] = Value::String(reasoning_content);
        }
        messages.push(m);
        return messages;
    }
    if openai_parts.is_empty() {
        if !reasoning_content.is_empty() {
            messages.push(serde_json::json!({
                "role": openai_role,
                "content": "",
                "reasoning_content": reasoning_content
            }));
        }
        return messages;
    }
    let mut message = serde_json::json!({
        "role": openai_role,
        "content": collapse_gemini_parts_for_openai(&openai_parts)
    });
    if !reasoning_content.is_empty() {
        message["reasoning_content"] = Value::String(reasoning_content);
    }
    messages.push(message);
    messages
}

fn uuid_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{t:x}")
}

fn openai_tool_choice_to_gemini_function_calling_config(
    choice: &Value,
    allowed_tool_names: Option<&Value>,
) -> Option<Value> {
    let allowed_names = allowed_tool_names
        .and_then(Value::as_array)
        .cloned()
        .filter(|names| !names.is_empty());

    if let Some(choice_str) = choice.as_str() {
        return match choice_str {
            "auto" => Some(serde_json::json!({ "mode": "AUTO" })),
            "none" => Some(serde_json::json!({ "mode": "NONE" })),
            "required" => {
                let mut config = serde_json::json!({ "mode": "ANY" });
                if let Some(allowed_names) = allowed_names {
                    config["allowedFunctionNames"] = Value::Array(allowed_names);
                }
                Some(config)
            }
            _ => None,
        };
    }

    let obj = choice.as_object()?;
    let ty = obj.get("type").and_then(Value::as_str)?;
    if ty != "function" {
        return None;
    }
    let name = obj.get("name").or_else(|| {
        obj.get("function")
            .and_then(|function| function.get("name"))
    })?;
    Some(serde_json::json!({
        "mode": "ANY",
        "allowedFunctionNames": [name]
    }))
}

fn flush_pending_gemini_function_responses(
    contents: &mut Vec<Value>,
    pending_tool_parts: &mut Vec<(usize, Value)>,
) {
    if pending_tool_parts.is_empty() {
        return;
    }

    pending_tool_parts.sort_by_key(|(sort_key, _)| *sort_key);
    let parts = pending_tool_parts
        .drain(..)
        .map(|(_, part)| part)
        .collect::<Vec<_>>();
    contents.push(serde_json::json!({ "role": "user", "parts": parts }));
}

fn openai_to_gemini(body: &mut Value) -> Result<(), String> {
    let mut result = serde_json::json!({
        "model": body.get("model").cloned().unwrap_or(serde_json::Value::Null),
        "contents": [],
        "generationConfig": {},
        "safetySettings": []
    });
    if let Some(mt) = body
        .get("max_completion_tokens")
        .cloned()
        .or_else(|| body.get("max_tokens").cloned())
    {
        result["generationConfig"]["maxOutputTokens"] = mt.clone();
    }
    if let Some(t) = body.get("temperature") {
        result["generationConfig"]["temperature"] = t.clone();
    }
    if let Some(p) = body.get("top_p") {
        result["generationConfig"]["topP"] = p.clone();
    }
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .ok_or("missing messages")?;
    let mut tool_name_by_call_id = std::collections::HashMap::new();
    let mut tool_sort_key_by_call_id = std::collections::HashMap::new();
    let mut next_tool_sort_key = 0usize;
    let mut contents: Vec<Value> = Vec::new();
    let mut system_segments: Vec<String> = Vec::new();
    let mut pending_tool_parts: Vec<(usize, Value)> = Vec::new();
    for msg in messages {
        let role = msg.get("role").and_then(Value::as_str).unwrap_or("user");
        if role != "tool" {
            flush_pending_gemini_function_responses(&mut contents, &mut pending_tool_parts);
        }
        if role == "system" || role == "developer" {
            let text = extract_text_content(msg.get("content"));
            if !text.is_empty() {
                system_segments.push(text);
            }
            continue;
        }
        if role == "user" {
            let parts = openai_content_to_gemini_parts(msg.get("content"));
            if !parts.is_empty() {
                contents.push(serde_json::json!({ "role": "user", "parts": parts }));
            }
        }
        if role == "assistant" {
            let mut parts: Vec<Value> = vec![];
            if let Some(c) = msg.get("content") {
                let text = extract_text_content(Some(c));
                if !text.is_empty() {
                    parts.push(serde_json::json!({ "text": text }));
                }
            }
            if let Some(tc) = msg.get("tool_calls").and_then(Value::as_array) {
                for (idx, t) in tc.iter().enumerate() {
                    let name = t.get("function").and_then(|f| f.get("name")).cloned();
                    if let (Some(id), Some(name)) =
                        (t.get("id").and_then(Value::as_str), name.clone())
                    {
                        tool_name_by_call_id.insert(id.to_string(), name);
                        tool_sort_key_by_call_id.insert(id.to_string(), next_tool_sort_key);
                        next_tool_sort_key += 1;
                    }
                    push_gemini_function_call_part(&mut parts, t, idx == 0);
                }
            }
            if !parts.is_empty() {
                contents.push(serde_json::json!({ "role": "model", "parts": parts }));
            }
        }
        if role == "tool" {
            let call_id = msg.get("tool_call_id").cloned();
            let call_id_str = msg.get("tool_call_id").and_then(Value::as_str);
            let function_name = msg
                .get("tool_call_id")
                .and_then(Value::as_str)
                .and_then(|id| tool_name_by_call_id.get(id).cloned())
                .or_else(|| msg.get("name").cloned())
                .unwrap_or_else(|| call_id.clone().unwrap_or(Value::Null));
            let content = msg
                .get("content")
                .cloned()
                .unwrap_or(Value::String(String::new()));
            let sort_key = call_id_str
                .and_then(|id| tool_sort_key_by_call_id.get(id).copied())
                .unwrap_or(next_tool_sort_key + pending_tool_parts.len());
            pending_tool_parts.push((
                sort_key,
                serde_json::json!({
                    "functionResponse": {
                        "id": call_id,
                        "name": function_name,
                        "response": { "result": content }
                    }
                }),
            ));
        }
    }
    flush_pending_gemini_function_responses(&mut contents, &mut pending_tool_parts);
    if !system_segments.is_empty() {
        result["systemInstruction"] = serde_json::json!({
            "role": "user",
            "parts": [{ "text": system_segments.join("\n\n") }]
        });
    }
    result["contents"] = Value::Array(contents);
    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
        let mut decls = vec![];
        for t in tools {
            if let Some(f) = t.get("function") {
                decls.push(serde_json::json!({
                    "name": f.get("name"),
                    "description": f.get("description"),
                    "parameters": f.get("parameters").unwrap_or(&serde_json::json!({ "type": "object", "properties": {} }))
                }));
            }
        }
        if !decls.is_empty() {
            result["tools"] = serde_json::json!([{ "functionDeclarations": decls }]);
        }
    }
    if let Some(tool_choice) = body.get("tool_choice") {
        if let Some(config) = openai_tool_choice_to_gemini_function_calling_config(
            tool_choice,
            body.get("allowed_tool_names"),
        ) {
            result["toolConfig"]["functionCallingConfig"] = config;
        }
    }
    *body = result;
    Ok(())
}

fn openai_content_to_gemini_parts(content: Option<&Value>) -> Vec<Value> {
    let content = match content {
        Some(c) => c,
        None => return vec![],
    };
    if let Some(s) = content.as_str() {
        return vec![serde_json::json!({ "text": s })];
    }
    let arr = match content.as_array() {
        Some(a) => a,
        None => return vec![],
    };
    let mut parts = vec![];
    for c in arr {
        if c.get("type").and_then(Value::as_str) == Some("text") {
            if let Some(t) = c.get("text") {
                parts.push(serde_json::json!({ "text": t }));
            }
        } else if c.get("type").and_then(Value::as_str) == Some("image_url") {
            let url = c
                .get("image_url")
                .and_then(|u| u.get("url").and_then(Value::as_str))
                .unwrap_or("");
            if let Some((mime, data)) = url
                .strip_prefix("data:")
                .and_then(|r| r.split_once(";base64,"))
            {
                parts.push(serde_json::json!({
                    "inlineData": { "mimeType": mime, "data": data }
                }));
            }
        }
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn responses_to_messages_via_translate() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": [
                { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "Hi" }] }
            ],
            "instructions": "You are helpful."
        });
        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            true,
        )
        .unwrap();
        assert!(body.get("messages").is_some());
        assert!(body.get("input").is_none());
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "You are helpful.");
        assert_eq!(messages[1]["role"], "user");
    }

    #[test]
    fn messages_to_responses_via_translate() {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [
                { "role": "system", "content": "Helper" },
                { "role": "user", "content": "Hi" }
            ]
        });
        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            "gpt-4o",
            &mut body,
            true,
        )
        .unwrap();
        assert!(body.get("input").is_some());
        assert_eq!(body["instructions"], "Helper");
        let input = body["input"].as_array().unwrap();
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[0]["role"], "user");
    }

    #[test]
    fn messages_to_responses_preserves_reasoning_items() {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [
                {
                    "role": "assistant",
                    "reasoning_content": "thinking",
                    "content": "Hi"
                }
            ]
        });
        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            "gpt-4o",
            &mut body,
            true,
        )
        .unwrap();
        let input = body["input"].as_array().unwrap();
        assert_eq!(input[0]["type"], "reasoning");
        assert_eq!(input[0]["summary"][0]["text"], "thinking");
        assert_eq!(input[1]["type"], "message");
    }

    #[test]
    fn openai_responses_round_trip_preserves_role_order_and_multimodal_parts() {
        let original = json!({
            "model": "gpt-4o",
            "messages": [
                { "role": "system", "content": "System A" },
                {
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "Look at this" },
                        { "type": "image_url", "image_url": { "url": "https://example.com/cat.png", "detail": "high" } },
                        { "type": "input_audio", "input_audio": { "data": "AAAA", "format": "wav" } },
                        { "type": "file", "file": { "file_id": "file_123" } }
                    ]
                },
                { "role": "developer", "content": "Developer B" },
                { "role": "user", "content": "Continue" }
            ]
        });
        let mut body = original.clone();
        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        assert!(body.get("instructions").is_none(), "body = {body}");
        let input = body["input"].as_array().expect("responses input");
        assert_eq!(input.len(), 4);
        assert_eq!(input[0]["role"], "system");
        assert_eq!(input[1]["role"], "user");
        assert_eq!(input[1]["content"][0]["type"], "input_text");
        assert_eq!(input[1]["content"][1]["type"], "input_image");
        assert_eq!(input[1]["content"][2]["type"], "input_audio");
        assert_eq!(input[1]["content"][3]["type"], "input_file");
        assert_eq!(input[2]["role"], "developer");
        assert_eq!(input[3]["role"], "user");

        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 4, "messages = {messages:?}");
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[2]["role"], "developer");
        assert_eq!(messages[3]["role"], "user");
        assert_eq!(messages[1]["content"][0]["type"], "text");
        assert_eq!(messages[1]["content"][1]["type"], "image_url");
        assert_eq!(messages[1]["content"][2]["type"], "input_audio");
        assert_eq!(messages[1]["content"][3]["type"], "file");
    }

    #[test]
    fn translate_request_chat_to_responses_maps_user_image_audio_and_file_legally() {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "Describe these inputs" },
                    { "type": "image_url", "image_url": { "url": "https://example.com/cat.png", "detail": "high" } },
                    { "type": "input_audio", "input_audio": { "data": "AAAA", "format": "wav" } },
                    { "type": "file", "file": { "file_id": "file_123" } }
                ]
            }]
        });

        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        let input = body["input"].as_array().expect("responses input");
        assert_eq!(input.len(), 1);
        let content = input[0]["content"].as_array().expect("content");
        assert_eq!(content[0]["type"], "input_text");
        assert_eq!(content[1]["type"], "input_image");
        assert_eq!(content[1]["image_url"], "https://example.com/cat.png");
        assert_eq!(content[1]["detail"], "high");
        assert_eq!(content[2]["type"], "input_audio");
        assert_eq!(content[2]["input_audio"]["data"], "AAAA");
        assert_eq!(content[2]["input_audio"]["format"], "wav");
        assert_eq!(content[3]["type"], "input_file");
        assert_eq!(content[3]["file_id"], "file_123");
    }

    #[test]
    fn translate_request_chat_to_responses_maps_user_image_to_input_image_legally() {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "Describe this image" },
                    { "type": "image_url", "image_url": { "url": "https://example.com/cat.png", "detail": "high" } }
                ]
            }]
        });

        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        let input = body["input"].as_array().expect("responses input");
        let content = input[0]["content"].as_array().expect("content");
        assert_eq!(content[0]["type"], "input_text");
        assert_eq!(content[1]["type"], "input_image");
        assert_eq!(content[1]["image_url"], "https://example.com/cat.png");
        assert_eq!(content[1]["detail"], "high");
    }

    #[test]
    fn translate_request_openai_to_responses_preserves_multiple_instruction_segments_without_merge()
    {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [
                { "role": "system", "content": "System A" },
                { "role": "user", "content": "User 1" },
                { "role": "developer", "content": "Developer B" },
                { "role": "user", "content": "User 2" }
            ]
        });

        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        assert!(body.get("instructions").is_none(), "body = {body}");
        let input = body["input"].as_array().expect("responses input");
        assert_eq!(input.len(), 4, "input = {input:?}");
        assert_eq!(input[0]["role"], "system");
        assert_eq!(input[0]["content"][0]["text"], "System A");
        assert_eq!(input[1]["role"], "user");
        assert_eq!(input[2]["role"], "developer");
        assert_eq!(input[2]["content"][0]["text"], "Developer B");
        assert_eq!(input[3]["role"], "user");
    }

    #[test]
    fn translate_request_same_format_passthrough() {
        let mut body =
            json!({ "model": "gpt-4o", "messages": [{ "role": "user", "content": "Hi" }] });
        let orig = body.clone();
        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            true,
        )
        .unwrap();
        assert_eq!(body, orig);
    }

    #[test]
    fn translate_request_openai_same_format_normalizes_developer_role() {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [
                { "role": "developer", "content": "Follow repo rules." },
                { "role": "user", "content": "Hi" }
            ]
        });
        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            true,
        )
        .unwrap();
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["role"], "user");
    }

    #[test]
    fn translate_request_openai_same_format_coalesces_adjacent_string_messages() {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [
                { "role": "system", "content": "System A" },
                { "role": "developer", "content": "System B" },
                { "role": "user", "content": "User A" },
                { "role": "user", "content": "User B" }
            ]
        });
        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            true,
        )
        .unwrap();
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "System A\n\nSystem B");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "User A\n\nUser B");
    }

    #[test]
    fn translate_request_responses_same_format_normalizes_developer_role() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": [
                { "type": "message", "role": "developer", "content": [{ "type": "input_text", "text": "Follow repo rules." }] },
                { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "Hi" }] }
            ]
        });
        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiResponses,
            "gpt-4o",
            &mut body,
            true,
        )
        .unwrap();
        let input = body["input"].as_array().unwrap();
        assert_eq!(input[0]["role"], "system");
        assert_eq!(input[1]["role"], "user");
    }

    #[test]
    fn translate_request_responses_to_openai() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": [{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "Hello" }] }]
        });
        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            true,
        )
        .unwrap();
        assert!(body.get("messages").is_some());
        assert!(body.get("input").is_none());
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages[0]["content"], "Hello");
    }

    #[test]
    fn translate_request_responses_to_minimax_openai_enables_reasoning_split() {
        let mut body = json!({
            "model": "claude-openai",
            "input": [
                {
                    "type": "reasoning",
                    "summary": [{ "type": "summary_text", "text": "internal thinking" }]
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": "Hello" }]
                }
            ]
        });
        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "MiniMax-M2.7-highspeed",
            &mut body,
            true,
        )
        .unwrap();
        assert_eq!(body["reasoning_split"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(
            messages[0]["reasoning_details"][0]["text"],
            "internal thinking"
        );
    }

    #[test]
    fn translate_request_responses_to_openai_coalesces_adjacent_string_messages() {
        let mut body = json!({
            "model": "gpt-4o",
            "instructions": "System A",
            "input": [
                { "type": "message", "role": "developer", "content": [{ "type": "input_text", "text": "System B" }] },
                { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "User A" }] },
                { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "User B" }] }
            ]
        });
        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            true,
        )
        .unwrap();
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "System A\n\nSystem B");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "User A\n\nUser B");
    }

    #[test]
    fn translate_request_responses_to_openai_flattens_text_only_content_arrays() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": [
                        { "type": "input_text", "text": "Hello " },
                        { "type": "input_text", "text": "world" }
                    ]
                }
            ]
        });
        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            true,
        )
        .unwrap();
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages[0]["content"], "Hello world");
    }

    #[test]
    fn translate_request_responses_to_openai_maps_developer_role_to_system() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": [
                { "type": "message", "role": "developer", "content": [{ "type": "input_text", "text": "Follow repo rules." }] },
                { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "Hello" }] }
            ]
        });
        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            true,
        )
        .unwrap();
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["role"], "user");
    }

    #[test]
    fn translate_request_responses_to_openai_preserves_reasoning_items() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": [
                { "type": "reasoning", "summary": [{ "type": "summary_text", "text": "thinking" }] },
                { "type": "message", "role": "assistant", "content": [{ "type": "output_text", "text": "Hi" }] }
            ]
        });
        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            true,
        )
        .unwrap();
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages[0]["role"], "assistant");
        assert_eq!(messages[0]["reasoning_content"], "thinking");
        assert_eq!(messages[0]["content"], "Hi");
    }

    #[test]
    fn translate_request_responses_to_openai_maps_tool_choice_and_parallel_calls() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": "Hello",
            "tool_choice": { "type": "function", "name": "lookup" },
            "parallel_tool_calls": false
        });
        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            true,
        )
        .unwrap();
        assert_eq!(body["tool_choice"]["type"], "function");
        assert_eq!(body["tool_choice"]["function"]["name"], "lookup");
        assert_eq!(body["parallel_tool_calls"], false);
    }

    #[test]
    fn translate_request_responses_to_openai_drops_responses_only_fields() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": "Hello",
            "stream": true,
            "max_output_tokens": 123,
            "include": ["reasoning.encrypted_content"],
            "text": { "format": { "type": "text" } },
            "reasoning": { "effort": "medium" },
            "store": true,
            "prompt_cache_key": "cache-key",
            "previous_response_id": "resp_123",
            "truncation": "auto"
        });
        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            true,
        )
        .unwrap();
        assert_eq!(body["max_completion_tokens"], 123);
        assert!(body.get("max_output_tokens").is_none());
        assert!(body.get("include").is_none());
        assert!(body.get("text").is_none());
        assert!(body.get("reasoning").is_none());
        assert!(body.get("store").is_none());
        assert!(body.get("prompt_cache_key").is_none());
        assert!(body.get("previous_response_id").is_none());
        assert!(body.get("truncation").is_none());
    }

    #[test]
    fn translate_request_responses_to_openai_preserves_empty_input_and_uses_max_completion_tokens()
    {
        let mut body = json!({
            "model": "gpt-4o",
            "input": "",
            "max_output_tokens": 123
        });
        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "");
        assert_eq!(body["max_completion_tokens"], 123);
        assert!(body.get("max_output_tokens").is_none());
    }

    #[test]
    fn translate_request_responses_to_openai_keeps_empty_input_empty() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": ""
        });

        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "");
    }

    #[test]
    fn translate_request_responses_to_openai_preserves_mid_thread_instruction_segments_in_order() {
        let mut body = json!({
            "model": "MiniMax-M2.7-highspeed",
            "input": [
                { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "Earlier user message" }] },
                { "type": "message", "role": "developer", "content": [{ "type": "input_text", "text": "Compacted thread summary" }] },
                { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "Continue" }] }
            ]
        });
        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "MiniMax-M2.7-highspeed",
            &mut body,
            true,
        )
        .unwrap();
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "Earlier user message");
        assert_eq!(messages[1]["role"], "developer");
        assert_eq!(messages[1]["content"], "Compacted thread summary");
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(messages[2]["content"], "Continue");
    }

    #[test]
    fn translate_response_openai_reasoning_details_maps_to_responses_reasoning() {
        let body = json!({
            "id": "chatcmpl_1",
            "model": "MiniMax-M2.7-highspeed",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Hello",
                    "reasoning_details": [{ "text": "internal thinking" }]
                },
                "finish_reason": "stop"
            }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            &body,
        )
        .unwrap();
        let output = out["output"].as_array().unwrap();
        assert_eq!(output[0]["type"], "reasoning");
        assert_eq!(output[0]["summary"][0]["text"], "internal thinking");
        assert_eq!(output[1]["type"], "message");
        assert_eq!(output[1]["content"][0]["text"], "Hello");
    }

    #[test]
    fn translate_response_openai_tool_only_turn_does_not_emit_empty_responses_message() {
        let body = json!({
            "id": "chatcmpl_tool_only",
            "model": "gpt-4o",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "lookup",
                            "arguments": "{\"city\":\"Tokyo\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            &body,
        )
        .unwrap();
        let output = out["output"].as_array().expect("responses output");
        assert_eq!(output.len(), 1, "output = {output:?}");
        assert_eq!(output[0]["type"], "function_call");
    }

    #[test]
    fn translate_response_openai_tool_only_turn_to_responses_does_not_create_empty_message_item() {
        let body = json!({
            "id": "chatcmpl_tool_only",
            "model": "gpt-4o",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "lookup",
                            "arguments": "{\"city\":\"Tokyo\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });

        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            &body,
        )
        .unwrap();

        let output = out["output"].as_array().expect("responses output");
        assert_eq!(output.len(), 1, "output = {output:?}");
        assert_eq!(output[0]["type"], "function_call");
    }

    #[test]
    fn translate_request_openai_to_claude_has_system_and_messages() {
        let mut body = json!({
            "model": "claude-3",
            "messages": [
                { "role": "system", "content": "Sys" },
                { "role": "user", "content": "Hi" }
            ]
        });
        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Anthropic,
            "claude-3",
            &mut body,
            true,
        )
        .unwrap();
        // System should be array with cache_control on last block
        let system = body
            .get("system")
            .and_then(Value::as_array)
            .expect("system should be array");
        assert!(!system.is_empty());
        assert_eq!(system[0]["text"], "Sys");
        assert!(body.get("messages").is_some());
        assert!(!body["messages"].as_array().unwrap().is_empty());
    }

    #[test]
    fn translate_request_openai_to_claude_does_not_emit_invalid_thinking_blocks_or_cache_control() {
        let mut body = json!({
            "model": "claude-3",
            "messages": [
                {
                    "role": "assistant",
                    "reasoning_content": "private reasoning",
                    "content": "Visible answer"
                }
            ]
        });
        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Anthropic,
            "claude-3",
            &mut body,
            false,
        )
        .unwrap();

        let blocks = body["messages"][0]["content"]
            .as_array()
            .expect("assistant blocks");
        assert!(blocks.iter().all(|block| block["type"] != "thinking"));
        assert!(blocks
            .iter()
            .all(|block| block.get("cache_control").is_none()));
    }

    #[test]
    fn translate_request_openai_to_claude_maps_tool_choice_and_parallel_calls() {
        let mut body = json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "tool_choice": { "type": "function", "function": { "name": "lookup" } },
            "parallel_tool_calls": false
        });
        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Anthropic,
            "claude-3",
            &mut body,
            true,
        )
        .unwrap();
        assert_eq!(body["tool_choice"]["type"], "tool");
        assert_eq!(body["tool_choice"]["name"], "lookup");
        assert_eq!(body["tool_choice"]["disable_parallel_tool_use"], true);
    }

    #[test]
    fn translate_request_openai_to_claude_preserves_top_p_stop_and_metadata() {
        let mut body = json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "top_p": 0.7,
            "stop": ["END"],
            "metadata": { "trace_id": "abc" }
        });
        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Anthropic,
            "claude-3",
            &mut body,
            true,
        )
        .unwrap();
        assert_eq!(body["top_p"], 0.7);
        assert_eq!(body["stop_sequences"][0], "END");
        assert_eq!(body["metadata"]["trace_id"], "abc");
    }

    #[test]
    fn translate_request_openai_to_gemini_has_contents() {
        let mut body = json!({
            "model": "gemini-1.5",
            "messages": [
                { "role": "system", "content": "Helper" },
                { "role": "user", "content": "Hi" }
            ]
        });
        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-1.5",
            &mut body,
            true,
        )
        .unwrap();
        assert!(body.get("contents").is_some());
        assert!(body.get("systemInstruction").is_some());
    }

    #[test]
    fn translate_request_openai_to_gemini_preserves_function_response_name() {
        let mut body = json!({
            "model": "gemini-1.5",
            "messages": [
                { "role": "user", "content": "Hi" },
                {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "lookup_weather", "arguments": "{\"city\":\"Tokyo\"}" }
                    }]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_1",
                    "content": "{\"temperature\":22}"
                }
            ]
        });
        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-1.5",
            &mut body,
            false,
        )
        .unwrap();
        assert_eq!(
            body["contents"][2]["parts"][0]["functionResponse"]["id"],
            "call_1"
        );
        assert_eq!(
            body["contents"][2]["parts"][0]["functionResponse"]["name"],
            "lookup_weather"
        );
    }

    #[test]
    fn translate_request_openai_to_gemini_merges_parallel_function_responses_in_original_call_order(
    ) {
        let mut body = json!({
            "model": "gemini-1.5",
            "messages": [
                { "role": "user", "content": "Hi" },
                {
                    "role": "assistant",
                    "tool_calls": [
                        {
                            "id": "call_1",
                            "type": "function",
                            "function": { "name": "first", "arguments": "{\"step\":1}" }
                        },
                        {
                            "id": "call_2",
                            "type": "function",
                            "function": { "name": "second", "arguments": "{\"step\":2}" }
                        }
                    ]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_2",
                    "content": "{\"ok\":2}"
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_1",
                    "content": "{\"ok\":1}"
                }
            ]
        });
        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-1.5",
            &mut body,
            false,
        )
        .unwrap();

        let contents = body["contents"].as_array().expect("gemini contents");
        assert_eq!(contents.len(), 3, "contents = {contents:?}");
        let responses = contents[2]["parts"].as_array().expect("function responses");
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["functionResponse"]["id"], "call_1");
        assert_eq!(responses[1]["functionResponse"]["id"], "call_2");
    }

    #[test]
    fn translate_request_openai_to_gemini_maps_tool_choice_and_allowlist() {
        let mut body = json!({
            "model": "gemini-1.5",
            "messages": [{ "role": "user", "content": "Hi" }],
            "tool_choice": "required",
            "allowed_tool_names": ["lookup_weather", "lookup_time"]
        });
        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-1.5",
            &mut body,
            false,
        )
        .unwrap();

        assert_eq!(body["toolConfig"]["functionCallingConfig"]["mode"], "ANY");
        assert_eq!(
            body["toolConfig"]["functionCallingConfig"]["allowedFunctionNames"][0],
            "lookup_weather"
        );
        assert_eq!(
            body["toolConfig"]["functionCallingConfig"]["allowedFunctionNames"][1],
            "lookup_time"
        );
    }

    #[test]
    fn translate_request_openai_to_gemini_maps_tool_choice_modes() {
        let cases = [
            (json!("auto"), json!({ "mode": "AUTO" })),
            (json!("none"), json!({ "mode": "NONE" })),
            (json!("required"), json!({ "mode": "ANY" })),
            (
                json!({ "type": "function", "function": { "name": "lookup_weather" } }),
                json!({
                    "mode": "ANY",
                    "allowedFunctionNames": ["lookup_weather"]
                }),
            ),
        ];

        for (tool_choice, expected) in cases {
            let mut body = json!({
                "model": "gemini-1.5",
                "messages": [{ "role": "user", "content": "Hi" }],
                "tool_choice": tool_choice
            });
            translate_request(
                UpstreamFormat::OpenAiCompletion,
                UpstreamFormat::Google,
                "gemini-1.5",
                &mut body,
                false,
            )
            .unwrap();

            assert_eq!(body["toolConfig"]["functionCallingConfig"], expected);
        }
    }

    #[test]
    fn translate_request_openai_to_gemini_does_not_attach_allowlist_for_auto_or_none() {
        let cases = [("auto", "AUTO"), ("none", "NONE")];

        for (tool_choice, expected_mode) in cases {
            let mut body = json!({
                "model": "gemini-1.5",
                "messages": [{ "role": "user", "content": "Hi" }],
                "tool_choice": tool_choice,
                "allowed_tool_names": ["lookup_weather", "lookup_time"]
            });
            translate_request(
                UpstreamFormat::OpenAiCompletion,
                UpstreamFormat::Google,
                "gemini-1.5",
                &mut body,
                false,
            )
            .unwrap();

            let config = &body["toolConfig"]["functionCallingConfig"];
            assert_eq!(config["mode"], expected_mode);
            assert!(
                config.get("allowedFunctionNames").is_none(),
                "config = {config:?}"
            );
        }
    }

    #[test]
    fn translate_request_openai_to_gemini_tool_turns_use_dummy_signature_without_reasoning_replay()
    {
        let mut body = json!({
            "model": "gemini-1.5",
            "messages": [
                { "role": "user", "content": "Hi" },
                {
                    "role": "assistant",
                    "reasoning_content": "internal reasoning",
                    "content": "Calling tool.",
                    "tool_calls": [
                        {
                            "id": "call_1",
                            "type": "function",
                            "function": { "name": "lookup_weather", "arguments": "{\"city\":\"Tokyo\"}" }
                        },
                        {
                            "id": "call_2",
                            "type": "function",
                            "function": { "name": "lookup_time", "arguments": "{\"city\":\"Tokyo\"}" }
                        }
                    ]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_1",
                    "content": "{\"temperature\":22}"
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_2",
                    "content": "{\"time\":\"10:00\"}"
                }
            ]
        });
        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-1.5",
            &mut body,
            false,
        )
        .unwrap();
        let assistant_parts = body["contents"][1]["parts"]
            .as_array()
            .expect("assistant parts");
        assert!(assistant_parts.iter().all(|part| part["thought"] != true));
        assert_eq!(assistant_parts[0]["text"], "Calling tool.");
        assert!(assistant_parts[1].get("functionCall").is_some());
        assert_eq!(
            assistant_parts[1]["thoughtSignature"],
            "skip_thought_signature_validator"
        );
        assert!(assistant_parts[2].get("functionCall").is_some());
        assert!(assistant_parts[2].get("thoughtSignature").is_none());
    }

    #[test]
    fn translate_request_openai_to_claude_omitted_stream_defaults_false() {
        let mut body = json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }]
        });
        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Anthropic,
            "claude-3",
            &mut body,
            false,
        )
        .unwrap();
        assert_eq!(body["stream"], false);
    }

    #[test]
    fn translate_request_openai_to_responses_maps_tool_choice() {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "Hi" }],
            "tool_choice": { "type": "function", "function": { "name": "lookup" } },
            "parallel_tool_calls": false
        });
        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            "gpt-4o",
            &mut body,
            true,
        )
        .unwrap();
        assert_eq!(body["tool_choice"]["type"], "function");
        assert_eq!(body["tool_choice"]["name"], "lookup");
        assert_eq!(body["parallel_tool_calls"], false);
    }

    #[test]
    fn translate_request_openai_to_responses_maps_max_completion_tokens_to_max_output_tokens() {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "Hi" }],
            "max_completion_tokens": 222
        });

        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        assert_eq!(body["max_output_tokens"], 222);
        assert!(body.get("max_completion_tokens").is_none());
    }

    #[test]
    fn translate_request_responses_to_chat_maps_max_output_tokens_to_max_completion_tokens() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": "Hello",
            "max_output_tokens": 321
        });

        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        assert_eq!(body["max_completion_tokens"], 321);
        assert!(body.get("max_output_tokens").is_none());
    }

    #[test]
    fn translate_request_gemini_to_openai_maps_tool_policies() {
        let mut body = json!({
            "model": "gemini-1.5",
            "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
            "toolConfig": {
                "functionCallingConfig": {
                    "mode": "ANY",
                    "allowedFunctionNames": ["lookup_weather", "lookup_time"]
                }
            }
        });
        translate_request(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiCompletion,
            "gemini-1.5",
            &mut body,
            false,
        )
        .unwrap();

        assert_eq!(body["tool_choice"], "required");
        assert_eq!(body["allowed_tool_names"][0], "lookup_weather");
        assert_eq!(body["allowed_tool_names"][1], "lookup_time");
    }

    #[test]
    fn translate_request_responses_to_claude_keeps_tool_use_and_result_adjacent() {
        let mut body = json!({
            "model": "codex-anthropic",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": "Run pwd" }]
                },
                {
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "exec_command",
                    "arguments": "{\"cmd\":\"pwd\"}"
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_1",
                    "output": "/home/percy/temp"
                }
            ],
            "stream": true
        });
        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Anthropic,
            "codex-anthropic",
            &mut body,
            true,
        )
        .unwrap();

        let messages = body["messages"].as_array().expect("messages array");
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[1]["role"], "assistant");
        let assistant_blocks = messages[1]["content"]
            .as_array()
            .expect("assistant content");
        assert_eq!(assistant_blocks.len(), 1);
        assert_eq!(assistant_blocks[0]["type"], "tool_use");
        assert_eq!(assistant_blocks[0]["id"], "call_1");
        assert_eq!(assistant_blocks[0]["input"]["cmd"], "pwd");

        assert_eq!(messages[2]["role"], "user");
        let user_blocks = messages[2]["content"].as_array().expect("user content");
        assert_eq!(user_blocks.len(), 1);
        assert_eq!(user_blocks[0]["type"], "tool_result");
        assert_eq!(user_blocks[0]["tool_use_id"], "call_1");
        assert_eq!(user_blocks[0]["content"], "/home/percy/temp");
    }

    #[test]
    fn translate_request_responses_to_claude_merges_assistant_text_with_tool_use() {
        let mut body = json!({
            "model": "codex-anthropic",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": "Run pwd" }]
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": "Let me check." }]
                },
                {
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "exec_command",
                    "arguments": "{\"cmd\":\"pwd\"}"
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_1",
                    "output": "/home/percy/temp"
                }
            ],
            "stream": true
        });
        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Anthropic,
            "codex-anthropic",
            &mut body,
            true,
        )
        .unwrap();

        let messages = body["messages"].as_array().expect("messages array");
        assert_eq!(messages.len(), 3);
        let assistant_blocks = messages[1]["content"]
            .as_array()
            .expect("assistant content");
        assert_eq!(assistant_blocks[0]["type"], "text");
        assert_eq!(assistant_blocks[0]["text"], "Let me check.");
        assert_eq!(assistant_blocks[1]["type"], "tool_use");
        assert_eq!(assistant_blocks[1]["id"], "call_1");
        let user_blocks = messages[2]["content"].as_array().expect("user content");
        assert_eq!(user_blocks[0]["type"], "tool_result");
        assert_eq!(user_blocks[0]["tool_use_id"], "call_1");
    }

    #[test]
    fn translate_request_responses_to_claude_moves_user_warning_after_tool_result() {
        let mut body = json!({
            "model": "codex-anthropic",
            "input": [
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": "Running test..." }]
                },
                {
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "exec_command",
                    "arguments": "{\"cmd\":\"python test.py\"}"
                },
                {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": "Warning: process limit reached" }]
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_1",
                    "output": "Done"
                }
            ],
            "stream": true
        });
        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Anthropic,
            "codex-anthropic",
            &mut body,
            true,
        )
        .unwrap();

        let messages = body["messages"].as_array().expect("messages array");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1]["role"], "user");
        let user_blocks = messages[1]["content"].as_array().expect("user content");
        assert_eq!(user_blocks[0]["type"], "tool_result");
        assert_eq!(user_blocks[0]["tool_use_id"], "call_1");
        assert_eq!(user_blocks[1]["type"], "text");
        assert_eq!(user_blocks[1]["text"], "Warning: process limit reached");
    }

    #[test]
    fn translate_request_claude_to_openai_omitted_stream_defaults_false() {
        let mut body = json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }]
        });
        translate_request(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiCompletion,
            "claude-3",
            &mut body,
            false,
        )
        .unwrap();
        assert_eq!(body["stream"], false);
    }

    #[test]
    fn translate_request_claude_to_openai_collapses_text_blocks_to_string() {
        let mut body = json!({
            "model": "claude-3",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "alpha\n" },
                    { "type": "text", "text": "beta" }
                ]
            }]
        });
        translate_request(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiCompletion,
            "claude-3",
            &mut body,
            false,
        )
        .unwrap();
        assert_eq!(body["messages"][0]["content"], "alpha\nbeta");
    }

    #[test]
    fn translate_request_claude_to_openai_drops_metadata_for_compatibility() {
        let mut body = json!({
            "model": "claude-3",
            "metadata": { "user_id": "abc" },
            "messages": [{ "role": "user", "content": "Hi" }]
        });
        translate_request(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiCompletion,
            "claude-3",
            &mut body,
            false,
        )
        .unwrap();
        assert!(body.get("metadata").is_none());
    }

    #[test]
    fn translate_request_gemini_to_openai_omitted_stream_defaults_false() {
        let mut body = json!({
            "contents": [{ "parts": [{ "text": "Hi" }] }]
        });
        translate_request(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiCompletion,
            "gemini-1.5",
            &mut body,
            false,
        )
        .unwrap();
        assert_eq!(body["stream"], false);
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], "Hi");
    }

    #[test]
    fn translate_request_gemini_to_openai_missing_role_preserves_text() {
        let mut body = json!({
            "model": "gemini-1.5",
            "contents": [{ "parts": [{ "text": "Reply with exactly: ok" }] }]
        });
        translate_request(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiCompletion,
            "gemini-1.5",
            &mut body,
            true,
        )
        .unwrap();
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], "Reply with exactly: ok");
    }

    #[test]
    fn translate_request_gemini_to_openai_accepts_snake_case_parts() {
        let mut body = json!({
            "model": "gemini-1.5",
            "contents": [{
                "parts": [
                    {
                        "inline_data": {
                            "mime_type": "image/jpeg",
                            "data": "abc123"
                        }
                    },
                    {
                        "function_call": {
                            "id": "call_1",
                            "name": "lookup_weather",
                            "args": { "city": "Tokyo" }
                        }
                    }
                ]
            }]
        });
        translate_request(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiCompletion,
            "gemini-1.5",
            &mut body,
            false,
        )
        .unwrap();
        assert_eq!(body["messages"][0]["role"], "assistant");
        assert_eq!(body["messages"][0]["tool_calls"][0]["id"], "call_1");
        assert_eq!(
            body["messages"][0]["tool_calls"][0]["function"]["name"],
            "lookup_weather"
        );
        assert!(body["messages"][0]["content"]
            .as_array()
            .expect("array content")
            .iter()
            .any(|part| part["type"] == "image_url"));
    }

    #[test]
    fn translate_request_gemini_to_openai_preserves_text_and_function_response_order() {
        let mut body = json!({
            "model": "gemini-1.5",
            "contents": [{
                "role": "user",
                "parts": [
                    { "text": "Before tool." },
                    {
                        "functionResponse": {
                            "id": "call_1",
                            "name": "lookup_weather",
                            "response": { "result": { "temperature": 22 } }
                        }
                    },
                    { "text": "After tool." }
                ]
            }]
        });
        translate_request(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiCompletion,
            "gemini-1.5",
            &mut body,
            false,
        )
        .unwrap();
        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "Before tool.");
        assert_eq!(messages[1]["role"], "tool");
        assert_eq!(messages[1]["tool_call_id"], "call_1");
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(messages[2]["content"], "After tool.");
    }

    #[test]
    fn translate_response_same_format_passthrough() {
        let body = json!({
            "id": "x",
            "choices": [{ "message": { "role": "assistant", "content": "Hi" }, "finish_reason": "stop" }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .unwrap();
        assert_eq!(out, body);
    }

    #[test]
    fn translate_response_claude_to_openai_has_choices() {
        let body = json!({
            "id": "msg_1",
            "content": [{ "type": "text", "text": "Hello back" }],
            "stop_reason": "end_turn",
            "model": "claude-3",
            "usage": { "input_tokens": 10, "output_tokens": 5 }
        });
        let out = translate_response(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .unwrap();
        assert!(out.get("choices").is_some());
        assert_eq!(out["choices"][0]["message"]["content"], "Hello back");
        assert_eq!(out["usage"]["prompt_tokens"], 10);
    }

    #[test]
    fn translate_response_claude_context_window_stop_maps_to_openai_error_reason() {
        let body = json!({
            "id": "msg_1",
            "content": [{ "type": "text", "text": "" }],
            "stop_reason": "model_context_window_exceeded",
            "model": "claude-3"
        });
        let out = translate_response(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .unwrap();
        assert_eq!(
            out["choices"][0]["finish_reason"],
            "context_length_exceeded"
        );
    }

    #[test]
    fn translate_response_claude_refusal_maps_to_content_filter() {
        let body = json!({
            "id": "msg_1",
            "content": [{ "type": "text", "text": "I can't help with that." }],
            "stop_reason": "refusal",
            "model": "claude-3"
        });
        let out = translate_response(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .unwrap();
        assert_eq!(out["choices"][0]["finish_reason"], "content_filter");
    }

    #[test]
    fn translate_response_claude_pause_turn_maps_to_pause_turn_finish() {
        let body = json!({
            "id": "msg_1",
            "content": [{ "type": "text", "text": "" }],
            "stop_reason": "pause_turn",
            "model": "claude-3"
        });
        let out = translate_response(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .unwrap();
        assert_eq!(out["choices"][0]["finish_reason"], "pause_turn");
    }

    #[test]
    fn translate_response_openai_to_claude_has_content_array() {
        let body = json!({
            "id": "chatcmpl-1",
            "choices": [{ "message": { "role": "assistant", "content": "Hi" }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 2 }
        });
        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Anthropic,
            &body,
        )
        .unwrap();
        assert!(out.get("content").is_some());
        assert!(out["content"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b.get("type").and_then(Value::as_str) == Some("text")));
    }

    #[test]
    fn translate_response_openai_error_finishes_to_claude_stop_reasons() {
        let body = json!({
            "id": "chatcmpl-1",
            "choices": [{ "message": { "role": "assistant", "content": "" }, "finish_reason": "context_length_exceeded" }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Anthropic,
            &body,
        )
        .unwrap();
        assert_eq!(out["stop_reason"], "model_context_window_exceeded");
    }

    #[test]
    fn translate_response_openai_error_finish_to_claude_error_body() {
        let body = json!({
            "id": "chatcmpl-1",
            "choices": [{ "message": { "role": "assistant", "content": "" }, "finish_reason": "error" }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Anthropic,
            &body,
        )
        .unwrap();
        assert_eq!(out["type"], "error");
        assert_eq!(out["error"]["type"], "api_error");
        assert!(out.get("stop_reason").is_none());
    }

    #[test]
    fn translate_response_openai_tool_error_finish_to_claude_error_body() {
        let body = json!({
            "id": "chatcmpl-1",
            "choices": [{ "message": { "role": "assistant", "content": "" }, "finish_reason": "tool_error" }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Anthropic,
            &body,
        )
        .unwrap();
        assert_eq!(out["type"], "error");
        assert_eq!(out["error"]["type"], "invalid_request_error");
        assert!(out.get("stop_reason").is_none());
    }

    #[test]
    fn translate_response_openai_pause_turn_to_claude_stop_reason() {
        let body = json!({
            "id": "chatcmpl-1",
            "choices": [{ "message": { "role": "assistant", "content": "" }, "finish_reason": "pause_turn" }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Anthropic,
            &body,
        )
        .unwrap();
        assert_eq!(out["stop_reason"], "pause_turn");
    }

    #[test]
    fn translate_response_openai_to_responses_maps_pause_turn_to_incomplete() {
        let body = json!({
            "id": "chatcmpl_1",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "" },
                "finish_reason": "pause_turn"
            }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            &body,
        )
        .unwrap();
        assert_eq!(out["status"], "incomplete");
        assert_eq!(out["incomplete_details"]["reason"], "pause_turn");
    }

    #[test]
    fn translate_response_responses_to_openai_maps_usage_fields() {
        let body = json!({
            "id": "resp_1",
            "object": "response",
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": "Hi" }]
            }],
            "usage": {
                "input_tokens": 11,
                "output_tokens": 7,
                "total_tokens": 18,
                "input_tokens_details": { "cached_tokens": 3 },
                "output_tokens_details": { "reasoning_tokens": 2 }
            }
        });
        let out = translate_response(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .unwrap();
        assert_eq!(out["usage"]["prompt_tokens"], 11);
        assert_eq!(out["usage"]["completion_tokens"], 7);
        assert_eq!(out["usage"]["prompt_tokens_details"]["cached_tokens"], 3);
        assert_eq!(
            out["usage"]["completion_tokens_details"]["reasoning_tokens"],
            2
        );
    }

    #[test]
    fn translate_response_responses_incomplete_to_openai_preserves_terminal_and_usage_details() {
        let body = json!({
            "id": "resp_1",
            "object": "response",
            "created_at": 42,
            "status": "incomplete",
            "incomplete_details": { "reason": "max_output_tokens" },
            "output": [{
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": "Hi" }]
            }],
            "usage": {
                "input_tokens": 11,
                "output_tokens": 7,
                "total_tokens": 18,
                "input_tokens_details": { "cached_tokens": 3 },
                "output_tokens_details": { "reasoning_tokens": 2 }
            }
        });
        let out = translate_response(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .unwrap();
        assert_eq!(out["choices"][0]["message"]["content"], "Hi");
        assert_eq!(out["choices"][0]["finish_reason"], "length");
        assert_eq!(out["usage"]["prompt_tokens_details"]["cached_tokens"], 3);
        assert_eq!(
            out["usage"]["completion_tokens_details"]["reasoning_tokens"],
            2
        );
    }

    #[test]
    fn translate_response_responses_failed_to_openai_maps_context_failure() {
        let body = json!({
            "id": "resp_1",
            "object": "response",
            "created_at": 42,
            "status": "failed",
            "error": { "code": "context_length_exceeded" },
            "output": [{
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": "" }]
            }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .unwrap();
        assert_eq!(
            out["choices"][0]["finish_reason"],
            "context_length_exceeded"
        );
    }

    #[test]
    fn translate_response_responses_failed_unknown_code_maps_to_error() {
        let body = json!({
            "id": "resp_1",
            "object": "response",
            "created_at": 42,
            "status": "failed",
            "error": { "code": "server_error" },
            "output": [{
                "id": "fc_1",
                "type": "function_call",
                "call_id": "call_1",
                "name": "lookup_weather",
                "arguments": "{\"city\":\"Tokyo\"}"
            }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .unwrap();
        assert_eq!(out["choices"][0]["finish_reason"], "error");
        assert!(out["choices"][0]["message"]["tool_calls"].is_array());
    }

    #[test]
    fn translate_response_responses_failed_tool_validation_maps_to_tool_error() {
        let body = json!({
            "id": "resp_1",
            "object": "response",
            "created_at": 42,
            "status": "failed",
            "error": { "code": "tool_validation_error" },
            "output": [{
                "id": "fc_1",
                "type": "function_call",
                "call_id": "call_1",
                "name": "lookup_weather",
                "arguments": "{\"city\":\"Tokyo\"}"
            }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .unwrap();
        assert_eq!(out["choices"][0]["finish_reason"], "tool_error");
        assert!(out["choices"][0]["message"]["tool_calls"].is_array());
    }

    #[test]
    fn translate_response_openai_to_responses_maps_usage_fields() {
        let body = json!({
            "id": "chatcmpl_1",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "Hi" },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 11,
                "completion_tokens": 7,
                "total_tokens": 18,
                "prompt_tokens_details": { "cached_tokens": 3 },
                "completion_tokens_details": { "reasoning_tokens": 2 }
            }
        });
        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            &body,
        )
        .unwrap();
        assert_eq!(out["usage"]["input_tokens"], 11);
        assert_eq!(out["usage"]["output_tokens"], 7);
        assert_eq!(out["usage"]["input_tokens_details"]["cached_tokens"], 3);
        assert_eq!(out["usage"]["output_tokens_details"]["reasoning_tokens"], 2);
    }

    #[test]
    fn translate_response_openai_to_claude_restores_server_tool_use_from_marker() {
        let body = json!({
            "id": "chatcmpl_server_tool",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "server_1",
                        "type": "function",
                        "proxied_tool_kind": "anthropic_server_tool_use",
                        "function": {
                            "name": "web_search",
                            "arguments": "{\"query\":\"rust\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });

        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Anthropic,
            &body,
        )
        .unwrap();

        let content = out["content"].as_array().expect("anthropic content");
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "server_tool_use");
        assert_eq!(content[0]["name"], "web_search");
    }

    #[test]
    fn translate_response_openai_to_responses_preserves_reasoning_output() {
        let body = json!({
            "id": "chatcmpl_1",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "reasoning_content": "thinking",
                    "content": "Hi"
                },
                "finish_reason": "stop"
            }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            &body,
        )
        .unwrap();
        assert_eq!(out["output"][0]["type"], "reasoning");
        assert_eq!(out["output"][0]["summary"][0]["text"], "thinking");
        assert_eq!(out["output"][1]["type"], "message");
    }

    #[test]
    fn translate_response_openai_to_responses_maps_length_to_incomplete() {
        let body = json!({
            "id": "chatcmpl_1",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "Hi" },
                "finish_reason": "length"
            }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            &body,
        )
        .unwrap();
        assert_eq!(out["status"], "incomplete");
        assert_eq!(out["incomplete_details"]["reason"], "max_output_tokens");
    }

    #[test]
    fn translate_response_openai_to_responses_maps_content_filter_to_incomplete() {
        let body = json!({
            "id": "chatcmpl_1",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "Hi" },
                "finish_reason": "content_filter"
            }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            &body,
        )
        .unwrap();
        assert_eq!(out["status"], "incomplete");
        assert_eq!(out["incomplete_details"]["reason"], "content_filter");
    }

    #[test]
    fn translate_response_openai_to_responses_maps_context_window_to_failed() {
        let body = json!({
            "id": "chatcmpl_1",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "" },
                "finish_reason": "context_length_exceeded"
            }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            &body,
        )
        .unwrap();
        assert_eq!(out["status"], "failed");
        assert_eq!(out["error"]["code"], "context_length_exceeded");
        assert_eq!(out["incomplete_details"], serde_json::Value::Null);
    }

    #[test]
    fn translate_response_openai_to_gemini_maps_usage_fields() {
        let body = json!({
            "id": "chatcmpl_1",
            "object": "chat.completion",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "Hi" },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 11,
                "completion_tokens": 7,
                "total_tokens": 18,
                "prompt_tokens_details": { "cached_tokens": 3 },
                "completion_tokens_details": { "reasoning_tokens": 2 }
            }
        });
        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            &body,
        )
        .unwrap();
        assert_eq!(out["usageMetadata"]["promptTokenCount"], 11);
        assert_eq!(out["usageMetadata"]["candidatesTokenCount"], 5);
        assert_eq!(out["usageMetadata"]["thoughtsTokenCount"], 2);
        assert_eq!(out["usageMetadata"]["cachedContentTokenCount"], 3);
    }

    #[test]
    fn translate_response_openai_to_gemini_tool_calls_include_dummy_signature() {
        let body = json!({
            "id": "chatcmpl_gem_fc",
            "object": "chat.completion",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "reasoning_content": "Need tools.",
                    "content": "Calling tools.",
                    "tool_calls": [
                        {
                            "id": "call_1",
                            "type": "function",
                            "function": { "name": "lookup_weather", "arguments": "{\"city\":\"Tokyo\"}" }
                        },
                        {
                            "id": "call_2",
                            "type": "function",
                            "function": { "name": "lookup_time", "arguments": "{\"city\":\"Tokyo\"}" }
                        }
                    ]
                },
                "finish_reason": "tool_calls"
            }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            &body,
        )
        .unwrap();
        let parts = out["candidates"][0]["content"]["parts"]
            .as_array()
            .expect("gemini parts");
        let function_parts = parts
            .iter()
            .filter(|part| part.get("functionCall").is_some())
            .collect::<Vec<_>>();
        assert_eq!(
            function_parts[0]["thoughtSignature"],
            "skip_thought_signature_validator"
        );
        assert!(function_parts[1].get("thoughtSignature").is_none());
    }

    #[test]
    fn translate_response_gemini_to_openai_maps_finish_and_reasoning_usage_details() {
        let body = json!({
            "response": {
                "responseId": "gem_resp_1",
                "modelVersion": "gemini-2.5",
                "candidates": [{
                    "content": {
                        "role": "model",
                        "parts": [{ "text": "Hi" }]
                    },
                    "finishReason": "MAX_TOKENS"
                }],
                "usageMetadata": {
                    "promptTokenCount": 11,
                    "candidatesTokenCount": 5,
                    "thoughtsTokenCount": 2,
                    "totalTokenCount": 18,
                    "cachedContentTokenCount": 3
                }
            }
        });
        let out = translate_response(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .unwrap();
        assert_eq!(out["id"], "gem_resp_1");
        assert_eq!(out["model"], "gemini-2.5");
        assert_eq!(out["choices"][0]["finish_reason"], "length");
        assert_eq!(out["usage"]["prompt_tokens"], 11);
        assert_eq!(out["usage"]["completion_tokens"], 7);
        assert_eq!(out["usage"]["prompt_tokens_details"]["cached_tokens"], 3);
        assert_eq!(
            out["usage"]["completion_tokens_details"]["reasoning_tokens"],
            2
        );
    }

    #[test]
    fn translate_response_gemini_prompt_feedback_without_candidates_is_not_an_error() {
        let body = json!({
            "promptFeedback": {
                "blockReason": "SAFETY"
            },
            "usageMetadata": {
                "promptTokenCount": 3,
                "totalTokenCount": 3
            },
            "modelVersion": "gemini-2.5"
        });
        let out = translate_response(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .expect("translated response");

        assert_eq!(out["object"], "chat.completion");
        assert_eq!(out["choices"][0]["finish_reason"], "content_filter");
        assert_eq!(out["usage"]["prompt_tokens"], 3);
    }

    #[test]
    fn translate_response_gemini_non_success_finish_reasons_do_not_collapse_to_success() {
        let cases = [
            ("MALFORMED_FUNCTION_CALL", "tool_error"),
            ("UNEXPECTED_TOOL_CALL", "tool_error"),
            ("TOO_MANY_TOOL_CALLS", "tool_error"),
            ("MISSING_THOUGHT_SIGNATURE", "tool_error"),
            ("IMAGE_OTHER", "error"),
            ("NO_IMAGE", "error"),
            ("LANGUAGE", "error"),
        ];

        for (reason, expected) in cases {
            let body = json!({
                "response": {
                    "responseId": format!("gem_{reason}"),
                    "modelVersion": "gemini-2.5",
                    "candidates": [{
                        "content": {
                            "role": "model",
                            "parts": [{
                                "functionCall": {
                                    "id": "call_1",
                                    "name": "lookup_weather",
                                    "args": { "city": "Tokyo" }
                                }
                            }]
                        },
                        "finishReason": reason
                    }]
                }
            });
            let out = translate_response(
                UpstreamFormat::Google,
                UpstreamFormat::OpenAiCompletion,
                &body,
            )
            .unwrap();
            assert_eq!(
                out["choices"][0]["finish_reason"], expected,
                "reason = {reason}, body = {out:?}"
            );
        }
    }

    #[test]
    fn translate_response_openai_to_responses_maps_error_finish_to_failed() {
        let body = json!({
            "id": "chatcmpl_1",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "" },
                "finish_reason": "error"
            }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            &body,
        )
        .unwrap();
        assert_eq!(out["status"], "failed");
        assert_eq!(out["error"]["code"], "error");
    }

    #[test]
    fn translate_response_openai_to_responses_maps_tool_error_finish_to_failed() {
        let body = json!({
            "id": "chatcmpl_1",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "" },
                "finish_reason": "tool_error"
            }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            &body,
        )
        .unwrap();
        assert_eq!(out["status"], "failed");
        assert_eq!(out["error"]["code"], "tool_error");
    }

    #[test]
    fn translate_response_openai_to_gemini_maps_portable_finish_reasons() {
        let body = json!({
            "id": "chatcmpl_1",
            "object": "chat.completion",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "Hi" },
                "finish_reason": "content_filter"
            }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            &body,
        )
        .unwrap();
        assert_eq!(out["responseId"], "chatcmpl_1");
        assert_eq!(out["modelVersion"], "gpt-4o");
        assert_eq!(out["candidates"][0]["finishReason"], "SAFETY");
    }
}
