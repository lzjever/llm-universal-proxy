use serde_json::Value;

use crate::formats::UpstreamFormat;

use super::assessment::{openai_named_tool_choice_name, shared_control_profile_for_target};
use super::messages::{
    custom_tools_not_portable_message, openai_assistant_audio_field_not_portable_message,
    reserved_openai_custom_bridge_prefix_message,
    responses_multiple_output_audio_items_not_portable_message, single_required_array_item,
    translation_target_label,
};
use super::models::{
    NormalizedOpenAiFamilyToolCall, NormalizedOpenAiFamilyToolDef, NormalizedToolPolicy,
    SemanticTextPart, SemanticToolKind,
};
use super::openai_family::{
    collapse_openai_text_parts, copy_remaining_usage_fields, extract_openai_refusal,
    normalized_output_shape_to_openai_response_format,
    normalized_output_shape_to_responses_text_format, openai_normalized_request_controls,
    responses_normalized_request_controls,
};
use super::response_logprobs::{
    attach_openai_choice_logprobs_to_responses_content,
    normalized_response_logprobs_from_openai_choice, responses_nonportable_output_item_message,
    responses_output_text_logprobs,
};
use super::response_protocols::{openai_message_reasoning_text, responses_finish_reason_to_openai};
use super::tools::{
    content_value_is_effectively_empty, normalized_openai_tool_definitions_from_request,
    normalized_responses_tool_call, normalized_responses_tool_definitions_from_request,
    normalized_tool_definition_to_openai, normalized_tool_definition_to_openai_with_custom_bridge,
    normalized_tool_definition_to_responses, openai_responses_custom_tool_bridge_name,
    openai_responses_custom_tool_bridge_prefix_is_reserved, openai_tool_call_to_responses_item,
    openai_tool_call_to_responses_item_decoding_custom_bridge,
    openai_tool_result_content_to_responses_output, responses_item_is_tool_output,
    responses_tool_call_item_to_openai_tool_call_strict,
    responses_tool_call_item_to_openai_tool_call_with_custom_bridge_strict,
    responses_tool_output_to_openai_tool_content, semantic_text_part_to_openai_value,
    semantic_text_part_to_responses_value, semantic_tool_kind_from_value,
    semantic_tool_output_item_type,
};

const OPENAI_ANTHROPIC_REASONING_REPLAY_FIELD: &str = "_anthropic_reasoning_replay";
const ANTHROPIC_REASONING_CARRIER_PREFIX: &str = "anthropic-thinking-v1:";

pub(super) fn encode_anthropic_reasoning_carrier(blocks: &[Value]) -> Result<String, String> {
    let payload = serde_json::json!({
        "format": "anthropic-thinking-replay",
        "version": 1,
        "blocks": blocks
    });
    let encoded = serde_json::to_vec(&payload)
        .map(hex::encode)
        .map_err(|err| format!("serialize Anthropic reasoning replay carrier: {err}"))?;
    Ok(format!("{ANTHROPIC_REASONING_CARRIER_PREFIX}{encoded}"))
}

pub(super) fn decode_anthropic_reasoning_carrier(carrier: &str) -> Result<Vec<Value>, String> {
    let encoded = carrier
        .strip_prefix(ANTHROPIC_REASONING_CARRIER_PREFIX)
        .ok_or("unsupported carrier prefix")?;
    let decoded = hex::decode(encoded).map_err(|err| format!("decode carrier hex: {err}"))?;
    let payload: Value =
        serde_json::from_slice(&decoded).map_err(|err| format!("decode carrier json: {err}"))?;
    if payload.get("format").and_then(Value::as_str) != Some("anthropic-thinking-replay") {
        return Err("unsupported carrier format".to_string());
    }
    if payload.get("version").and_then(Value::as_u64) != Some(1) {
        return Err("unsupported carrier version".to_string());
    }
    let blocks = payload
        .get("blocks")
        .and_then(Value::as_array)
        .ok_or("carrier blocks must be an array")?;
    if blocks
        .iter()
        .any(|block| block.get("type").and_then(Value::as_str) != Some("thinking"))
    {
        return Err("carrier blocks must only contain Anthropic thinking blocks".to_string());
    }
    Ok(blocks.clone())
}

pub(super) fn openai_message_anthropic_reasoning_replay_blocks(
    message: &Value,
) -> Option<Vec<Value>> {
    message
        .get(OPENAI_ANTHROPIC_REASONING_REPLAY_FIELD)
        .and_then(Value::as_array)
        .cloned()
}

pub(super) fn append_openai_message_anthropic_reasoning_replay_blocks(
    message: &mut Value,
    blocks: Vec<Value>,
) {
    if blocks.is_empty() {
        return;
    }
    let Some(obj) = message.as_object_mut() else {
        return;
    };
    let replay = obj
        .entry(OPENAI_ANTHROPIC_REASONING_REPLAY_FIELD.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Some(arr) = replay.as_array_mut() {
        arr.extend(blocks);
    }
}

fn responses_output_audio_item_to_openai_audio(item: &Value) -> Option<Value> {
    if item.get("type").and_then(Value::as_str) != Some("output_audio") {
        return None;
    }

    let mut merged = serde_json::Map::new();
    if let Some(audio) = item.get("audio") {
        if let Some(audio_obj) = audio.as_object() {
            merged.extend(audio_obj.clone());
        } else if !audio.is_null() {
            merged.insert("data".to_string(), audio.clone());
        }
    }
    if let Some(item_obj) = item.as_object() {
        for (key, value) in item_obj {
            if matches!(key.as_str(), "type" | "audio") {
                continue;
            }
            merged.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    if merged.is_empty() {
        None
    } else {
        Some(Value::Object(merged))
    }
}

pub(super) fn responses_portable_output_item_type(item_type: &str) -> bool {
    matches!(
        item_type,
        "message" | "function_call" | "custom_tool_call" | "reasoning" | "output_audio"
    )
}

pub(super) fn responses_hosted_output_item_type(item_type: &str) -> bool {
    matches!(
        item_type,
        "file_search_call"
            | "web_search_call"
            | "code_interpreter_call"
            | "mcp_call"
            | "image_generation_call"
            | "computer_call"
            | "computer_call_output"
    )
}

fn responses_response_audio_to_openai_audio(body: &Value) -> Result<Option<Value>, String> {
    let Some(output) = body.get("output").and_then(Value::as_array) else {
        return Ok(None);
    };

    let audio_items = output
        .iter()
        .filter_map(responses_output_audio_item_to_openai_audio)
        .collect::<Vec<_>>();
    match audio_items.len() {
        0 => Ok(None),
        1 => Ok(audio_items.into_iter().next()),
        _ => Err(responses_multiple_output_audio_items_not_portable_message(
            "OpenAI Chat Completions",
        )),
    }
}

fn openai_assistant_audio_to_responses_output_item(
    message: &Value,
) -> Result<Option<Value>, String> {
    let Some(audio) = message.get("audio") else {
        return Ok(None);
    };
    if audio.is_null() {
        return Ok(None);
    }
    let mut item = serde_json::Map::new();
    item.insert(
        "type".to_string(),
        Value::String("output_audio".to_string()),
    );
    if let Some(audio_obj) = audio.as_object() {
        for field in ["id", "expires_at"] {
            if audio_obj.get(field).is_some() {
                return Err(openai_assistant_audio_field_not_portable_message(
                    field,
                    "OpenAI Responses",
                ));
            }
        }
        if let Some(data) = audio_obj.get("data").cloned() {
            item.insert("data".to_string(), data);
        }
        if let Some(transcript) = audio_obj.get("transcript").cloned() {
            item.insert("transcript".to_string(), transcript);
        }
    } else {
        item.insert("data".to_string(), audio.clone());
    }
    Ok(Some(Value::Object(item)))
}

fn clone_usage_details_object(details: Option<&Value>) -> Option<Value> {
    let details = details?.as_object()?;
    (!details.is_empty()).then(|| Value::Object(details.clone()))
}

fn extract_responses_refusal_text(content: Option<&Value>) -> String {
    let Some(content) = content.and_then(Value::as_array) else {
        return String::new();
    };

    content
        .iter()
        .filter_map(|part| {
            (part.get("type").and_then(Value::as_str) == Some("refusal"))
                .then(|| part.get("refusal").and_then(Value::as_str))
                .flatten()
        })
        .collect::<Vec<_>>()
        .join("")
}

fn openai_message_to_responses_content(msg: &Value, content_type: &str) -> Vec<Value> {
    let mut content = map_openai_content_to_responses(msg.get("content").cloned(), content_type);
    if let Some(refusal) = extract_openai_refusal(msg) {
        content.push(serde_json::json!({
            "type": "refusal",
            "refusal": refusal
        }));
    }
    content
}

pub(super) fn responses_response_to_openai(body: &Value) -> Result<Value, String> {
    let output = match body.get("output").and_then(Value::as_array) {
        Some(o) => o,
        None => return Ok(body.clone()),
    };
    if let Some(message) = output
        .iter()
        .find_map(|item| responses_nonportable_output_item_message(item, "OpenAI Chat Completions"))
    {
        return Err(message);
    }
    let audio = responses_response_audio_to_openai_audio(body)?;
    let mut content_parts: Vec<Value> = vec![];
    let mut content_logprobs: Vec<Value> = vec![];
    let mut saw_content_logprobs = false;
    let mut reasoning_content = String::new();
    let mut refusal = String::new();
    let mut tool_calls: Vec<Value> = vec![];
    for item in output {
        let ty = item.get("type").and_then(Value::as_str);
        if ty == Some("message") {
            if let Some(item_logprobs) = responses_output_text_logprobs(item)? {
                saw_content_logprobs = true;
                content_logprobs.extend(item_logprobs);
            }
            if let Some(arr) = item.get("content").and_then(Value::as_array) {
                for part in arr {
                    match part.get("type").and_then(Value::as_str) {
                        Some("output_text") => {
                            let text = part.get("text").and_then(Value::as_str).unwrap_or("");
                            let annotations = part
                                .get("annotations")
                                .and_then(Value::as_array)
                                .cloned()
                                .unwrap_or_default();
                            content_parts.push(semantic_text_part_to_openai_value(
                                &SemanticTextPart {
                                    text: text.to_string(),
                                    annotations,
                                },
                            ));
                        }
                        Some("refusal") => {
                            refusal.push_str(
                                part.get("refusal").and_then(Value::as_str).unwrap_or(""),
                            );
                        }
                        _ => {}
                    }
                }
            }
        }
        if let Some(tool_call) =
            responses_tool_call_item_to_openai_tool_call_strict(item, "OpenAI Chat Completions")?
        {
            tool_calls.push(tool_call);
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
    if !content_parts.is_empty() {
        message["content"] = collapse_openai_text_parts(&content_parts);
    } else if has_tool_calls || message.get("reasoning_content").is_some() || !refusal.is_empty() {
        message["content"] = Value::Null;
    } else {
        message["content"] = Value::String(String::new());
    }
    if !refusal.is_empty() {
        message["refusal"] = Value::String(refusal);
    }
    if let Some(audio) = audio {
        message["audio"] = audio;
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
    if saw_content_logprobs {
        result["choices"][0]["logprobs"] = serde_json::json!({
            "content": content_logprobs,
            "refusal": []
        });
    }
    if let Some(u) = body.get("usage") {
        result["usage"] = responses_usage_to_openai_usage(u);
    }
    Ok(result)
}

pub(super) fn openai_response_to_responses(body: &Value) -> Result<Value, String> {
    let choice = single_required_array_item(
        body.get("choices")
            .and_then(Value::as_array)
            .map(Vec::as_slice),
        "OpenAI response",
        "OpenAI Responses",
        "choices",
    )?;
    let message = choice.get("message").ok_or("missing message")?;
    let mut output: Vec<Value> = vec![];
    let replay_blocks = openai_message_anthropic_reasoning_replay_blocks(message);
    if let Some(reasoning) = openai_message_reasoning_text(message) {
        if !reasoning.is_empty() || replay_blocks.is_some() {
            let mut reasoning_item = serde_json::json!({
                "type": "reasoning",
                "summary": []
            });
            if !reasoning.is_empty() {
                reasoning_item["summary"] =
                    serde_json::json!([{ "type": "summary_text", "text": reasoning }]);
            }
            if let Some(blocks) = replay_blocks.as_ref() {
                reasoning_item["encrypted_content"] =
                    Value::String(encode_anthropic_reasoning_carrier(blocks)?);
            }
            output.push(reasoning_item);
        }
    } else if let Some(blocks) = replay_blocks.as_ref() {
        output.push(serde_json::json!({
            "type": "reasoning",
            "summary": [],
            "encrypted_content": encode_anthropic_reasoning_carrier(blocks)?
        }));
    }
    let audio_output = openai_assistant_audio_to_responses_output_item(message)?;
    let mut content = openai_message_to_responses_content(message, "output_text");
    if let Some(content_logprobs) =
        normalized_response_logprobs_from_openai_choice(choice, "OpenAI Responses")?
    {
        attach_openai_choice_logprobs_to_responses_content(&mut content, &content_logprobs)?;
    }
    if !content.is_empty() {
        output.push(serde_json::json!({
            "type": "message",
            "role": "assistant",
            "content": content
        }));
    } else if message.get("tool_calls").is_none() && output.is_empty() && audio_output.is_none() {
        output.push(serde_json::json!({
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": "" }]
        }));
    }
    if let Some(tc) = message.get("tool_calls").and_then(Value::as_array) {
        for t in tc {
            output.push(openai_tool_call_to_responses_item_decoding_custom_bridge(
                t,
            )?);
        }
    }
    if let Some(audio_output) = audio_output {
        output.push(audio_output);
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

fn reject_reserved_custom_bridge_name(name: &str) -> Result<(), String> {
    if openai_responses_custom_tool_bridge_prefix_is_reserved(name) {
        return Err(reserved_openai_custom_bridge_prefix_message(name));
    }
    Ok(())
}

fn validate_responses_tool_choice_for_custom_bridge(choice: &Value) -> Result<(), String> {
    let Some(choice_obj) = choice.as_object() else {
        return Ok(());
    };

    match choice_obj.get("type").and_then(Value::as_str) {
        Some("function") => {
            if let Some(name) = openai_named_tool_choice_name(choice, "function") {
                reject_reserved_custom_bridge_name(name)?;
            }
        }
        Some("allowed_tools") => {
            let Some(selected_tools) = choice_obj
                .get("allowed_tools")
                .and_then(Value::as_object)
                .and_then(|allowed_tools| allowed_tools.get("tools"))
                .or_else(|| choice_obj.get("tools"))
                .and_then(Value::as_array)
            else {
                return Ok(());
            };

            for tool in selected_tools {
                if tool.get("type").and_then(Value::as_str) != Some("function") {
                    continue;
                }
                if let Some(name) = openai_named_tool_choice_name(tool, "function") {
                    reject_reserved_custom_bridge_name(name)?;
                }
            }
        }
        _ => {}
    }

    Ok(())
}

fn validate_responses_request_for_custom_bridge(body: &Value) -> Result<(), String> {
    for tool in normalized_responses_tool_definitions_from_request(body)? {
        if let NormalizedOpenAiFamilyToolDef::Function(function) = tool {
            reject_reserved_custom_bridge_name(&function.name)?;
        }
    }

    if let Some(items) = body.get("input").and_then(Value::as_array) {
        for item in items {
            let Some(tool_call) = normalized_responses_tool_call(item)? else {
                continue;
            };
            if let NormalizedOpenAiFamilyToolCall::Function { name, .. } = tool_call {
                reject_reserved_custom_bridge_name(&name)?;
            }
        }
    }

    if let Some(tool_choice) = body.get("tool_choice") {
        validate_responses_tool_choice_for_custom_bridge(tool_choice)?;
    }

    Ok(())
}

pub(super) fn responses_to_messages(
    body: &mut Value,
    target_format: UpstreamFormat,
) -> Result<(), String> {
    let bridge_custom_responses_semantics = target_format == UpstreamFormat::OpenAiCompletion;
    if bridge_custom_responses_semantics {
        validate_responses_request_for_custom_bridge(body)?;
    }
    let controls = responses_normalized_request_controls(body)?;
    let profile = shared_control_profile_for_target(target_format);
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
    let mut tool_kind_by_call_id = std::collections::HashMap::new();
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
                let refusal = extract_responses_refusal_text(item.get("content"));
                let content = map_responses_content_to_openai(item.get("content").cloned());
                if role == "assistant" {
                    let assistant = current_assistant.get_or_insert_with(|| {
                        serde_json::json!({
                            "role": "assistant",
                            "content": Value::Null
                        })
                    });
                    assistant["role"] = Value::String("assistant".to_string());
                    if !refusal.is_empty() {
                        assistant["refusal"] = Value::String(refusal);
                    }
                    assistant["content"] = if content_value_is_effectively_empty(&content)
                        && assistant.get("refusal").is_some()
                    {
                        Value::Null
                    } else {
                        content
                    };
                } else if role == "user"
                    && items
                        .get(idx + 1)
                        .is_some_and(responses_item_is_tool_output)
                {
                    deferred_user_after_tool_results =
                        Some(serde_json::json!({ "role": "user", "content": content }));
                } else {
                    flush_assistant(&mut messages, &mut current_assistant);
                    messages.push(serde_json::json!({ "role": role, "content": content }));
                }
            }
            "function_call" | "custom_tool_call" => {
                let tc = if bridge_custom_responses_semantics {
                    responses_tool_call_item_to_openai_tool_call_with_custom_bridge_strict(
                        &item,
                        translation_target_label(target_format),
                    )?
                } else {
                    responses_tool_call_item_to_openai_tool_call_strict(
                        &item,
                        translation_target_label(target_format),
                    )?
                };
                let Some(tc) = tc else {
                    idx += 1;
                    continue;
                };
                if let Some(call_id) = item.get("call_id").and_then(Value::as_str) {
                    tool_kind_by_call_id
                        .insert(call_id.to_string(), semantic_tool_kind_from_value(&item));
                }
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
            "function_call_output" | "custom_tool_call_output" => {
                flush_assistant(&mut messages, &mut current_assistant);
                let call_id = item.get("call_id").cloned();
                let tool_kind = item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .and_then(|call_id| tool_kind_by_call_id.get(call_id).copied())
                    .unwrap_or_else(|| semantic_tool_kind_from_value(&item));
                if tool_kind == SemanticToolKind::OpenAiCustom
                    && target_format != UpstreamFormat::OpenAiCompletion
                {
                    return Err(custom_tools_not_portable_message(target_format));
                }
                messages.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": call_id,
                    "content": responses_tool_output_to_openai_tool_content(
                        item.get("output"),
                        target_format,
                    )?
                }));
                let next_is_function_output = items
                    .get(idx + 1)
                    .is_some_and(responses_item_is_tool_output);
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
                let replay_blocks = if target_format == UpstreamFormat::Anthropic {
                    item.get("encrypted_content")
                        .and_then(Value::as_str)
                        .map(decode_anthropic_reasoning_carrier)
                        .transpose()
                        .map_err(|err| {
                            format!(
                                "OpenAI Responses reasoning encrypted_content cannot be replayed to Anthropic: {err}"
                            )
                        })?
                } else {
                    None
                };
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
                if let Some(blocks) = replay_blocks {
                    if current_assistant.is_none() {
                        current_assistant = Some(serde_json::json!({
                            "role": "assistant",
                            "content": null
                        }));
                    }
                    if let Some(ref mut a) = current_assistant {
                        append_openai_message_anthropic_reasoning_replay_blocks(a, blocks);
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
    let tools = body.get("tools").cloned();
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
    if profile.top_logprobs {
        if let Some(logprobs) = controls.logprobs.as_ref() {
            if logprobs.enabled || logprobs.top_logprobs.is_some() {
                out.insert("logprobs".to_string(), Value::Bool(true));
            }
        }
    }
    if let Some(tool_choice) = body.get("tool_choice").cloned() {
        if let Some(mapped_tool_choice) = responses_tool_choice_to_openai_tool_choice(
            &tool_choice,
            bridge_custom_responses_semantics,
        ) {
            out.insert("tool_choice".to_string(), mapped_tool_choice);
        }
    }
    if profile.parallel_tool_calls {
        if let Some(parallel_tool_calls) = controls.parallel_tool_calls.as_ref() {
            out.insert(
                "parallel_tool_calls".to_string(),
                parallel_tool_calls.clone(),
            );
        }
    }
    if profile.metadata {
        if let Some(metadata) = controls.metadata.as_ref() {
            out.insert("metadata".to_string(), metadata.clone());
        }
    }
    if profile.user {
        if let Some(user) = controls.user.as_ref() {
            out.insert("user".to_string(), user.clone());
        }
    }
    if profile.service_tier {
        if let Some(service_tier) = controls.service_tier.as_ref() {
            out.insert("service_tier".to_string(), service_tier.clone());
        }
    }
    if profile.stream_include_obfuscation {
        if let Some(include_obfuscation) = controls.stream_include_obfuscation.as_ref() {
            insert_stream_include_obfuscation(&mut out, include_obfuscation.clone());
        }
    }
    if profile.verbosity {
        if let Some(verbosity) = controls.verbosity.as_ref() {
            out.insert("verbosity".to_string(), verbosity.clone());
        }
    }
    if profile.reasoning_effort {
        if let Some(reasoning_effort) = controls.reasoning_effort.as_ref() {
            out.insert("reasoning_effort".to_string(), reasoning_effort.clone());
        }
    }
    if profile.prompt_cache_key {
        if let Some(prompt_cache_key) = controls.prompt_cache_key.as_ref() {
            out.insert("prompt_cache_key".to_string(), prompt_cache_key.clone());
        }
    }
    if profile.prompt_cache_retention {
        if let Some(prompt_cache_retention) = controls.prompt_cache_retention.as_ref() {
            out.insert(
                "prompt_cache_retention".to_string(),
                prompt_cache_retention.clone(),
            );
        }
    }
    if profile.safety_identifier {
        if let Some(safety_identifier) = controls.safety_identifier.as_ref() {
            out.insert("safety_identifier".to_string(), safety_identifier.clone());
        }
    }
    if profile.top_logprobs {
        if let Some(top_logprobs) = controls
            .logprobs
            .as_ref()
            .and_then(|logprobs| logprobs.top_logprobs.as_ref())
        {
            out.insert("top_logprobs".to_string(), top_logprobs.clone());
        }
    }
    if let Some(output_shape) = controls.output_shape.as_ref() {
        out.insert(
            "response_format".to_string(),
            normalized_output_shape_to_openai_response_format(output_shape),
        );
    }

    // Convert tools from Responses API format to Chat Completions format.
    // The OpenAICompletion bridge rewrites Responses custom tools into synthetic function tools.
    if tools.is_some() {
        let converted_tools = normalized_responses_tool_definitions_from_request(body)?
            .into_iter()
            .map(|tool| {
                if bridge_custom_responses_semantics {
                    normalized_tool_definition_to_openai_with_custom_bridge(&tool)
                } else {
                    normalized_tool_definition_to_openai(&tool)
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
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
                let annotations = c
                    .get("annotations")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if annotations.is_empty() {
                    plain_text_parts.push(text.clone());
                } else {
                    has_non_text_part = true;
                }
                return semantic_text_part_to_openai_value(&SemanticTextPart { text, annotations });
            }
            if ty == Some("refusal") {
                return Value::Null;
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
    Value::Array(out.into_iter().filter(|item| !item.is_null()).collect())
}

pub(super) fn messages_to_responses(body: &mut Value) -> Result<(), String> {
    let controls = openai_normalized_request_controls(body)?;
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .ok_or("missing messages")?;
    let mut input: Vec<Value> = vec![];
    let mut tool_kind_by_call_id = std::collections::HashMap::new();
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
            let content_type = if role == "assistant" {
                "output_text"
            } else {
                "input_text"
            };
            let content_arr = openai_message_to_responses_content(msg, content_type);
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
                    if let Some(call_id) = tc.get("id").and_then(Value::as_str) {
                        tool_kind_by_call_id
                            .insert(call_id.to_string(), semantic_tool_kind_from_value(tc));
                    }
                    input.push(openai_tool_call_to_responses_item(tc));
                }
            }
        }
        if role == "tool" {
            let tool_kind = msg
                .get("tool_call_id")
                .and_then(Value::as_str)
                .and_then(|call_id| tool_kind_by_call_id.get(call_id).copied())
                .unwrap_or_else(|| semantic_tool_kind_from_value(msg));
            input.push(serde_json::json!({
                "type": semantic_tool_output_item_type(tool_kind),
                "call_id": msg.get("tool_call_id"),
                "output": openai_tool_result_content_to_responses_output(msg.get("content"))?
            }));
        }
    }
    body["input"] = Value::Array(input);
    let normalized_tools = normalized_openai_tool_definitions_from_request(body)?;
    if !normalized_tools.is_empty() {
        body["tools"] = Value::Array(
            normalized_tools
                .iter()
                .map(normalized_tool_definition_to_responses)
                .collect(),
        );
    }
    if let Some(tool_policy) = controls.tool_policy.as_ref() {
        body["tool_choice"] = normalized_tool_policy_to_responses_tool_choice(
            tool_policy,
            controls.restricted_tool_names.as_deref(),
        );
    } else if let Some(tool_choice) = body.get("tool_choice").cloned() {
        if let Some(mapped_tool_choice) = openai_tool_choice_to_responses_tool_choice(&tool_choice)
        {
            body["tool_choice"] = mapped_tool_choice;
        } else if let Some(obj) = body.as_object_mut() {
            obj.remove("tool_choice");
        }
    }
    if let Some(parallel_tool_calls) = controls.parallel_tool_calls.as_ref() {
        body["parallel_tool_calls"] = parallel_tool_calls.clone();
    }
    if let Some(max_output_tokens) = body
        .get("max_completion_tokens")
        .cloned()
        .or_else(|| body.get("max_tokens").cloned())
    {
        body["max_output_tokens"] = max_output_tokens;
    }
    if let Some(output_shape) = controls.output_shape.as_ref() {
        body["text"] = serde_json::json!({
            "format": normalized_output_shape_to_responses_text_format(output_shape)
        });
    }
    if let Some(metadata) = controls.metadata.as_ref() {
        body["metadata"] = metadata.clone();
    }
    if let Some(user) = controls.user.as_ref() {
        body["user"] = user.clone();
    }
    if let Some(service_tier) = controls.service_tier.as_ref() {
        body["service_tier"] = service_tier.clone();
    }
    if let Some(include_obfuscation) = controls.stream_include_obfuscation.as_ref() {
        if let Some(obj) = body.as_object_mut() {
            insert_stream_include_obfuscation(obj, include_obfuscation.clone());
        }
    }
    if let Some(verbosity) = controls.verbosity.as_ref() {
        if let Some(obj) = body.as_object_mut() {
            insert_responses_text_verbosity(obj, verbosity.clone());
        }
    }
    if let Some(reasoning_effort) = controls.reasoning_effort.as_ref() {
        if let Some(obj) = body.as_object_mut() {
            insert_responses_reasoning_effort(obj, reasoning_effort.clone());
        }
    }
    if let Some(logprobs) = controls.logprobs.as_ref() {
        if logprobs.enabled || logprobs.top_logprobs.is_some() {
            if let Some(obj) = body.as_object_mut() {
                insert_responses_include_item(
                    obj,
                    Value::String("message.output_text.logprobs".to_string()),
                );
            }
        }
    }
    if let Some(prompt_cache_key) = controls.prompt_cache_key.as_ref() {
        body["prompt_cache_key"] = prompt_cache_key.clone();
    }
    if let Some(prompt_cache_retention) = controls.prompt_cache_retention.as_ref() {
        body["prompt_cache_retention"] = prompt_cache_retention.clone();
    }
    if let Some(safety_identifier) = controls.safety_identifier.as_ref() {
        body["safety_identifier"] = safety_identifier.clone();
    }
    if let Some(top_logprobs) = controls
        .logprobs
        .as_ref()
        .and_then(|logprobs| logprobs.top_logprobs.as_ref())
    {
        body["top_logprobs"] = top_logprobs.clone();
    }
    if let Some(obj) = body.as_object_mut() {
        obj.remove("instructions");
        obj.remove("messages");
        obj.remove("max_completion_tokens");
        obj.remove("max_tokens");
        obj.remove("response_format");
        obj.remove("stop");
        obj.remove("seed");
        obj.remove("presence_penalty");
        obj.remove("frequency_penalty");
        obj.remove("logprobs");
        obj.remove("logit_bias");
        obj.remove("allowed_tool_names");
        obj.remove("verbosity");
        obj.remove("reasoning_effort");
        obj.remove("prediction");
        obj.remove("web_search_options");
        obj.remove("n");
        obj.remove("store");
        obj.remove("modalities");
        obj.remove("audio");
    }
    Ok(())
}

fn responses_tool_choice_to_openai_tool_choice(
    choice: &Value,
    bridge_custom_responses_semantics: bool,
) -> Option<Value> {
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
        "custom" => {
            let name = openai_named_tool_choice_name(choice, "custom")?;
            if bridge_custom_responses_semantics {
                Some(serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": openai_responses_custom_tool_bridge_name(name)
                    }
                }))
            } else {
                Some(serde_json::json!({
                    "type": "custom",
                    "custom": { "name": name }
                }))
            }
        }
        "allowed_tools" => {
            let mode = obj.get("mode")?;
            let tools = obj.get("tools")?.as_array()?;
            let converted_tools = tools
                .iter()
                .map(|tool| match tool.get("type").and_then(Value::as_str) {
                    Some("function") if tool.get("name").is_some() => serde_json::json!({
                        "type": "function",
                        "function": { "name": tool.get("name").cloned().unwrap_or(Value::Null) }
                    }),
                    Some("custom") if bridge_custom_responses_semantics => tool
                        .get("name")
                        .and_then(Value::as_str)
                        .map(|name| {
                            serde_json::json!({
                                "type": "function",
                                "function": {
                                    "name": openai_responses_custom_tool_bridge_name(name)
                                }
                            })
                        })
                        .unwrap_or_else(|| tool.clone()),
                    Some("custom") if tool.get("name").is_some() => serde_json::json!({
                        "type": "custom",
                        "custom": { "name": tool.get("name").cloned().unwrap_or(Value::Null) }
                    }),
                    _ => tool.clone(),
                })
                .collect::<Vec<_>>();
            Some(serde_json::json!({
                "type": "allowed_tools",
                "allowed_tools": {
                    "mode": mode,
                    "tools": converted_tools
                }
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
        "custom" => {
            let name = openai_named_tool_choice_name(choice, "custom")?;
            Some(serde_json::json!({
                "type": "custom",
                "name": name
            }))
        }
        "allowed_tools" => {
            let allowed_tools = obj.get("allowed_tools")?.as_object()?;
            let mode = allowed_tools.get("mode")?;
            let tools = allowed_tools.get("tools")?.as_array()?;
            let converted_tools = tools
                .iter()
                .map(|tool| {
                    match tool.get("type").and_then(Value::as_str) {
                        Some("function") => {
                            if let Some(name) = tool.get("name").or_else(|| {
                                tool.get("function")
                                    .and_then(|function| function.get("name"))
                            }) {
                                return serde_json::json!({
                                    "type": "function",
                                    "name": name
                                });
                            }
                        }
                        Some("custom") => {
                            if let Some(name) = tool.get("name").or_else(|| {
                                tool.get("custom").and_then(|custom| custom.get("name"))
                            }) {
                                return serde_json::json!({
                                    "type": "custom",
                                    "name": name
                                });
                            }
                        }
                        _ => {}
                    }
                    tool.clone()
                })
                .collect::<Vec<_>>();
            Some(serde_json::json!({
                "type": "allowed_tools",
                "mode": mode,
                "tools": converted_tools
            }))
        }
        _ => None,
    }
}

fn normalized_tool_policy_to_responses_tool_choice(
    tool_policy: &NormalizedToolPolicy,
    restricted_tool_names: Option<&[String]>,
) -> Value {
    match tool_policy {
        NormalizedToolPolicy::Auto => {
            if let Some(names) = restricted_tool_names {
                serde_json::json!({
                    "type": "allowed_tools",
                    "mode": "auto",
                    "tools": names
                        .iter()
                        .map(|name| serde_json::json!({ "type": "function", "name": name }))
                        .collect::<Vec<_>>()
                })
            } else {
                serde_json::json!("auto")
            }
        }
        NormalizedToolPolicy::None => serde_json::json!("none"),
        NormalizedToolPolicy::Required => {
            if let Some([name]) = restricted_tool_names {
                serde_json::json!({
                    "type": "function",
                    "name": name
                })
            } else if let Some(names) = restricted_tool_names {
                serde_json::json!({
                    "type": "allowed_tools",
                    "mode": "required",
                    "tools": names
                        .iter()
                        .map(|name| serde_json::json!({ "type": "function", "name": name }))
                        .collect::<Vec<_>>()
                })
            } else {
                serde_json::json!("required")
            }
        }
        NormalizedToolPolicy::ForcedFunction(name) => serde_json::json!({
            "type": "function",
            "name": name
        }),
    }
}

fn insert_stream_include_obfuscation(
    object: &mut serde_json::Map<String, Value>,
    include_obfuscation: Value,
) {
    let stream_options = object
        .entry("stream_options".to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if let Some(stream_options) = stream_options.as_object_mut() {
        stream_options.insert("include_obfuscation".to_string(), include_obfuscation);
    }
}

fn insert_responses_include_item(object: &mut serde_json::Map<String, Value>, item: Value) {
    let include = object
        .entry("include".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Some(include) = include.as_array_mut() {
        if !include.iter().any(|existing| existing == &item) {
            include.push(item);
        }
    }
}

fn insert_responses_text_verbosity(object: &mut serde_json::Map<String, Value>, verbosity: Value) {
    let text = object
        .entry("text".to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if let Some(text) = text.as_object_mut() {
        text.insert("verbosity".to_string(), verbosity);
    }
}

fn insert_responses_reasoning_effort(
    object: &mut serde_json::Map<String, Value>,
    reasoning_effort: Value,
) {
    let reasoning = object
        .entry("reasoning".to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if let Some(reasoning) = reasoning.as_object_mut() {
        reasoning.insert("effort".to_string(), reasoning_effort);
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
                let text = c.get("text").and_then(Value::as_str).unwrap_or("").to_string();
                let annotations = c
                    .get("annotations")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                return semantic_text_part_to_responses_value(
                    &SemanticTextPart { text, annotations },
                    content_type,
                );
            }
            if ty == Some("refusal") {
                return serde_json::json!({
                    "type": "refusal",
                    "refusal": c.get("refusal").cloned().unwrap_or_else(|| Value::String(String::new()))
                });
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

fn responses_usage_to_openai_usage(usage: &Value) -> Value {
    let input_tokens = usage
        .get("input_tokens")
        .or(usage.get("prompt_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage
        .get("output_tokens")
        .or(usage.get("completion_tokens"))
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

    if let Some(details) = clone_usage_details_object(
        usage
            .get("input_tokens_details")
            .or(usage.get("prompt_tokens_details")),
    ) {
        mapped["prompt_tokens_details"] = details;
    }
    if let Some(details) = clone_usage_details_object(
        usage
            .get("output_tokens_details")
            .or(usage.get("completion_tokens_details")),
    ) {
        mapped["completion_tokens_details"] = details;
    }

    copy_remaining_usage_fields(
        usage,
        &mut mapped,
        &[
            "input_tokens",
            "prompt_tokens",
            "output_tokens",
            "completion_tokens",
            "total_tokens",
            "input_tokens_details",
            "prompt_tokens_details",
            "output_tokens_details",
            "completion_tokens_details",
        ],
    );

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

    if let Some(details) = clone_usage_details_object(usage.get("prompt_tokens_details")) {
        mapped["input_tokens_details"] = details;
    }
    if let Some(details) = clone_usage_details_object(usage.get("completion_tokens_details")) {
        mapped["output_tokens_details"] = details;
    }

    copy_remaining_usage_fields(
        usage,
        &mut mapped,
        &[
            "prompt_tokens",
            "completion_tokens",
            "total_tokens",
            "prompt_tokens_details",
            "completion_tokens_details",
        ],
    );

    mapped
}
