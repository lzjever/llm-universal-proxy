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
        UpstreamFormat::OpenAiCompletion => Ok(body.clone()),
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

fn gemini_response_to_openai(body: &Value) -> Result<Value, String> {
    let response = body.get("response").unwrap_or(body);
    let candidates = response
        .get("candidates")
        .and_then(Value::as_array)
        .ok_or("missing candidates")?;
    let candidate = candidates.first().ok_or("empty candidates")?;
    let content = candidate.get("content");
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
    let mut finish_reason = candidate
        .get("finishReason")
        .and_then(Value::as_str)
        .unwrap_or("stop")
        .to_lowercase();
    if finish_reason == "stop" && has_tool_calls {
        finish_reason = "tool_calls".to_string();
    }
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

        result["usage"] = usage_json;
    }
    Ok(result)
}

fn openai_response_to_claude(body: &Value) -> Result<Value, String> {
    let choices = body
        .get("choices")
        .and_then(Value::as_array)
        .ok_or("missing choices")?;
    let choice = choices.first().ok_or("empty choices")?;
    let message = choice.get("message").ok_or("missing message")?;
    let mut content: Vec<Value> = vec![];
    if let Some(rc) = message.get("reasoning_content").and_then(Value::as_str) {
        if !rc.is_empty() {
            content.push(serde_json::json!({ "type": "thinking", "thinking": rc }));
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
                "type": "tool_use",
                "id": t.get("id"),
                "name": t.get("function").and_then(|f| f.get("name")),
                "input": input
            }));
        }
    }
    let mut finish = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .unwrap_or("stop")
        .to_string();
    if finish == "tool_calls" {
        finish = "tool_use".to_string();
    } else if finish == "length" {
        finish = "max_tokens".to_string();
    } else if finish == "content_filter" {
        finish = "refusal".to_string();
    } else if finish == "context_length_exceeded" {
        finish = "model_context_window_exceeded".to_string();
    } else if finish == "pause_turn" {
        finish = "pause_turn".to_string();
    }
    let mut result = serde_json::json!({
        "id": body.get("id").cloned().unwrap_or(serde_json::Value::Null),
        "type": "message",
        "role": "assistant",
        "content": content,
        "model": body.get("model").cloned().unwrap_or(serde_json::Value::Null),
        "stop_reason": finish
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
    if let Some(rc) = message.get("reasoning_content").and_then(Value::as_str) {
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
        for t in tc {
            let args = t
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(Value::as_str);
            let args_val = args
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or(serde_json::json!({}));
            parts.push(serde_json::json!({
                "functionCall": {
                    "id": t.get("id"),
                    "name": t.get("function").and_then(|f| f.get("name")),
                    "args": args_val
                }
            }));
        }
    }
    if parts.is_empty() {
        parts.push(serde_json::json!({ "text": "" }));
    }
    let finish = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .unwrap_or("stop");
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
    }
    let mut message = serde_json::json!({ "role": "assistant" });
    message["content"] = Value::String(content);
    let has_tool_calls = !tool_calls.is_empty();
    if has_tool_calls {
        message["tool_calls"] = Value::Array(tool_calls);
    }
    let finish = if has_tool_calls { "tool_calls" } else { "stop" };
    let mut result = serde_json::json!({
        "id": body.get("id").cloned().unwrap_or(serde_json::Value::Null),
        "object": "chat.completion",
        "created": body.get("created").cloned().unwrap_or_else(|| serde_json::json!(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs())),
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
    if let Some(reasoning) = message.get("reasoning_content").and_then(Value::as_str) {
        if !reasoning.is_empty() {
            output.push(serde_json::json!({
                "type": "reasoning",
                "summary": [{ "type": "summary_text", "text": reasoning }]
            }));
        }
    }
    let content = openai_message_content_to_responses_output(message.get("content"));
    output.push(serde_json::json!({
        "type": "message",
        "role": "assistant",
        "content": content
    }));
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
    let incomplete_reason = match finish_reason {
        "length" => Some("max_output_tokens"),
        "content_filter" => Some("content_filter"),
        "pause_turn" => Some("pause_turn"),
        _ => None,
    };
    let mut result = serde_json::json!({
        "id": body.get("id").cloned().unwrap_or(serde_json::Value::Null),
        "object": "response",
        "created_at": created_at,
        "output": output,
        "status": if incomplete_reason.is_some() { "incomplete" } else { "completed" },
        "incomplete_details": incomplete_reason.map(|reason| serde_json::json!({ "reason": reason })).unwrap_or(serde_json::Value::Null),
        "error": serde_json::Value::Null
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
    _model: &str,
    body: &mut Value,
    _stream: bool,
) -> Result<(), String> {
    if client_format == upstream_format {
        normalize_openai_roles_for_compatibility(upstream_format, body);
        return Ok(());
    }
    // Step 1: client → openai (if client is not openai)
    if client_format != UpstreamFormat::OpenAiCompletion {
        client_to_openai_completion(client_format, body)?;
    }
    normalize_openai_message_roles(body);
    // Step 2: openai → upstream (if upstream is not openai)
    if upstream_format != UpstreamFormat::OpenAiCompletion {
        openai_completion_to_upstream(upstream_format, body)?;
    }
    normalize_openai_roles_for_compatibility(upstream_format, body);
    Ok(())
}

fn normalize_openai_roles_for_compatibility(format: UpstreamFormat, body: &mut Value) {
    match format {
        UpstreamFormat::OpenAiCompletion => normalize_openai_message_roles(body),
        UpstreamFormat::OpenAiResponses => normalize_openai_responses_roles(body),
        _ => {}
    }
}

fn normalize_openai_message_roles(body: &mut Value) {
    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    for message in messages.iter_mut() {
        let role = message.get("role").and_then(Value::as_str);
        if role == Some("developer") {
            message["role"] = Value::String("system".to_string());
        }
    }
}

fn normalize_openai_responses_roles(body: &mut Value) {
    let Some(items) = body.get_mut("input").and_then(Value::as_array_mut) else {
        return;
    };
    for item in items.iter_mut() {
        let role = item.get("role").and_then(Value::as_str);
        if role == Some("developer") {
            item["role"] = Value::String("system".to_string());
        }
    }
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
        let text = input.as_str().unwrap_or("").trim();
        let t = if text.is_empty() { "..." } else { text };
        vec![serde_json::json!({
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": t }]
        })]
    } else {
        body.get("input")
            .and_then(Value::as_array)
            .ok_or("input must be array or string")?
            .to_vec()
    };
    let mut current_assistant: Option<Value> = None;
    for item in items {
        let item_type = item
            .get("type")
            .and_then(Value::as_str)
            .or_else(|| item.get("role").and_then(Value::as_str).map(|_| "message"));
        let Some(ty) = item_type else { continue };
        match ty {
            "message" => {
                let role = item.get("role").and_then(Value::as_str).unwrap_or("user");
                let normalized_role = if role == "developer" { "system" } else { role };
                let content = item.get("content").cloned();
                let content = map_responses_content_to_openai(content);
                if normalized_role == "assistant" {
                    let assistant = current_assistant.get_or_insert_with(|| {
                        serde_json::json!({
                            "role": "assistant",
                            "content": Value::Null
                        })
                    });
                    assistant["role"] = Value::String("assistant".to_string());
                    assistant["content"] = content;
                } else {
                    flush_assistant(&mut messages, &mut current_assistant);
                    messages.push(serde_json::json!({ "role": normalized_role, "content": content }));
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
                        a["reasoning_content"] = Value::String(format!("{}{}", existing, summary));
                    }
                }
            }
            _ => {}
        }
    }
    flush_assistant(&mut messages, &mut current_assistant);
    body["messages"] = Value::Array(messages);
    if let Some(tool_choice) = body.get("tool_choice").cloned() {
        if let Some(mapped_tool_choice) = responses_tool_choice_to_openai_tool_choice(&tool_choice) {
            body["tool_choice"] = mapped_tool_choice;
        } else if let Some(obj) = body.as_object_mut() {
            obj.remove("tool_choice");
        }
    }
    if let Some(parallel_tool_calls) = body.get("parallel_tool_calls").cloned() {
        body["parallel_tool_calls"] = parallel_tool_calls;
    }

    // Convert tools from Responses API format to Chat Completions format
    // Responses: { "name": "...", "description": "...", "parameters": {...} }
    // Chat: { "type": "function", "function": { "name": "...", "description": "...", "parameters": {...} } }
    // Note: Responses API may include non-function tools like web_search which don't have names.
    // We only convert tools that have a "name" field (function tools).
    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
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
        if converted_tools.is_empty() {
            // Remove tools array if all tools were filtered out
            if let Some(obj) = body.as_object_mut() {
                obj.remove("tools");
            }
        } else {
            body["tools"] = Value::Array(converted_tools);
        }
    }

    if let Some(obj) = body.as_object_mut() {
        obj.remove("input");
        obj.remove("instructions");
        obj.remove("include");
        obj.remove("prompt_cache_key");
        obj.remove("store");
        obj.remove("reasoning");
    }
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
    let out: Vec<Value> = arr
        .into_iter()
        .map(|c| {
            let ty = c.get("type").and_then(Value::as_str);
            if ty == Some("input_text") || ty == Some("output_text") {
                let text = c
                    .get("text")
                    .cloned()
                    .unwrap_or(Value::String(String::new()));
                return serde_json::json!({ "type": "text", "text": text });
            }
            c
        })
        .collect();
    Value::Array(out)
}

fn messages_to_responses(body: &mut Value) -> Result<(), String> {
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .ok_or("missing messages")?;
    let mut input: Vec<Value> = vec![];
    let mut instructions = String::new();
    for msg in messages {
        let role = msg.get("role").and_then(Value::as_str).unwrap_or("user");
        if role == "system" {
            if let Some(c) = msg.get("content") {
                instructions = c.as_str().unwrap_or("").to_string();
            }
            continue;
        }
        if role == "user" || role == "assistant" {
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
            let content_type = if role == "user" {
                "input_text"
            } else {
                "output_text"
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
    body["instructions"] = Value::String(instructions);
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
    if let Some(obj) = body.as_object_mut() {
        obj.remove("messages");
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
                return serde_json::json!({ "type": "image_url", "image_url": c.get("image_url") });
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
    let items = map_openai_content_to_responses(content.cloned(), "output_text");
    if items.is_empty() {
        vec![serde_json::json!({ "type": "output_text", "text": "" })]
    } else {
        items
    }
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
            // Skip thinking/redacted_thinking blocks - not part of OpenAI format
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
        return Some(vec![m]);
    }
    if parts.is_empty() {
        return Some(vec![
            serde_json::json!({ "role": openai_role, "content": "" }),
        ]);
    }
    let content = collapse_claude_text_parts_for_openai(&parts);
    Some(vec![
        serde_json::json!({ "role": openai_role, "content": content }),
    ])
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
        "max_tokens": body.get("max_tokens").cloned().unwrap_or(serde_json::json!(4096)),
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
        && body.get("tools").and_then(Value::as_array).map(|t| !t.is_empty()).unwrap_or(false)
    {
        result["tool_choice"] =
            serde_json::json!({ "type": "auto", "disable_parallel_tool_use": true });
    }
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .ok_or("missing messages")?;

    // System message: convert to array of blocks with cache_control on last block
    // Reference: 9router claudeHelper.js - add cache_control { type: "ephemeral", ttl: "1h" } to last system block
    let mut system_blocks: Vec<Value> = vec![];
    for msg in messages {
        if msg.get("role").and_then(Value::as_str) != Some("system") {
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
        // Add cache_control to last system block
        let last_idx = system_blocks.len() - 1;
        system_blocks[last_idx]["cache_control"] =
            serde_json::json!({ "type": "ephemeral", "ttl": "1h" });
        result["system"] = Value::Array(system_blocks);
    }

    let non_system: Vec<_> = messages
        .iter()
        .filter(|m| m.get("role").and_then(Value::as_str) != Some("system"))
        .cloned()
        .collect();

    // Find last assistant message index (for cache_control)
    // Reference: 9router openai-to-claude.js - add cache_control to last assistant message's last block
    let mut last_assistant_idx: Option<usize> = None;
    for (i, msg) in non_system.iter().enumerate() {
        if msg.get("role").and_then(Value::as_str) == Some("assistant") {
            last_assistant_idx = Some(i);
        }
    }

    let mut pending_tool_results: Vec<Value> = vec![];
    for (i, msg) in non_system.into_iter().enumerate() {
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
                result["messages"]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::json!({ "role": "user", "content": pending_tool_results.clone() }));
                pending_tool_results.clear();
            }

            // Add cache_control to last assistant message's last block
            if last_assistant_idx == Some(i)
                && role == "assistant"
                && !claude_blocks.is_empty()
                && can_attach_cache_control_to_content_block(&claude_blocks[claude_blocks.len() - 1])
            {
                let last_block_idx = claude_blocks.len() - 1;
                claude_blocks[last_block_idx]["cache_control"] =
                    serde_json::json!({ "type": "ephemeral" });
            }
            result["messages"].as_array_mut().unwrap().push(serde_json::json!({
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

    // Tools: add cache_control to last tool
    // Reference: 9router claudeHelper.js - add cache_control { type: "ephemeral", ttl: "1h" } to last tool
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
            let mut claude_tools = claude_tools;
            // Add cache_control to last tool
            let last_idx = claude_tools.len() - 1;
            claude_tools[last_idx]["cache_control"] =
                serde_json::json!({ "type": "ephemeral", "ttl": "1h" });
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
                    "type": "tool_use",
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
        can_attach_cache_control_to_content_block, openai_message_to_claude_blocks,
        openai_to_claude,
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
        let assistant_content = messages[0]["content"].as_array().expect("assistant content");
        assert_eq!(assistant_content[1]["type"], "tool_use");
        assert!(assistant_content[1].get("cache_control").is_none());
        assert!(can_attach_cache_control_to_content_block(&assistant_content[0]));
        assert!(!can_attach_cache_control_to_content_block(&assistant_content[1]));
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
            if let Some(msg) = convert_gemini_content_to_openai(content) {
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
    *body = result;
    Ok(())
}

fn convert_gemini_content_to_openai(content: &Value) -> Option<Value> {
    let role = content
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("user");
    let openai_role = if role == "user" { "user" } else { "assistant" };
    let parts = content.get("parts").and_then(Value::as_array)?;
    let mut openai_parts: Vec<Value> = vec![];
    let mut tool_calls: Vec<Value> = vec![];
    for part in parts {
        if part.get("text").is_some() {
            openai_parts.push(serde_json::json!({ "type": "text", "text": part.get("text") }));
        }
        if let Some(inline) = part.get("inlineData") {
            let mime = inline
                .get("mimeType")
                .and_then(Value::as_str)
                .unwrap_or("image/png");
            let data = inline.get("data").and_then(Value::as_str).unwrap_or("");
            openai_parts.push(serde_json::json!({
                "type": "image_url",
                "image_url": { "url": format!("data:{};base64,{}", mime, data) }
            }));
        }
        if let Some(fc) = part.get("functionCall") {
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
        if let Some(fr) = part.get("functionResponse") {
            let call_id = fr.get("id").or(fr.get("name")).cloned();
            let resp = fr.get("response");
            let content_str = resp
                .and_then(|r| r.get("result").cloned())
                .or_else(|| resp.cloned())
                .map(|v| serde_json::to_string(&v).unwrap_or_default())
                .unwrap_or_default();
            return Some(serde_json::json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": content_str
            }));
        }
    }
    if !tool_calls.is_empty() {
        let mut m = serde_json::json!({ "role": "assistant", "tool_calls": tool_calls });
        if !openai_parts.is_empty() {
            m["content"] = if openai_parts.len() == 1
                && openai_parts[0].get("type").and_then(Value::as_str) == Some("text")
            {
                openai_parts[0]
                    .get("text")
                    .cloned()
                    .unwrap_or(Value::String(String::new()))
            } else {
                Value::Array(openai_parts)
            };
        }
        return Some(m);
    }
    if openai_parts.is_empty() {
        return None;
    }
    let content = if openai_parts.len() == 1
        && openai_parts[0].get("type").and_then(Value::as_str) == Some("text")
    {
        openai_parts[0]
            .get("text")
            .cloned()
            .unwrap_or(Value::String(String::new()))
    } else {
        Value::Array(openai_parts)
    };
    Some(serde_json::json!({ "role": openai_role, "content": content }))
}

fn uuid_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{:x}", t)
}

fn openai_to_gemini(body: &mut Value) -> Result<(), String> {
    let mut result = serde_json::json!({
        "model": body.get("model").cloned().unwrap_or(serde_json::Value::Null),
        "contents": [],
        "generationConfig": {},
        "safetySettings": []
    });
    if let Some(mt) = body.get("max_tokens") {
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
    for msg in messages {
        let role = msg.get("role").and_then(Value::as_str).unwrap_or("user");
        if role == "system" {
            let text = extract_text_content(msg.get("content"));
            if !text.is_empty() {
                result["systemInstruction"] = serde_json::json!({
                    "role": "user",
                    "parts": [{ "text": text }]
                });
            }
            continue;
        }
        if role == "user" {
            let parts = openai_content_to_gemini_parts(msg.get("content"));
            if !parts.is_empty() {
                result["contents"]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::json!({ "role": "user", "parts": parts }));
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
                for t in tc {
                    let args = t
                        .get("function")
                        .and_then(|f| f.get("arguments"))
                        .and_then(Value::as_str)
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or(serde_json::json!({}));
                    parts.push(serde_json::json!({
                        "functionCall": {
                            "id": t.get("id"),
                            "name": t.get("function").and_then(|f| f.get("name")),
                            "args": args
                        }
                    }));
                }
            }
            if !parts.is_empty() {
                result["contents"]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::json!({ "role": "model", "parts": parts }));
            }
        }
        if role == "tool" {
            let call_id = msg.get("tool_call_id").cloned();
            let content = msg
                .get("content")
                .cloned()
                .unwrap_or(Value::String(String::new()));
            result["contents"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({
                    "role": "user",
                    "parts": [{
                        "functionResponse": {
                            "id": call_id,
                            "name": call_id,
                            "response": { "result": content }
                        }
                    }]
                }));
        }
    }
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
        assert_eq!(messages[0]["content"][0]["type"], "text");
        assert_eq!(messages[0]["content"][0]["text"], "Hi");
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
        assert_eq!(system[0]["cache_control"]["type"], "ephemeral");
        assert_eq!(system[0]["cache_control"]["ttl"], "1h");
        assert!(body.get("messages").is_some());
        assert!(!body["messages"].as_array().unwrap().is_empty());
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
        let assistant_blocks = messages[1]["content"].as_array().expect("assistant content");
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
        let assistant_blocks = messages[1]["content"].as_array().expect("assistant content");
        assert_eq!(assistant_blocks[0]["type"], "text");
        assert_eq!(assistant_blocks[0]["text"], "Let me check.");
        assert_eq!(assistant_blocks[1]["type"], "tool_use");
        assert_eq!(assistant_blocks[1]["id"], "call_1");
        let user_blocks = messages[2]["content"].as_array().expect("user content");
        assert_eq!(user_blocks[0]["type"], "tool_result");
        assert_eq!(user_blocks[0]["tool_use_id"], "call_1");
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
        assert_eq!(out["choices"][0]["finish_reason"], "context_length_exceeded");
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
}
