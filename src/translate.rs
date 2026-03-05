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
fn upstream_response_to_openai(upstream_format: UpstreamFormat, body: &Value) -> Result<Value, String> {
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
    let content = body.get("content").and_then(Value::as_array).ok_or("missing content")?;
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
    let mut finish_reason = body.get("stop_reason").and_then(Value::as_str).unwrap_or("stop").to_string();
    if finish_reason == "end_turn" {
        finish_reason = "stop".to_string();
    }
    if finish_reason == "tool_use" {
        finish_reason = "tool_calls".to_string();
    }
    let mut result = serde_json::json!({
        "id": body.get("id").cloned().unwrap_or_else(|| serde_json::json!(format!("chatcmpl-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()))),
        "object": "chat.completion",
        "created": body.get("created").cloned().unwrap_or_else(|| serde_json::json!(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs())),
        "model": body.get("model").cloned().unwrap_or(serde_json::json!("claude")),
        "choices": [{ "index": 0, "message": message, "finish_reason": finish_reason }]
    });
    if let Some(usage) = body.get("usage") {
        result["usage"] = serde_json::json!({
            "prompt_tokens": usage.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
            "completion_tokens": usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
            "total_tokens": usage.get("input_tokens").and_then(Value::as_u64).unwrap_or(0) + usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0)
        });
    }
    Ok(result)
}

fn gemini_response_to_openai(body: &Value) -> Result<Value, String> {
    let response = body.get("response").unwrap_or(body);
    let candidates = response.get("candidates").and_then(Value::as_array).ok_or("missing candidates")?;
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
    let mut finish_reason = candidate.get("finishReason").and_then(Value::as_str).unwrap_or("stop").to_lowercase();
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
    if let Some(u) = usage {
        result["usage"] = serde_json::json!({
            "prompt_tokens": u.get("promptTokenCount").and_then(Value::as_u64).unwrap_or(0) + u.get("thoughtsTokenCount").and_then(Value::as_u64).unwrap_or(0),
            "completion_tokens": u.get("candidatesTokenCount").and_then(Value::as_u64).unwrap_or(0),
            "total_tokens": u.get("totalTokenCount").and_then(Value::as_u64).unwrap_or(0)
        });
    }
    Ok(result)
}

fn openai_response_to_claude(body: &Value) -> Result<Value, String> {
    let choices = body.get("choices").and_then(Value::as_array).ok_or("missing choices")?;
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
            let args = t.get("function").and_then(|f| f.get("arguments")).and_then(Value::as_str);
            let input = args.and_then(|s| serde_json::from_str(s).ok()).unwrap_or(serde_json::json!({}));
            content.push(serde_json::json!({
                "type": "tool_use",
                "id": t.get("id"),
                "name": t.get("function").and_then(|f| f.get("name")),
                "input": input
            }));
        }
    }
    let mut finish = choice.get("finish_reason").and_then(Value::as_str).unwrap_or("stop").to_string();
    if finish == "tool_calls" {
        finish = "tool_use".to_string();
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
    let choices = body.get("choices").and_then(Value::as_array).ok_or("missing choices")?;
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
            let args = t.get("function").and_then(|f| f.get("arguments")).and_then(Value::as_str);
            let args_val = args.and_then(|s| serde_json::from_str(s).ok()).unwrap_or(serde_json::json!({}));
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
    let finish = choice.get("finish_reason").and_then(Value::as_str).unwrap_or("stop");
    let mut result = serde_json::json!({
        "candidates": [{
            "content": { "role": "model", "parts": parts },
            "finishReason": finish
        }],
        "usageMetadata": body.get("usage").cloned().unwrap_or(serde_json::json!({})),
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
        result["usage"] = u.clone();
    }
    Ok(result)
}

fn openai_response_to_responses(body: &Value) -> Result<Value, String> {
    let choices = body.get("choices").and_then(Value::as_array).ok_or("missing choices")?;
    let choice = choices.first().ok_or("empty choices")?;
    let message = choice.get("message").ok_or("missing message")?;
    let content = message.get("content").and_then(Value::as_str).unwrap_or("");
    let mut output: Vec<Value> = vec![];
    output.push(serde_json::json!({
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "output_text", "text": content }]
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
    let created_at = body
        .get("created")
        .cloned()
        .unwrap_or_else(|| serde_json::json!(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()));
    let mut result = serde_json::json!({
        "id": body.get("id").cloned().unwrap_or(serde_json::Value::Null),
        "object": "response",
        "created_at": created_at,
        "output": output,
        "status": "completed"
    });
    if let Some(u) = body.get("usage") {
        result["usage"] = u.clone();
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
        return Ok(());
    }
    // Step 1: client → openai (if client is not openai)
    if client_format != UpstreamFormat::OpenAiCompletion {
        client_to_openai_completion(client_format, body)?;
    }
    // Step 2: openai → upstream (if upstream is not openai)
    if upstream_format != UpstreamFormat::OpenAiCompletion {
        openai_completion_to_upstream(upstream_format, body)?;
    }
    Ok(())
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
            .iter()
            .cloned()
            .collect()
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
                flush_assistant(&mut messages, &mut current_assistant);
                let role = item.get("role").and_then(Value::as_str).unwrap_or("user");
                let content = item.get("content").cloned();
                let content = map_responses_content_to_openai(content);
                messages.push(serde_json::json!({ "role": role, "content": content }));
            }
            "function_call" => {
                let call_id = item.get("call_id").cloned();
                let name = item.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                let args = item.get("arguments").cloned().unwrap_or(serde_json::json!("{}"));
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
            "reasoning" => {}
            _ => {}
        }
    }
    flush_assistant(&mut messages, &mut current_assistant);
    body["messages"] = Value::Array(messages);
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
                let text = c.get("text").cloned().unwrap_or(Value::String(String::new()));
                return serde_json::json!({ "type": "text", "text": text });
            }
            c
        })
        .collect();
    Value::Array(out)
}

fn messages_to_responses(body: &mut Value) -> Result<(), String> {
    let messages = body.get("messages").and_then(Value::as_array).ok_or("missing messages")?;
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
            let content = msg.get("content").cloned();
            let content_type = if role == "user" { "input_text" } else { "output_text" };
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
    if let Some(obj) = body.as_object_mut() {
        obj.remove("messages");
    }
    Ok(())
}

fn map_openai_content_to_responses(content: Option<Value>, content_type: &str) -> Vec<Value> {
    let content = match content {
        None => return vec![],
        Some(Value::String(s)) => return vec![serde_json::json!({ "type": content_type, "text": s })],
        Some(Value::Array(a)) => a,
        Some(_) => return vec![],
    };
    content
        .into_iter()
        .map(|c| {
            let ty = c.get("type").and_then(Value::as_str);
            if ty == Some("text") {
                let text = c.get("text").cloned().unwrap_or(Value::String(String::new()));
                return serde_json::json!({ "type": content_type, "text": text });
            }
            if ty == Some("image_url") {
                return serde_json::json!({ "type": "image_url", "image_url": c.get("image_url") });
            }
            let text = c.get("text").or(c.get("content")).cloned();
            let text = text.and_then(|t| t.as_str().map(String::from)).unwrap_or_else(|| serde_json::to_string(&c).unwrap_or_default());
            serde_json::json!({ "type": content_type, "text": text })
        })
        .collect()
}

fn claude_to_openai(body: &mut Value) -> Result<(), String> {
    let mut result = serde_json::json!({
        "model": body.get("model").cloned().unwrap_or(serde_json::Value::Null),
        "messages": [],
        "stream": body.get("stream").cloned().unwrap_or(serde_json::json!(true))
    });
    if let Some(max_tokens) = body.get("max_tokens") {
        result["max_tokens"] = max_tokens.clone();
    }
    if let Some(t) = body.get("temperature") {
        result["temperature"] = t.clone();
    }
    if let Some(system) = body.get("system") {
        let text = if system.is_string() {
            system.as_str().unwrap_or("").to_string()
        } else if let Some(arr) = system.as_array() {
            arr.iter()
                .filter_map(|s| s.get("text").and_then(Value::as_str))
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
    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
        result["tools"] = serde_json::Value::Array(
            tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.get("name"),
                            "description": t.get("description"),
                            "parameters": t.get("input_schema").or(t.get("parameters")).unwrap_or(&serde_json::json!({ "type": "object", "properties": {} }))
                        }
                    })
                })
                .collect(),
        );
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
        return Some(vec![serde_json::json!({ "role": openai_role, "content": content })]);
    }
    let arr = content.as_array()?;
    let mut parts: Vec<Value> = vec![];
    let mut tool_calls: Vec<Value> = vec![];
    let mut tool_results: Vec<Value> = vec![];
    for block in arr {
        let ty = block.get("type").and_then(Value::as_str)?;
        match ty {
            "text" => parts.push(serde_json::json!({ "type": "text", "text": block.get("text") })),
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
            _ => {}
        }
    }
    if !tool_results.is_empty() {
        let mut out: Vec<Value> = tool_results;
        if !parts.is_empty() {
            let content = if parts.len() == 1 && parts[0].get("type").and_then(Value::as_str) == Some("text") {
                parts[0].get("text").cloned().unwrap_or(Value::String(String::new()))
            } else {
                Value::Array(parts)
            };
            out.push(serde_json::json!({ "role": "user", "content": content }));
        }
        return Some(out);
    }
    if !tool_calls.is_empty() {
        let mut m = serde_json::json!({ "role": "assistant", "tool_calls": tool_calls });
        if !parts.is_empty() {
            m["content"] = if parts.len() == 1 && parts[0].get("type").and_then(Value::as_str) == Some("text") {
                parts[0].get("text").cloned().unwrap_or(Value::String(String::new()))
            } else {
                Value::Array(parts)
            };
        }
        return Some(vec![m]);
    }
    if parts.is_empty() {
        return Some(vec![serde_json::json!({ "role": openai_role, "content": "" })]);
    }
    let content = if parts.len() == 1 && parts[0].get("type").and_then(Value::as_str) == Some("text") {
        parts[0].get("text").cloned().unwrap_or(Value::String(String::new()))
    } else {
        Value::Array(parts)
    };
    Some(vec![serde_json::json!({ "role": openai_role, "content": content })])
}

fn openai_to_claude(body: &mut Value) -> Result<(), String> {
    let mut result = serde_json::json!({
        "model": body.get("model").cloned().unwrap_or(serde_json::Value::Null),
        "max_tokens": body.get("max_tokens").cloned().unwrap_or(serde_json::json!(4096)),
        "messages": [],
        "stream": body.get("stream").cloned().unwrap_or(serde_json::json!(true))
    });
    if let Some(t) = body.get("temperature") {
        result["temperature"] = t.clone();
    }
    let messages = body.get("messages").and_then(Value::as_array).ok_or("missing messages")?;
    let mut system_parts: Vec<String> = vec![];
    for msg in messages {
        if msg.get("role").and_then(Value::as_str) != Some("system") {
            continue;
        }
        let c = msg.get("content");
        let text = c.and_then(Value::as_str).map(String::from).unwrap_or_else(|| extract_text_content(c));
        if !text.is_empty() {
            system_parts.push(text);
        }
    }
    if !system_parts.is_empty() {
        result["system"] = serde_json::Value::String(system_parts.join("\n"));
    }
    let non_system: Vec<_> = messages.iter().filter(|m| m.get("role").and_then(Value::as_str) != Some("system")).cloned().collect();
    for msg in non_system {
        if let Some(claude_blocks) = openai_message_to_claude_blocks(&msg) {
            result["messages"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({ "role": if msg.get("role").and_then(Value::as_str) == Some("user") || msg.get("role").and_then(Value::as_str) == Some("tool") { "user" } else { "assistant" }, "content": claude_blocks }));
        }
    }
    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
        result["tools"] = serde_json::Value::Array(
            tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.get("function").and_then(|f| f.get("name")),
                        "description": t.get("function").and_then(|f| f.get("description")),
                        "input_schema": t.get("function").and_then(|f| f.get("parameters"))
                    })
                })
                .collect(),
        );
    }
    *body = result;
    Ok(())
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
    if let Some(Value::String(s)) = content {
        return Some(vec![serde_json::json!({ "type": "text", "text": s })]);
    }
    let arr = content.and_then(Value::as_array)?;
    let mut blocks: Vec<Value> = vec![];
    for c in arr {
        let ty = c.get("type").and_then(Value::as_str);
        if ty == Some("text") {
            blocks.push(serde_json::json!({ "type": "text", "text": c.get("text") }));
        } else if ty == Some("image_url") {
            let url = c.get("image_url").and_then(|u| u.get("url").and_then(Value::as_str)).unwrap_or("");
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
        "stream": body.get("stream").cloned().unwrap_or(serde_json::json!(true))
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
    let role = content.get("role").and_then(Value::as_str)?;
    let openai_role = if role == "user" { "user" } else { "assistant" };
    let parts = content.get("parts").and_then(Value::as_array)?;
    let mut openai_parts: Vec<Value> = vec![];
    let mut tool_calls: Vec<Value> = vec![];
    for part in parts {
        if part.get("text").is_some() {
            openai_parts.push(serde_json::json!({ "type": "text", "text": part.get("text") }));
        }
        if let Some(inline) = part.get("inlineData") {
            let mime = inline.get("mimeType").and_then(Value::as_str).unwrap_or("image/png");
            let data = inline.get("data").and_then(Value::as_str).unwrap_or("");
            openai_parts.push(serde_json::json!({
                "type": "image_url",
                "image_url": { "url": format!("data:{};base64,{}", mime, data) }
            }));
        }
        if let Some(fc) = part.get("functionCall") {
            let id = fc.get("id").cloned().unwrap_or_else(|| serde_json::json!(format!("call_{}", uuid_simple())));
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
            m["content"] = if openai_parts.len() == 1 && openai_parts[0].get("type").and_then(Value::as_str) == Some("text") {
                openai_parts[0].get("text").cloned().unwrap_or(Value::String(String::new()))
            } else {
                Value::Array(openai_parts)
            };
        }
        return Some(m);
    }
    if openai_parts.is_empty() {
        return None;
    }
    let content = if openai_parts.len() == 1 && openai_parts[0].get("type").and_then(Value::as_str) == Some("text") {
        openai_parts[0].get("text").cloned().unwrap_or(Value::String(String::new()))
    } else {
        Value::Array(openai_parts)
    };
    Some(serde_json::json!({ "role": openai_role, "content": content }))
}

fn uuid_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
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
    let messages = body.get("messages").and_then(Value::as_array).ok_or("missing messages")?;
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
            let content = msg.get("content").cloned().unwrap_or(Value::String(String::new()));
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
            let url = c.get("image_url").and_then(|u| u.get("url").and_then(Value::as_str)).unwrap_or("");
            if let Some((mime, data)) = url.strip_prefix("data:").and_then(|r| r.split_once(";base64,")) {
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
    fn translate_request_same_format_passthrough() {
        let mut body = json!({ "model": "gpt-4o", "messages": [{ "role": "user", "content": "Hi" }] });
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
        assert_eq!(body["system"], "Sys");
        assert!(body.get("messages").is_some());
        assert!(body["messages"].as_array().unwrap().len() >= 1);
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
        assert!(out["content"].as_array().unwrap().iter().any(|b| b.get("type").and_then(Value::as_str) == Some("text")));
    }
}
