use serde_json::Value;

use super::messages::single_required_array_item;
use super::{
    append_openai_message_anthropic_reasoning_replay_blocks,
    classify_portable_non_success_terminal, openai_message_anthropic_reasoning_replay_blocks,
    openai_message_to_claude_blocks,
};

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

fn prepare_openai_message_for_claude_response(message: &Value) -> Value {
    let mut prepared = message.clone();
    if openai_message_anthropic_reasoning_replay_blocks(message).is_some() {
        return prepared;
    }
    let Some(reasoning) = openai_message_reasoning_text(message) else {
        return prepared;
    };
    if reasoning.is_empty() {
        return prepared;
    }
    append_openai_message_anthropic_reasoning_replay_blocks(
        &mut prepared,
        vec![serde_json::json!({
            "type": "thinking",
            "thinking": reasoning
        })],
    );
    prepared
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
    let prepared_message = prepare_openai_message_for_claude_response(message);
    let content = openai_message_to_claude_blocks(&prepared_message)?
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
