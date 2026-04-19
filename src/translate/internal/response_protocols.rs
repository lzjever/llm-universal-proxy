use serde_json::Value;

use super::request_gemini::convert_gemini_content_to_openai;
use super::response_logprobs::{
    normalized_response_logprobs_from_gemini_candidate,
    normalized_response_logprobs_to_openai_values,
};
use super::{
    classify_portable_non_success_terminal, openai_message_to_claude_blocks,
    openai_message_anthropic_reasoning_replay_blocks,
    openai_tool_arguments_to_structured_value, single_optional_array_item,
    single_required_array_item,
};

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

pub(super) fn openai_finish_reason_to_gemini(finish_reason: &str) -> &'static str {
    match finish_reason {
        "stop" | "tool_calls" => "STOP",
        "length" => "MAX_TOKENS",
        "content_filter" => "SAFETY",
        "pause_turn" | "context_length_exceeded" | "tool_error" | "error" => "OTHER",
        _ => "STOP",
    }
}

pub(super) const GEMINI_DUMMY_THOUGHT_SIGNATURE: &str = "skip_thought_signature_validator";

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

pub(super) fn responses_finish_reason_to_openai(body: &Value, has_tool_calls: bool) -> String {
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

pub(super) fn push_gemini_function_call_part(
    parts: &mut Vec<Value>,
    tool_call: &Value,
    first_in_step: bool,
) -> Result<(), String> {
    let args_val = openai_tool_arguments_to_structured_value(tool_call, "Gemini")?;
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
    Ok(())
}

pub(super) fn gemini_response_to_openai(body: &Value) -> Result<Value, String> {
    let response = body.get("response").unwrap_or(body);
    let candidate = single_optional_array_item(
        response
            .get("candidates")
            .and_then(Value::as_array)
            .map(Vec::as_slice),
        "Gemini response",
        "OpenAI Chat Completions",
        "candidates",
    )?;
    let response_logprobs = match candidate {
        Some(candidate) => normalized_response_logprobs_from_gemini_candidate(
            candidate,
            "OpenAI Chat Completions",
        )?,
        None => None,
    };
    let message = if let Some(candidate) = candidate {
        gemini_candidate_to_openai_assistant_message(candidate.get("content"))?
    } else {
        serde_json::json!({ "role": "assistant", "content": "" })
    };
    let has_tool_calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|tool_calls| !tool_calls.is_empty())
        .unwrap_or(false);
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
    if let Some(content_logprobs) = response_logprobs {
        result["choices"][0]["logprobs"] = serde_json::json!({
            "content": normalized_response_logprobs_to_openai_values(&content_logprobs),
            "refusal": []
        });
    }
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

        let completion_tokens = candidates_tokens + thoughts_tokens;

        let mut usage_json = serde_json::json!({
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens": total_tokens
        });

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

pub(super) fn gemini_candidate_to_openai_assistant_message(
    content: Option<&Value>,
) -> Result<Value, String> {
    let translated = match content {
        Some(content) => convert_gemini_content_to_openai(content)?,
        None => Vec::new(),
    };
    if translated.is_empty() {
        return Ok(serde_json::json!({
            "role": "assistant",
            "content": ""
        }));
    }
    if translated.len() == 1
        && translated[0].get("role").and_then(Value::as_str) == Some("assistant")
    {
        if gemini_assistant_message_has_non_text_content_parts(&translated[0]) {
            return Err(
                "Gemini assistant multimodal output cannot be faithfully translated to OpenAI Chat Completions assistant content."
                    .to_string(),
            );
        }
        return Ok(translated[0].clone());
    }
    Err(
        "Gemini response content cannot be faithfully translated to a single OpenAI Chat Completions assistant message."
            .to_string(),
    )
}

pub(super) fn gemini_assistant_message_has_non_text_content_parts(message: &Value) -> bool {
    message
        .get("content")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .any(|part| part.get("type").and_then(Value::as_str) != Some("text"))
        })
        .unwrap_or(false)
}

pub(super) fn is_minimax_model(model: &str) -> bool {
    model.starts_with("MiniMax-")
}

pub(super) fn reasoning_details_to_text(value: Option<&Value>) -> Option<String> {
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

pub(super) fn openai_message_reasoning_text(message: &Value) -> Option<String> {
    if let Some(text) = message.get("reasoning_content").and_then(Value::as_str) {
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }
    reasoning_details_to_text(message.get("reasoning_details"))
}

pub(super) fn normalize_openai_completion_response(body: &Value) -> Value {
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

fn sanitize_openai_message_for_claude_response(message: &Value) -> Value {
    let mut sanitized = message.clone();
    if openai_message_reasoning_text(message).is_none()
        || openai_message_anthropic_reasoning_replay_blocks(message).is_some()
    {
        return sanitized;
    }
    let Some(obj) = sanitized.as_object_mut() else {
        return sanitized;
    };
    obj.remove("reasoning_content");
    obj.remove("reasoning_details");
    sanitized
}

pub(super) fn openai_response_to_claude(body: &Value) -> Result<Value, String> {
    let choice = single_required_array_item(
        body.get("choices")
            .and_then(Value::as_array)
            .map(Vec::as_slice),
        "OpenAI response",
        "Anthropic",
        "choices",
    )?;
    let message = choice.get("message").ok_or("missing message")?;
    let sanitized_message = sanitize_openai_message_for_claude_response(message);
    let content = openai_message_to_claude_blocks(&sanitized_message)?
        .unwrap_or_else(|| vec![serde_json::json!({ "type": "text", "text": "" })]);
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
