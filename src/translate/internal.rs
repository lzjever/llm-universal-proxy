//! Request/response translation between formats (pivot: OpenAI Chat Completions).
//!
//! Reference: 9router open-sse/translator/index.js — source → openai → target.

use serde_json::{Map, Value};

use crate::config::{CompatibilityMode, ModelSurface};
use crate::formats::UpstreamFormat;

pub(crate) mod assessment;
pub(crate) mod messages;
pub(crate) mod models;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RequestTranslationPolicy {
    pub(crate) compatibility_mode: CompatibilityMode,
    pub(crate) surface: ModelSurface,
}

impl RequestTranslationPolicy {
    pub(crate) fn is_empty(&self) -> bool {
        !self.has_native_request_policy_hooks()
            && self.compatibility_mode == CompatibilityMode::default()
    }

    fn max_output_tokens(&self) -> Option<u64> {
        self.surface
            .limits
            .as_ref()
            .and_then(|limits| limits.max_output_tokens)
    }

    fn disables_parallel_tool_calls(&self) -> bool {
        self.surface
            .tools
            .as_ref()
            .and_then(|tools| tools.supports_parallel_calls)
            == Some(false)
    }

    fn has_native_request_policy_hooks(&self) -> bool {
        self.max_output_tokens().is_some() || self.disables_parallel_tool_calls()
    }
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
    translate_request_with_policy(
        client_format,
        upstream_format,
        model,
        body,
        RequestTranslationPolicy::default(),
        stream,
    )
}

pub fn translate_request_with_policy(
    client_format: UpstreamFormat,
    upstream_format: UpstreamFormat,
    model: &str,
    body: &mut Value,
    policy: RequestTranslationPolicy,
    stream: bool,
) -> Result<(), String> {
    if let TranslationDecision::Reject(message) =
        assessment::assess_request_translation_with_surface(
            client_format,
            upstream_format,
            body,
            policy.compatibility_mode,
            &policy.surface,
        )
        .decision()
    {
        return Err(message);
    }

    if client_format == upstream_format {
        if stream {
            normalize_openai_roles_for_compatibility(client_format, body);
        }
        if client_format == UpstreamFormat::OpenAiCompletion {
            apply_openai_completion_compat_overrides(model, body);
        }
        apply_request_translation_policy_defaults(upstream_format, &policy, body);
        apply_request_translation_policy_bridge_context(&policy, body);
        return Ok(());
    }
    let translated_from_openai_completion = client_format == UpstreamFormat::OpenAiCompletion;
    // Step 1: client → openai (if client is not openai)
    if client_format != UpstreamFormat::OpenAiCompletion {
        client_to_openai_completion(client_format, upstream_format, body)?;
    }
    apply_request_translation_policy_defaults(UpstreamFormat::OpenAiCompletion, &policy, body);
    apply_max_compat_structural_repair_pass(policy.compatibility_mode, upstream_format, body)?;
    // Step 2: openai → upstream (if upstream is not openai)
    if upstream_format != UpstreamFormat::OpenAiCompletion {
        openai_completion_to_upstream(upstream_format, model, body)?;
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
        if stream {
            normalize_openai_roles_for_compatibility(upstream_format, body);
        }
        apply_openai_completion_compat_overrides(model, body);
    }
    apply_request_translation_policy_defaults(upstream_format, &policy, body);
    apply_request_translation_policy_bridge_context(&policy, body);
    Ok(())
}

fn apply_request_translation_policy_bridge_context(
    policy: &RequestTranslationPolicy,
    body: &mut Value,
) {
    let Some(bridge_context) = body.get_mut(REQUEST_SCOPED_TOOL_BRIDGE_CONTEXT_FIELD) else {
        return;
    };
    let Some(bridge_context_obj) = bridge_context.as_object_mut() else {
        return;
    };
    let compatibility_mode = match policy.compatibility_mode {
        CompatibilityMode::Strict => "strict",
        CompatibilityMode::Balanced => "balanced",
        CompatibilityMode::MaxCompat => "max_compat",
    };
    bridge_context_obj.insert(
        "compatibility_mode".to_string(),
        Value::String(compatibility_mode.to_string()),
    );
}

fn apply_max_compat_structural_repair_pass(
    compatibility_mode: CompatibilityMode,
    target_format: UpstreamFormat,
    body: &mut Value,
) -> Result<(), String> {
    if compatibility_mode != CompatibilityMode::MaxCompat {
        return Ok(());
    }
    let bridge_context = body
        .get(REQUEST_SCOPED_TOOL_BRIDGE_CONTEXT_FIELD)
        .cloned()
        .filter(Value::is_object);
    let Some(bridge_context) = bridge_context.as_ref() else {
        return Ok(());
    };

    repair_request_scoped_custom_bridge_tool_definitions(target_format, body, bridge_context)?;
    repair_request_scoped_custom_bridge_tool_choice(body, bridge_context);
    repair_request_scoped_custom_bridge_message_tool_calls(body, bridge_context)?;
    Ok(())
}

fn repair_request_scoped_custom_bridge_tool_definitions(
    target_format: UpstreamFormat,
    body: &mut Value,
    bridge_context: &Value,
) -> Result<(), String> {
    let Some(original_tools) = body.get("tools").and_then(Value::as_array) else {
        return Ok(());
    };
    let normalized =
        normalized_openai_tool_definitions_from_request_with_request_scoped_custom_bridge(
            body,
            Some(bridge_context),
        )?;
    if normalized.len() != original_tools.len() {
        return Ok(());
    }
    let repaired = normalized
        .iter()
        .map(|tool| {
            normalized_tool_definition_to_openai_with_request_scoped_custom_bridge(
                tool,
                target_format,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(obj) = body.as_object_mut() {
        obj.insert("tools".to_string(), Value::Array(repaired));
    }
    Ok(())
}

fn request_scoped_bridge_choice_name(choice: &Value) -> Option<&str> {
    choice
        .get("name")
        .or_else(|| choice.get("custom").and_then(|custom| custom.get("name")))
        .or_else(|| {
            choice
                .get("function")
                .and_then(|function| function.get("name"))
        })
        .and_then(Value::as_str)
}

fn repair_request_scoped_custom_bridge_tool_choice(body: &mut Value, bridge_context: &Value) {
    let Some(choice) = body.get("tool_choice").cloned() else {
        return;
    };
    let Some(choice_obj) = choice.as_object() else {
        return;
    };
    let Some(choice_type) = choice_obj.get("type").and_then(Value::as_str) else {
        return;
    };

    let repaired = match choice_type {
        "custom" => request_scoped_bridge_choice_name(&choice)
            .filter(|name| {
                request_scoped_openai_custom_bridge_expects_canonical_input_wrapper(
                    Some(bridge_context),
                    name,
                )
            })
            .map(|name| {
                serde_json::json!({
                    "type": "function",
                    "function": { "name": name }
                })
            }),
        "allowed_tools" => choice_obj
            .get("allowed_tools")
            .and_then(Value::as_object)
            .and_then(|allowed_tools| {
                let mode = allowed_tools.get("mode")?;
                let tools = allowed_tools.get("tools")?.as_array()?;
                Some(serde_json::json!({
                    "type": "allowed_tools",
                    "allowed_tools": {
                        "mode": mode,
                        "tools": tools
                            .iter()
                            .map(|tool| {
                                let Some(tool_type) = tool.get("type").and_then(Value::as_str) else {
                                    return tool.clone();
                                };
                                if tool_type != "custom" {
                                    return tool.clone();
                                }
                                request_scoped_bridge_choice_name(tool)
                                    .filter(|name| {
                                        request_scoped_openai_custom_bridge_expects_canonical_input_wrapper(
                                            Some(bridge_context),
                                            name,
                                        )
                                    })
                                    .map(|name| {
                                        serde_json::json!({
                                            "type": "function",
                                            "function": { "name": name }
                                        })
                                    })
                                    .unwrap_or_else(|| tool.clone())
                            })
                            .collect::<Vec<_>>()
                    }
                }))
            }),
        _ => None,
    };

    if let Some(repaired) = repaired {
        body["tool_choice"] = repaired;
    }
}

fn repair_request_scoped_custom_bridge_message_tool_calls(
    body: &mut Value,
    bridge_context: &Value,
) -> Result<(), String> {
    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return Ok(());
    };
    for message in messages {
        let Some(tool_calls) = message.get_mut("tool_calls").and_then(Value::as_array_mut) else {
            continue;
        };
        for tool_call in tool_calls.iter_mut() {
            let Some(NormalizedOpenAiFamilyToolCall::Custom {
                id,
                name,
                input,
                namespace,
                proxied_tool_kind,
            }) = normalized_openai_tool_call(tool_call)?
            else {
                continue;
            };
            if namespace.is_some()
                || !request_scoped_openai_custom_bridge_expects_canonical_input_wrapper(
                    Some(bridge_context),
                    &name,
                )
            {
                continue;
            }
            let mut repaired = serde_json::json!({
                "id": id,
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": openai_responses_custom_tool_bridge_arguments(&input)?
                }
            });
            if let Some(proxied_tool_kind) = proxied_tool_kind {
                repaired["proxied_tool_kind"] = proxied_tool_kind;
            }
            copy_non_replayable_tool_call_marker(tool_call, &mut repaired);
            *tool_call = repaired;
        }
    }
    Ok(())
}

fn apply_request_translation_policy_defaults(
    target_format: UpstreamFormat,
    policy: &RequestTranslationPolicy,
    body: &mut Value,
) {
    apply_request_translation_policy_default_output_limit(target_format, policy, body);
    apply_request_translation_policy_parallel_tool_gate(target_format, policy, body);
}

fn apply_request_translation_policy_default_output_limit(
    target_format: UpstreamFormat,
    policy: &RequestTranslationPolicy,
    body: &mut Value,
) {
    let Some(max_output_tokens) = policy.max_output_tokens() else {
        return;
    };
    if request_body_has_explicit_output_limit(target_format, body) {
        return;
    }
    let Some(obj) = body.as_object_mut() else {
        return;
    };

    match target_format {
        UpstreamFormat::Anthropic => {
            obj.insert("max_tokens".to_string(), Value::from(max_output_tokens));
        }
        UpstreamFormat::OpenAiCompletion => {
            obj.insert(
                "max_completion_tokens".to_string(),
                Value::from(max_output_tokens),
            );
        }
        UpstreamFormat::OpenAiResponses => {
            obj.insert(
                "max_output_tokens".to_string(),
                Value::from(max_output_tokens),
            );
        }
        UpstreamFormat::Google => {
            let generation_config = obj
                .entry("generationConfig".to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            if !generation_config.is_object() {
                *generation_config = Value::Object(Map::new());
            }
            if let Some(generation_config_obj) = generation_config.as_object_mut() {
                generation_config_obj.insert(
                    "maxOutputTokens".to_string(),
                    Value::from(max_output_tokens),
                );
            }
        }
    }
}

fn apply_request_translation_policy_parallel_tool_gate(
    target_format: UpstreamFormat,
    policy: &RequestTranslationPolicy,
    body: &mut Value,
) {
    if !policy.disables_parallel_tool_calls()
        || request_body_has_explicit_parallel_tool_calls_preference(target_format, body)
        || !request_body_has_tool_definitions(target_format, body)
    {
        return;
    }

    match target_format {
        UpstreamFormat::OpenAiCompletion | UpstreamFormat::OpenAiResponses => {
            if let Some(obj) = body.as_object_mut() {
                obj.insert("parallel_tool_calls".to_string(), Value::Bool(false));
            }
        }
        UpstreamFormat::Anthropic => {
            let Some(obj) = body.as_object_mut() else {
                return;
            };
            let tool_choice = obj
                .entry("tool_choice".to_string())
                .or_insert_with(|| serde_json::json!({ "type": "auto" }));
            let Some(tool_choice_obj) = tool_choice.as_object_mut() else {
                return;
            };
            if tool_choice_obj.get("type").and_then(Value::as_str) == Some("none") {
                return;
            }
            tool_choice_obj
                .entry("disable_parallel_tool_use".to_string())
                .or_insert(Value::Bool(true));
        }
        UpstreamFormat::Google => {}
    }
}

fn request_body_has_explicit_output_limit(target_format: UpstreamFormat, body: &Value) -> bool {
    let Some(obj) = body.as_object() else {
        return false;
    };

    match target_format {
        UpstreamFormat::Anthropic => obj.get("max_tokens").is_some(),
        UpstreamFormat::OpenAiCompletion => {
            obj.get("max_completion_tokens").is_some() || obj.get("max_tokens").is_some()
        }
        UpstreamFormat::OpenAiResponses => obj.get("max_output_tokens").is_some(),
        UpstreamFormat::Google => {
            google_generation_config_has_output_limit(obj.get("generationConfig"))
                || google_generation_config_has_output_limit(obj.get("generation_config"))
        }
    }
}

fn google_generation_config_has_output_limit(config: Option<&Value>) -> bool {
    config.and_then(Value::as_object).is_some_and(|config| {
        config.get("maxOutputTokens").is_some() || config.get("max_output_tokens").is_some()
    })
}

fn request_body_has_explicit_parallel_tool_calls_preference(
    target_format: UpstreamFormat,
    body: &Value,
) -> bool {
    match target_format {
        UpstreamFormat::OpenAiCompletion | UpstreamFormat::OpenAiResponses => body
            .get("parallel_tool_calls")
            .and_then(Value::as_bool)
            .is_some(),
        UpstreamFormat::Anthropic => body
            .get("tool_choice")
            .and_then(Value::as_object)
            .and_then(|tool_choice| tool_choice.get("disable_parallel_tool_use"))
            .and_then(Value::as_bool)
            .is_some(),
        UpstreamFormat::Google => false,
    }
}

fn request_body_has_tool_definitions(target_format: UpstreamFormat, body: &Value) -> bool {
    match target_format {
        UpstreamFormat::OpenAiCompletion
        | UpstreamFormat::OpenAiResponses
        | UpstreamFormat::Anthropic => body
            .get("tools")
            .and_then(Value::as_array)
            .is_some_and(|tools| !tools.is_empty()),
        UpstreamFormat::Google => body
            .get("tools")
            .and_then(Value::as_array)
            .is_some_and(|tools| !tools.is_empty()),
    }
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
        let Some(role) = openai_string_message_role_if_coalescible(&message) else {
            coalesced.push(message);
            continue;
        };
        let Some(content) = message
            .get("content")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            coalesced.push(message);
            continue;
        };

        if let Some(previous) = coalesced.last_mut() {
            let previous_role = openai_string_message_role_if_coalescible(previous);
            let previous_content = previous.get("content").and_then(Value::as_str);
            if previous_role == Some(role) {
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

fn openai_string_message_role_if_coalescible(message: &Value) -> Option<&str> {
    let role = message.get("role").and_then(Value::as_str)?;
    message.get("content").and_then(Value::as_str)?;
    let object = message.as_object()?;
    let has_only_role_and_content = object.keys().all(|key| key == "role" || key == "content");
    has_only_role_and_content.then_some(role)
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

fn client_to_openai_completion(
    from: UpstreamFormat,
    target_format: UpstreamFormat,
    body: &mut Value,
) -> Result<(), String> {
    match from {
        UpstreamFormat::OpenAiCompletion => {}
        UpstreamFormat::OpenAiResponses => {
            responses_to_messages(body, target_format)?;
        }
        UpstreamFormat::Anthropic => {
            claude_to_openai(body, target_format == UpstreamFormat::OpenAiResponses)?;
        }
        UpstreamFormat::Google => {
            gemini_to_openai(body)?;
        }
    }
    Ok(())
}

fn openai_completion_to_upstream(
    to: UpstreamFormat,
    target_model: &str,
    body: &mut Value,
) -> Result<(), String> {
    match to {
        UpstreamFormat::OpenAiCompletion => {}
        UpstreamFormat::OpenAiResponses => {
            messages_to_responses(body)?;
        }
        UpstreamFormat::Anthropic => {
            openai_to_claude(body)?;
        }
        UpstreamFormat::Google => {
            openai_to_gemini(body, target_model)?;
        }
    }
    Ok(())
}
mod openai_family;
mod openai_responses;
#[cfg(test)]
mod regression_tests;
mod request_gemini;
mod response_logprobs;
pub(crate) mod response_protocols;
pub(crate) mod tools;

use messages::{
    anthropic_request_tool_definition_not_portable_message,
    anthropic_tool_result_order_not_portable_message, custom_tools_not_portable_message,
    gemini_function_response_parts_not_portable_message,
    openai_assistant_audio_not_portable_message, single_optional_array_item,
    single_required_array_item, translation_target_label,
};
use models::{
    NormalizedOpenAiFamilyToolCall, NormalizedToolPolicy, SemanticToolKind, TranslationDecision,
};
use openai_family::{
    collapse_openai_text_parts, copy_remaining_usage_fields, extract_openai_content_text,
    extract_openai_refusal, extract_responses_text_content, openai_normalized_request_controls,
    openai_response_has_assistant_audio,
};
use openai_responses::{
    append_openai_message_anthropic_reasoning_replay_blocks, messages_to_responses,
    openai_message_anthropic_reasoning_replay_blocks, openai_response_to_responses,
    responses_response_to_openai, responses_response_to_openai_for_anthropic,
    responses_to_messages,
};
#[cfg(test)]
use request_gemini::convert_gemini_content_to_openai;
use request_gemini::{
    gemini_function_declaration_output_schema_field,
    gemini_function_output_schema_not_portable_message,
    gemini_function_response_has_nonportable_parts, gemini_openai_function_tools_from_request,
    gemini_part_field, gemini_request_function_calling_config_from_object,
    gemini_request_nonportable_output_shape_message, gemini_request_tool_config,
    gemini_request_tools, gemini_to_openai, gemini_tool_function_declarations,
    gemini_validated_allowed_function_names, normalized_output_shape_to_claude_output_config,
    openai_content_to_gemini_parts, openai_portable_function_tools, openai_to_gemini,
};
use response_logprobs::{
    normalized_response_logprobs_from_openai_choice, normalized_response_logprobs_to_gemini_fields,
};
use response_protocols::{
    gemini_response_to_openai, is_minimax_model, normalize_openai_completion_response,
    openai_finish_reason_to_gemini, openai_message_reasoning_text, openai_response_to_claude,
    push_gemini_function_call_part,
};
use tools::{
    anthropic_tool_use_type_for_openai_tool_call, copy_non_replayable_tool_call_marker,
    normalized_openai_tool_call,
    normalized_openai_tool_definitions_from_request_with_request_scoped_custom_bridge,
    normalized_tool_definition_to_openai_with_request_scoped_custom_bridge,
    openai_responses_custom_tool_bridge_arguments,
    openai_responses_custom_tool_input_from_bridge_value,
    openai_tool_arguments_to_structured_value, openai_tool_call_partial_replay_text,
    request_scoped_openai_custom_bridge_expects_canonical_input_wrapper,
    semantic_text_part_from_claude_block, semantic_text_part_from_openai_part,
    semantic_text_part_to_openai_value, semantic_tool_kind_from_value,
    semantic_tool_result_content_from_value, semantic_tool_result_content_to_value,
    tool_call_is_marked_non_replayable, REQUEST_SCOPED_TOOL_BRIDGE_CONTEXT_FIELD,
};

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
    let bridge_context = body.get(REQUEST_SCOPED_TOOL_BRIDGE_CONTEXT_FIELD).cloned();
    let mut openai = if upstream_format == UpstreamFormat::Anthropic
        && client_format == UpstreamFormat::OpenAiResponses
    {
        claude_response_to_openai_with_reasoning_replay(body)?
    } else if upstream_format == UpstreamFormat::OpenAiResponses
        && client_format == UpstreamFormat::Anthropic
    {
        responses_response_to_openai_for_anthropic(body)?
    } else {
        upstream_response_to_openai(upstream_format, body)?
    };
    if let Some(bridge_context) = bridge_context {
        if let Some(openai_obj) = openai.as_object_mut() {
            openai_obj.insert(
                REQUEST_SCOPED_TOOL_BRIDGE_CONTEXT_FIELD.to_string(),
                bridge_context,
            );
        }
    }
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
    if matches!(
        client_format,
        UpstreamFormat::Anthropic | UpstreamFormat::Google
    ) && openai_response_has_assistant_audio(body)
    {
        return Err(openai_assistant_audio_not_portable_message(
            translation_target_label(client_format),
        ));
    }
    match client_format {
        UpstreamFormat::OpenAiCompletion => Ok(body.clone()),
        UpstreamFormat::OpenAiResponses => openai_response_to_responses(body),
        UpstreamFormat::Anthropic => openai_response_to_claude(body),
        UpstreamFormat::Google => openai_response_to_gemini(body),
    }
}

fn anthropic_block_has_cache_control(block: &Value) -> bool {
    block.get("cache_control").is_some()
}

fn anthropic_block_has_nonportable_thinking_provenance(block: &Value) -> bool {
    if block.get("type").and_then(Value::as_str) != Some("thinking") {
        return false;
    }
    block.get("signature").is_some()
        || block
            .get("thinking")
            .map(|thinking| !thinking.is_string())
            .unwrap_or(false)
}

fn anthropic_blocks_have_nonportable_thinking_provenance(blocks: &[Value]) -> bool {
    blocks
        .iter()
        .any(anthropic_block_has_nonportable_thinking_provenance)
}

fn anthropic_content_block_supported(block_type: &str) -> bool {
    matches!(
        block_type,
        "text" | "image" | "tool_use" | "server_tool_use" | "tool_result" | "thinking"
    )
}

fn anthropic_protocol_uses_cache_control(body: &Value) -> bool {
    if body.get("cache_control").is_some() {
        return true;
    }

    let system_uses_cache_control = body
        .get("system")
        .map(|system| match system {
            Value::Array(blocks) => blocks.iter().any(anthropic_block_has_cache_control),
            Value::Object(_) => anthropic_block_has_cache_control(system),
            _ => false,
        })
        .unwrap_or(false);
    if system_uses_cache_control {
        return true;
    }

    let messages_use_cache_control = body
        .get("messages")
        .and_then(Value::as_array)
        .map(|messages| {
            messages.iter().any(|message| {
                message
                    .get("content")
                    .and_then(Value::as_array)
                    .map(|blocks| blocks.iter().any(anthropic_block_has_cache_control))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    if messages_use_cache_control {
        return true;
    }

    body.get("tools")
        .and_then(Value::as_array)
        .map(|tools| tools.iter().any(anthropic_block_has_cache_control))
        .unwrap_or(false)
}

fn anthropic_block_not_portable_message(block_type: &str, target_label: &str) -> String {
    format!(
        "Anthropic content block `{block_type}` cannot be faithfully translated to {target_label}"
    )
}

fn anthropic_nonportable_block_message(block: &Value, target_label: &str) -> Option<String> {
    let block_type = block.get("type").and_then(Value::as_str)?;
    if anthropic_content_block_supported(block_type) {
        None
    } else {
        Some(anthropic_block_not_portable_message(
            block_type,
            target_label,
        ))
    }
}

fn anthropic_nonportable_content_block_message(body: &Value, target_label: &str) -> Option<String> {
    if let Some(system) = body.get("system") {
        match system {
            Value::Array(blocks) => {
                for block in blocks {
                    if let Some(message) = anthropic_nonportable_block_message(block, target_label)
                    {
                        return Some(message);
                    }
                }
            }
            Value::Object(_) => {
                if let Some(message) = anthropic_nonportable_block_message(system, target_label) {
                    return Some(message);
                }
            }
            _ => {}
        }
    }

    let messages = body.get("messages").and_then(Value::as_array)?;
    for message in messages {
        let Some(content) = message.get("content").and_then(Value::as_array) else {
            continue;
        };
        for block in content {
            if let Some(message) = anthropic_nonportable_block_message(block, target_label) {
                return Some(message);
            }
        }
    }
    None
}

fn anthropic_request_has_nonportable_thinking_provenance(body: &Value) -> bool {
    if let Some(system) = body.get("system") {
        match system {
            Value::Array(blocks)
                if anthropic_blocks_have_nonportable_thinking_provenance(blocks) =>
            {
                return true;
            }
            Value::Object(_) if anthropic_block_has_nonportable_thinking_provenance(system) => {
                return true;
            }
            _ => {}
        }
    }

    body.get("messages")
        .and_then(Value::as_array)
        .map(|messages| {
            messages.iter().any(|message| {
                message
                    .get("content")
                    .and_then(Value::as_array)
                    .map(|blocks| anthropic_blocks_have_nonportable_thinking_provenance(blocks))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn anthropic_request_nonportable_tool_definition_message(
    body: &Value,
    target_label: &str,
) -> Option<String> {
    let tools = body.get("tools").and_then(Value::as_array)?;
    for tool in tools {
        for field in [
            "strict",
            "defer_loading",
            "allowed_callers",
            "input_examples",
        ] {
            if tool.get(field).is_some() {
                return Some(anthropic_request_tool_definition_not_portable_message(
                    &format!("native `{field}` metadata"),
                    target_label,
                ));
            }
        }
        if tool.get("type").is_some() || tool.get("name").is_none() {
            return Some(anthropic_request_tool_definition_not_portable_message(
                "server-side or provider-native tool shapes",
                target_label,
            ));
        }
    }
    None
}

fn gemini_request_nonportable_function_response_message(
    body: &Value,
    target_label: &str,
) -> Option<String> {
    let contents = body.get("contents").and_then(Value::as_array)?;
    for content in contents {
        let Some(parts) = content.get("parts").and_then(Value::as_array) else {
            continue;
        };
        for part in parts {
            let Some(function_response) =
                gemini_part_field(part, "functionResponse", "function_response")
            else {
                continue;
            };
            if gemini_function_response_has_nonportable_parts(function_response) {
                return Some(gemini_function_response_parts_not_portable_message(
                    target_label,
                ));
            }
        }
    }
    None
}

fn gemini_request_nonportable_tool_message(body: &Value, target_label: &str) -> Option<String> {
    if let Some(tools) = gemini_request_tools(body) {
        for tool in tools {
            let Some(tool_obj) = tool.as_object() else {
                return Some(format!(
                    "Gemini tools must be objects; this tool entry cannot be faithfully translated to {target_label}"
                ));
            };
            for (key, value) in tool_obj {
                if value.is_null() {
                    continue;
                }
                if !matches!(
                    key.as_str(),
                    "functionDeclarations" | "function_declarations"
                ) {
                    return Some(format!(
                        "Gemini tool branch `{key}` cannot be faithfully translated to {target_label}; only pure functionDeclarations are portable cross-protocol"
                    ));
                }
            }
            if let Some(declarations) = gemini_tool_function_declarations(tool) {
                for declaration in declarations {
                    if let Some((field_name, _)) =
                        gemini_function_declaration_output_schema_field(declaration)
                    {
                        return Some(gemini_function_output_schema_not_portable_message(
                            declaration,
                            field_name,
                            target_label,
                        ));
                    }
                }
            }
        }
    }

    let openai_tools = match gemini_openai_function_tools_from_request(body) {
        Ok(tools) => tools,
        Err(message) => return Some(message),
    };

    let tool_config = gemini_request_tool_config(body)?;

    for (key, value) in tool_config {
        if value.is_null() {
            continue;
        }
        match key.as_str() {
            "functionCallingConfig" | "function_calling_config" => {}
            "includeServerSideToolInvocations" | "include_server_side_tool_invocations" => {
                return Some(format!(
                    "Gemini toolConfig `{key}` cannot be faithfully translated to {target_label}"
                ));
            }
            _ => {
                return Some(format!(
                    "Gemini toolConfig `{key}` cannot be faithfully translated to {target_label}"
                ));
            }
        }
    }

    let function_calling_config = gemini_request_function_calling_config_from_object(tool_config)?;

    for (key, value) in function_calling_config {
        if value.is_null() {
            continue;
        }
        if !matches!(
            key.as_str(),
            "mode" | "allowedFunctionNames" | "allowed_function_names"
        ) {
            return Some(format!(
                "Gemini functionCallingConfig field `{key}` cannot be faithfully translated to {target_label}"
            ));
        }
    }

    let mode = function_calling_config.get("mode").and_then(Value::as_str);
    match mode {
        Some("AUTO") | Some("NONE") | Some("ANY") | None => {}
        Some("VALIDATED") => {
            return Some(format!(
                "Gemini functionCallingConfig.mode = VALIDATED cannot be faithfully translated to {target_label}"
            ));
        }
        Some(other) => {
            return Some(format!(
                "Gemini functionCallingConfig.mode = {other} cannot be faithfully translated to {target_label}"
            ));
        }
    }

    let allowed_names =
        match gemini_validated_allowed_function_names(function_calling_config, &openai_tools) {
            Ok(allowed_names) => allowed_names,
            Err(message) => return Some(message),
        };
    if allowed_names.is_some() && mode != Some("ANY") {
        return Some(format!(
            "Gemini functionCallingConfig.allowedFunctionNames is only portable with mode ANY when translating to {target_label}"
        ));
    }

    None
}

fn gemini_request_nonportable_message(body: &Value, target_label: &str) -> Option<String> {
    gemini_request_nonportable_tool_message(body, target_label)
        .or_else(|| gemini_request_nonportable_output_shape_message(body, target_label))
        .or_else(|| gemini_request_nonportable_function_response_message(body, target_label))
}

fn anthropic_user_turn_requires_tool_result_reordering(blocks: &[Value]) -> bool {
    let mut saw_tool_result = false;
    let mut saw_non_tool_before_tool_result = false;
    let mut saw_non_tool_after_tool_result = false;

    for block in blocks {
        let is_tool_result = block.get("type").and_then(Value::as_str) == Some("tool_result");
        if is_tool_result {
            if saw_non_tool_before_tool_result || saw_non_tool_after_tool_result {
                return true;
            }
            saw_tool_result = true;
        } else if saw_tool_result {
            saw_non_tool_after_tool_result = true;
        } else {
            saw_non_tool_before_tool_result = true;
        }
    }

    false
}

fn anthropic_request_tool_result_order_message(body: &Value, target_label: &str) -> Option<String> {
    let messages = body.get("messages").and_then(Value::as_array)?;
    for message in messages {
        if message.get("role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let Some(blocks) = message.get("content").and_then(Value::as_array) else {
            continue;
        };
        let has_tool_results = blocks
            .iter()
            .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"));
        let has_non_tool_results = blocks
            .iter()
            .any(|block| block.get("type").and_then(Value::as_str) != Some("tool_result"));
        if has_tool_results
            && has_non_tool_results
            && anthropic_user_turn_requires_tool_result_reordering(blocks)
        {
            return Some(anthropic_tool_result_order_not_portable_message(
                target_label,
            ));
        }
    }
    None
}

fn claude_system_to_openai_content(system: &Value) -> Result<Option<Value>, String> {
    match system {
        Value::String(text) => Ok((!text.is_empty()).then(|| Value::String(text.clone()))),
        Value::Array(blocks) => {
            let mut parts = Vec::new();
            for block in blocks {
                let Some(block_type) = block.get("type").and_then(Value::as_str) else {
                    continue;
                };
                if block_type == "thinking" {
                    continue;
                }
                if block_type != "text" {
                    return Err(anthropic_block_not_portable_message(
                        block_type,
                        "OpenAI Chat Completions",
                    ));
                }
                let Some(part) = semantic_text_part_from_claude_block(block) else {
                    continue;
                };
                parts.push(semantic_text_part_to_openai_value(&part));
            }
            if parts.is_empty() {
                Ok(None)
            } else {
                Ok(Some(Value::Array(parts)))
            }
        }
        Value::Object(_) => {
            let Some(block_type) = system.get("type").and_then(Value::as_str) else {
                return Ok(None);
            };
            if block_type == "thinking" {
                return Ok(None);
            }
            if block_type != "text" {
                return Err(anthropic_block_not_portable_message(
                    block_type,
                    "OpenAI Chat Completions",
                ));
            }
            Ok(semantic_text_part_from_claude_block(system)
                .map(|part| semantic_text_part_to_openai_value(&part)))
        }
        _ => Ok(None),
    }
}

fn claude_response_to_openai(body: &Value) -> Result<Value, String> {
    claude_response_to_openai_internal(body, false)
}

fn claude_response_to_openai_with_reasoning_replay(body: &Value) -> Result<Value, String> {
    claude_response_to_openai_internal(body, true)
}

fn claude_response_to_openai_internal(
    body: &Value,
    allow_reasoning_replay: bool,
) -> Result<Value, String> {
    let content = body.get("content").cloned().ok_or("missing content")?;
    let bridge_context = body.get(REQUEST_SCOPED_TOOL_BRIDGE_CONTEXT_FIELD).cloned();
    let mut converted = convert_claude_message_to_openai_impl(
        &serde_json::json!({
            "role": "assistant",
            "content": content
        }),
        allow_reasoning_replay,
        bridge_context.as_ref(),
    )?
    .ok_or("missing content")?;
    let mut message = converted
        .drain(..)
        .find(|item| item.get("role").and_then(Value::as_str) == Some("assistant"))
        .ok_or("missing assistant message")?;
    if allow_reasoning_replay {
        let thinking_blocks = content
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|block| block.get("type").and_then(Value::as_str) == Some("thinking"))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if anthropic_blocks_have_nonportable_thinking_provenance(&thinking_blocks) {
            append_openai_message_anthropic_reasoning_replay_blocks(&mut message, thinking_blocks);
        }
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
        let refusal = extract_openai_content_text(message.get("content"));
        if !refusal.is_empty() {
            message["refusal"] = Value::String(refusal);
        }
        message["content"] = Value::Null;
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

        copy_remaining_usage_fields(
            usage,
            &mut usage_json,
            &[
                "input_tokens",
                "output_tokens",
                "cache_read_input_tokens",
                "cache_creation_input_tokens",
            ],
        );

        result["usage"] = usage_json;
    }
    if let Some(bridge_context) = bridge_context {
        result[REQUEST_SCOPED_TOOL_BRIDGE_CONTEXT_FIELD] = bridge_context;
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

fn openai_response_to_gemini(body: &Value) -> Result<Value, String> {
    let choice = single_required_array_item(
        body.get("choices")
            .and_then(Value::as_array)
            .map(Vec::as_slice),
        "OpenAI response",
        "Gemini",
        "choices",
    )?;
    let message = choice.get("message").ok_or("missing message")?;
    let response_logprobs = normalized_response_logprobs_from_openai_choice(choice, "Gemini")?;
    let mut parts: Vec<Value> = vec![];
    if let Some(rc) = openai_message_reasoning_text(message) {
        if !rc.is_empty() {
            parts.push(serde_json::json!({ "thought": true, "text": rc }));
        }
    }
    parts.extend(openai_content_to_gemini_parts(message.get("content"))?);
    if let Some(refusal) = extract_openai_refusal(message) {
        if !refusal.is_empty() {
            parts.push(serde_json::json!({ "text": refusal }));
        }
    }
    if let Some(tc) = message.get("tool_calls").and_then(Value::as_array) {
        for (idx, t) in tc.iter().enumerate() {
            push_gemini_function_call_part(&mut parts, t, idx == 0)?;
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
    if let Some(content_logprobs) = response_logprobs {
        let (avg_logprobs, logprobs_result) =
            normalized_response_logprobs_to_gemini_fields(&content_logprobs);
        result["candidates"][0]["logprobsResult"] = logprobs_result;
        if let Some(avg_logprobs) = avg_logprobs {
            result["candidates"][0]["avgLogprobs"] = avg_logprobs;
        }
    }
    if let Some(id) = body.get("id") {
        result["responseId"] = id.clone();
    }
    Ok(result)
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

fn claude_to_openai(body: &mut Value, preserve_reasoning_replay: bool) -> Result<(), String> {
    let bridge_context = body.get(REQUEST_SCOPED_TOOL_BRIDGE_CONTEXT_FIELD).cloned();
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
    if let Some(tool_choice) = body.get("tool_choice").filter(|value| !value.is_null()) {
        if let Some((mapped_tool_choice, disable_parallel)) = anthropic_tool_choice_to_openai(
            tool_choice,
            preserve_reasoning_replay,
            bridge_context.as_ref(),
        )? {
            result["tool_choice"] = mapped_tool_choice;
            if disable_parallel {
                result["parallel_tool_calls"] = Value::Bool(false);
            }
        }
    }
    // System: strip cache_control from blocks
    // Reference: 9router claudeHelper.js - remove all cache_control, add only to last block
    if let Some(system) = body.get("system") {
        if let Some(content) = claude_system_to_openai_content(system)? {
            result["messages"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({ "role": "system", "content": content }));
        }
    }
    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        for msg in messages {
            let thinking_blocks = msg
                .get("content")
                .and_then(Value::as_array)
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter(|block| {
                            block.get("type").and_then(Value::as_str) == Some("thinking")
                        })
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let preserve_message_replay = preserve_reasoning_replay
                && msg.get("role").and_then(Value::as_str) == Some("assistant")
                && anthropic_blocks_have_nonportable_thinking_provenance(&thinking_blocks);
            if let Some(mut openai_msg) = convert_claude_message_to_openai_impl(
                msg,
                preserve_reasoning_replay,
                bridge_context.as_ref(),
            )? {
                if preserve_message_replay {
                    for translated_msg in openai_msg.iter_mut() {
                        if translated_msg.get("role").and_then(Value::as_str) == Some("assistant") {
                            append_openai_message_anthropic_reasoning_replay_blocks(
                                translated_msg,
                                thinking_blocks.clone(),
                            );
                        }
                    }
                }
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
                let name = t.get("name").and_then(Value::as_str)?;
                if preserve_reasoning_replay
                    && request_scoped_openai_custom_bridge_expects_canonical_input_wrapper(
                        bridge_context.as_ref(),
                        name,
                    )
                {
                    return Some(serde_json::json!({
                        "type": "custom",
                        "custom": {
                            "name": name,
                            "description": t.get("description")
                        }
                    }));
                }
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
    if let Some(bridge_context) = bridge_context {
        result[REQUEST_SCOPED_TOOL_BRIDGE_CONTEXT_FIELD] = bridge_context;
    }
    *body = result;
    Ok(())
}

fn anthropic_tool_choice_to_openai(
    tool_choice: &Value,
    decode_custom_bridge: bool,
    bridge_context: Option<&Value>,
) -> Result<Option<(Value, bool)>, String> {
    let Some(tool_choice) = tool_choice.as_object() else {
        return Err(
            "Anthropic tool_choice must be an object for cross-protocol translation".to_string(),
        );
    };
    let Some(choice_type) = tool_choice.get("type").and_then(Value::as_str) else {
        return Err(
            "Anthropic tool_choice.type is required for cross-protocol translation".to_string(),
        );
    };
    let disable_parallel = tool_choice
        .get("disable_parallel_tool_use")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let mapped = match choice_type {
        "auto" => Value::String("auto".to_string()),
        "none" => Value::String("none".to_string()),
        "any" => Value::String("required".to_string()),
        "tool" => {
            let name = tool_choice
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .ok_or(
                    "Anthropic tool_choice.type = tool requires a non-empty `name` field."
                        .to_string(),
                )?;
            if decode_custom_bridge
                && request_scoped_openai_custom_bridge_expects_canonical_input_wrapper(
                    bridge_context,
                    name,
                )
            {
                serde_json::json!({
                    "type": "custom",
                    "custom": { "name": name }
                })
            } else {
                serde_json::json!({
                    "type": "function",
                    "function": { "name": name }
                })
            }
        }
        other => {
            return Err(format!(
            "Anthropic tool_choice.type `{other}` cannot be translated to OpenAI Chat Completions"
        ))
        }
    };

    Ok(Some((mapped, disable_parallel)))
}

fn convert_claude_message_to_openai_impl(
    msg: &Value,
    decode_custom_bridge: bool,
    bridge_context: Option<&Value>,
) -> Result<Option<Vec<Value>>, String> {
    let Some(role) = msg.get("role").and_then(Value::as_str) else {
        return Ok(None);
    };
    let openai_role = if role == "user" || role == "tool" {
        "user"
    } else {
        "assistant"
    };
    let Some(content) = msg.get("content") else {
        return Ok(None);
    };
    if content.is_string() {
        return Ok(Some(vec![
            serde_json::json!({ "role": openai_role, "content": content }),
        ]));
    }
    let Some(arr) = content.as_array() else {
        return Ok(None);
    };
    let mut parts: Vec<Value> = vec![];
    let mut tool_calls: Vec<Value> = vec![];
    let mut tool_results: Vec<Value> = vec![];
    let mut reasoning_text: String = String::new();
    for block in arr {
        let Some(ty) = block.get("type").and_then(Value::as_str) else {
            return Ok(None);
        };
        match ty {
            // Strip cache_control when converting from Claude to OpenAI
            // Reference: 9router claudeHelper.js - remove all cache_control
            "text" => {
                let Some(text_part) = semantic_text_part_from_claude_block(block) else {
                    return Ok(None);
                };
                parts.push(semantic_text_part_to_openai_value(&text_part));
            }
            "image" => {
                if block
                    .get("source")
                    .and_then(|s| s.get("type").and_then(Value::as_str))
                    == Some("base64")
                {
                    let src = block.get("source").unwrap();
                    let media = src
                        .get("media_type")
                        .and_then(Value::as_str)
                        .unwrap_or("image/png");
                    let data = src.get("data").and_then(Value::as_str).unwrap_or("");
                    parts.push(serde_json::json!({
                        "type": "image_url",
                        "image_url": { "url": format!("data:{};base64,{}", media, data) }
                    }));
                }
            }
            "tool_use" => {
                let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                let input = block
                    .get("input")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                if decode_custom_bridge
                    && request_scoped_openai_custom_bridge_expects_canonical_input_wrapper(
                        bridge_context,
                        name,
                    )
                {
                    if let Some(custom_input) =
                        openai_responses_custom_tool_input_from_bridge_value(&input)
                    {
                        tool_calls.push(serde_json::json!({
                            "id": block.get("id"),
                            "type": "custom",
                            "custom": {
                                "name": name,
                                "input": custom_input
                            }
                        }));
                        continue;
                    }
                }
                tool_calls.push(serde_json::json!({
                    "id": block.get("id"),
                    "type": "function",
                    "function": {
                        "name": block.get("name"),
                        "arguments": serde_json::to_string(&input)
                            .unwrap_or_else(|_| "{}".to_string())
                    }
                }));
            }
            "server_tool_use" => tool_calls.push(serde_json::json!({
                "id": block.get("id"),
                "type": "function",
                "proxied_tool_kind": "anthropic_server_tool_use",
                "function": {
                    "name": block.get("name"),
                    "arguments": block
                        .get("input")
                        .and_then(|input| serde_json::to_string(input).ok())
                        .unwrap_or_else(|| "{}".to_string())
                }
            })),
            "tool_result" => {
                let semantic_content =
                    semantic_tool_result_content_from_value(block.get("content"));
                tool_results.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": block.get("tool_use_id"),
                    "content": semantic_tool_result_content_to_value(&semantic_content)
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
            other if !anthropic_content_block_supported(other) => {
                return Err(anthropic_block_not_portable_message(
                    other,
                    "OpenAI Chat Completions",
                ));
            }
            _ => {}
        }
    }
    if !tool_results.is_empty() {
        let mut out: Vec<Value> = tool_results;
        if !parts.is_empty() {
            let content = collapse_claude_text_parts_for_openai(&parts);
            out.push(serde_json::json!({ "role": "user", "content": content }));
        }
        return Ok(Some(out));
    }
    if !tool_calls.is_empty() {
        let mut m = serde_json::json!({ "role": "assistant", "tool_calls": tool_calls });
        if !parts.is_empty() {
            m["content"] = collapse_claude_text_parts_for_openai(&parts);
        }
        if !reasoning_text.is_empty() {
            m["reasoning_content"] = Value::String(reasoning_text);
        }
        return Ok(Some(vec![m]));
    }
    if parts.is_empty() {
        let mut m = serde_json::json!({ "role": openai_role, "content": "" });
        if !reasoning_text.is_empty() {
            m["reasoning_content"] = Value::String(reasoning_text);
        }
        return Ok(Some(vec![m]));
    }
    let content = collapse_claude_text_parts_for_openai(&parts);
    let mut m = serde_json::json!({ "role": openai_role, "content": content });
    if !reasoning_text.is_empty() {
        m["reasoning_content"] = Value::String(reasoning_text);
    }
    Ok(Some(vec![m]))
}

fn collapse_claude_text_parts_for_openai(parts: &[Value]) -> Value {
    collapse_openai_text_parts(parts)
}

fn openai_to_claude(body: &mut Value) -> Result<(), String> {
    let controls = openai_normalized_request_controls(body)?;
    let request_scoped_tool_bridge_context =
        body.get(REQUEST_SCOPED_TOOL_BRIDGE_CONTEXT_FIELD).cloned();
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
    if let Some(tool_policy) = controls.tool_policy.as_ref() {
        result["tool_choice"] = normalized_tool_policy_to_claude_tool_choice(
            tool_policy,
            body.get("parallel_tool_calls"),
        );
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
    if let Some(output_config) = controls
        .output_shape
        .as_ref()
        .map(normalized_output_shape_to_claude_output_config)
        .transpose()?
        .flatten()
    {
        result["output_config"] = output_config;
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
            if let Some(tool_blocks) = openai_message_to_claude_blocks(&msg)? {
                pending_tool_results.extend(tool_blocks);
            }
            continue;
        }

        if let Some(mut claude_blocks) = openai_message_to_claude_blocks(&msg)? {
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

    let portable_tools = openai_portable_function_tools(
        body,
        controls.restricted_tool_names.as_deref(),
        "tool_choice.allowed_tools",
    )?;
    if !portable_tools.is_empty() {
        let claude_tools: Vec<Value> = portable_tools
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
    if let Some(bridge_context) = request_scoped_tool_bridge_context {
        result[REQUEST_SCOPED_TOOL_BRIDGE_CONTEXT_FIELD] = bridge_context;
    }
    *body = result;
    Ok(())
}

fn normalized_tool_policy_to_claude_tool_choice(
    tool_policy: &NormalizedToolPolicy,
    parallel_tool_calls: Option<&Value>,
) -> Value {
    let disable_parallel_tool_use = parallel_tool_calls.and_then(Value::as_bool) == Some(false);
    let mut mapped = match tool_policy {
        NormalizedToolPolicy::None => serde_json::json!({ "type": "none" }),
        NormalizedToolPolicy::Auto => serde_json::json!({ "type": "auto" }),
        NormalizedToolPolicy::Required => serde_json::json!({ "type": "any" }),
        NormalizedToolPolicy::ForcedFunction(name) => {
            serde_json::json!({ "type": "tool", "name": name })
        }
    };

    if disable_parallel_tool_use && mapped.get("type").and_then(Value::as_str) != Some("none") {
        mapped["disable_parallel_tool_use"] = Value::Bool(true);
    }
    mapped
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

fn openai_message_to_claude_blocks(msg: &Value) -> Result<Option<Vec<Value>>, String> {
    let Some(role) = msg.get("role").and_then(Value::as_str) else {
        return Ok(None);
    };
    if role == "tool" {
        if semantic_tool_kind_from_value(msg) == SemanticToolKind::OpenAiCustom {
            return Err(custom_tools_not_portable_message(UpstreamFormat::Anthropic));
        }
        let semantic_content = semantic_tool_result_content_from_value(msg.get("content"));
        return Ok(Some(vec![serde_json::json!({
            "type": "tool_result",
            "tool_use_id": msg.get("tool_call_id"),
            "content": semantic_tool_result_content_to_value(&semantic_content)
        })]));
    }
    let content = msg.get("content");
    let mut blocks: Vec<Value> = vec![];
    if role == "assistant" {
        if let Some(replay_blocks) = openai_message_anthropic_reasoning_replay_blocks(msg) {
            blocks.extend(replay_blocks);
        } else if let Some(reasoning) = openai_message_reasoning_text(msg) {
            if !reasoning.is_empty() {
                blocks.push(serde_json::json!({
                    "type": "thinking",
                    "thinking": reasoning
                }));
            }
        }
        if let Some(refusal) = extract_openai_refusal(msg) {
            if !refusal.is_empty() {
                blocks.push(serde_json::json!({ "type": "text", "text": refusal }));
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
                    let text_part = semantic_text_part_from_openai_part(c)
                        .ok_or("invalid OpenAI text content part")?;
                    let mut block = serde_json::json!({ "type": "text", "text": text_part.text });
                    if !text_part.annotations.is_empty() {
                        block["citations"] = Value::Array(text_part.annotations);
                    }
                    blocks.push(block);
                } else if ty == Some("refusal") {
                    blocks.push(serde_json::json!({
                        "type": "text",
                        "text": c.get("refusal").cloned().unwrap_or_else(|| Value::String(String::new()))
                    }));
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
                if tool_call_is_marked_non_replayable(t) {
                    blocks.push(serde_json::json!({
                        "type": "text",
                        "text": openai_tool_call_partial_replay_text(t)
                    }));
                    continue;
                }
                blocks.push(serde_json::json!({
                    "type": anthropic_tool_use_type_for_openai_tool_call(t)?,
                    "id": t.get("id"),
                    "name": t.get("function").and_then(|f| f.get("name")),
                    "input": openai_tool_arguments_to_structured_value(t, "Anthropic")?
                }));
            }
        }
    }
    if blocks.is_empty() && content.is_some() {
        return Ok(Some(vec![
            serde_json::json!({ "type": "text", "text": "" }),
        ]));
    }
    if blocks.is_empty() {
        return Ok(None);
    }
    Ok(Some(blocks))
}

#[cfg(test)]
mod tests;
