//! Request/response translation between formats (pivot: OpenAI Chat Completions).
//!
//! Reference: 9router open-sse/translator/index.js — source → openai → target.

use serde_json::Value;

use crate::config::ModelSurface;
use crate::formats::UpstreamFormat;

pub(crate) mod assessment;
pub(crate) mod messages;
pub(crate) mod models;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RequestTranslationPolicy {
    pub(crate) surface: ModelSurface,
}

impl RequestTranslationPolicy {
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        !self.has_native_request_policy_hooks()
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

    #[cfg(test)]
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
            &policy.surface,
        )
        .decision()
    {
        return Err(message);
    }
    validate_public_request_tool_names(client_format, body)?;

    if client_format == upstream_format {
        if stream && client_format != UpstreamFormat::OpenAiCompletion {
            normalize_openai_roles_for_compatibility(client_format, body);
        }
        if client_format == UpstreamFormat::OpenAiCompletion {
            apply_openai_completion_maximum_safe_role_repairs(body);
            apply_openai_completion_compat_overrides(model, body);
        }
        apply_request_translation_policy_defaults(upstream_format, &policy, body);
        return Ok(());
    }
    let translated_from_openai_completion = client_format == UpstreamFormat::OpenAiCompletion;
    // Step 1: client → openai (if client is not openai)
    if client_format != UpstreamFormat::OpenAiCompletion {
        client_to_openai_completion(client_format, upstream_format, body)?;
    }
    apply_request_translation_policy_defaults(UpstreamFormat::OpenAiCompletion, &policy, body);
    apply_request_scoped_bridge_structural_repair_pass(upstream_format, body)?;
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
        if stream && upstream_format != UpstreamFormat::OpenAiCompletion {
            normalize_openai_roles_for_compatibility(upstream_format, body);
        }
        apply_openai_completion_maximum_safe_role_repairs(body);
        apply_openai_completion_compat_overrides(model, body);
    }
    apply_request_translation_policy_defaults(upstream_format, &policy, body);
    Ok(())
}

fn apply_request_scoped_bridge_structural_repair_pass(
    target_format: UpstreamFormat,
    body: &mut Value,
) -> Result<(), String> {
    let Some(bridge_context) = request_scoped_tool_bridge_context_from_body(body) else {
        return Ok(());
    };

    repair_request_scoped_custom_bridge_tool_definitions(target_format, body, &bridge_context)?;
    repair_request_scoped_custom_bridge_tool_choice(body, &bridge_context);
    repair_request_scoped_custom_bridge_message_tool_calls(body, &bridge_context)?;
    Ok(())
}

fn repair_request_scoped_custom_bridge_tool_definitions(
    target_format: UpstreamFormat,
    body: &mut Value,
    bridge_context: &tools::ToolBridgeContext,
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

fn repair_request_scoped_custom_bridge_tool_choice(
    body: &mut Value,
    bridge_context: &tools::ToolBridgeContext,
) {
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
    bridge_context: &tools::ToolBridgeContext,
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
    }
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

fn apply_openai_completion_maximum_safe_role_repairs(body: &mut Value) {
    downgrade_openai_instruction_messages_to_user_for_maximum_safe_compatibility(body);
}

fn downgrade_openai_instruction_messages_to_user_for_maximum_safe_compatibility(body: &mut Value) {
    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };

    for message in messages.iter_mut() {
        let Some(role) = message
            .get("role")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            continue;
        };
        if !matches!(role.as_str(), "system" | "developer") {
            continue;
        }

        annotate_openai_instruction_message_for_maximum_safe_compatibility(message, &role);
        message["role"] = Value::String("user".to_string());
    }

    coalesce_openai_string_messages(body);
}

fn annotate_openai_instruction_message_for_maximum_safe_compatibility(
    message: &mut Value,
    role: &str,
) {
    let Some(content) = message.get("content") else {
        return;
    };
    let Some(text) = openai_instruction_content_as_text(content) else {
        return;
    };
    let label = match role {
        "developer" => "Developer instructions",
        _ => "System instructions",
    };
    message["content"] = Value::String(if text.is_empty() {
        label.to_string()
    } else {
        format!("{label}:\n{text}")
    });
}

fn openai_instruction_content_as_text(content: &Value) -> Option<String> {
    match content {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) if parts.iter().all(openai_instruction_part_is_text) => {
            Some(extract_openai_content_text(Some(content)))
        }
        Value::Object(_) if openai_instruction_part_is_text(content) => {
            Some(extract_openai_content_text(Some(&Value::Array(vec![
                content.clone(),
            ]))))
        }
        Value::Null => Some(String::new()),
        _ => None,
    }
}

fn openai_instruction_part_is_text(part: &Value) -> bool {
    part.get("type").and_then(Value::as_str) == Some("text")
        && part.get("text").and_then(Value::as_str).is_some()
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
    }
    Ok(())
}

fn openai_completion_to_upstream(
    to: UpstreamFormat,
    _target_model: &str,
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
    }
    Ok(())
}
mod media;
mod openai_family;
mod openai_responses;
#[cfg(test)]
mod regression_tests;
mod response_logprobs;
pub(crate) mod response_protocols;
pub(crate) mod tools;

use media::{
    classify_media_source_reference, http_or_https_remote_url, is_pdf_mime,
    openai_file_data_reference_from_part, openai_file_part_field,
    openai_file_part_resolved_mime_type, openai_file_reference_payload,
    validate_inline_base64_payload, MediaSourceReference, OpenAiFileDataReference,
};
use messages::{
    anthropic_request_tool_definition_not_portable_message,
    anthropic_tool_result_order_not_portable_message, custom_tools_not_portable_message,
    openai_assistant_audio_not_portable_message, translation_target_label,
};
use models::{
    NormalizedOpenAiFamilyToolCall, NormalizedOutputShape, NormalizedToolPolicy, SemanticToolKind,
    TranslationDecision,
};
use openai_family::{
    collapse_openai_text_parts, copy_remaining_usage_fields, extract_openai_content_text,
    extract_openai_refusal, extract_responses_text_content, openai_declared_function_tools,
    openai_normalized_request_controls, openai_response_has_assistant_audio,
    openai_select_function_tools_by_name,
};
use openai_responses::{
    append_openai_message_anthropic_reasoning_replay_blocks, messages_to_responses,
    openai_message_anthropic_reasoning_replay_blocks, openai_response_to_responses,
    responses_response_to_openai, responses_response_to_openai_for_anthropic,
    responses_to_messages,
};
use response_protocols::{
    is_minimax_model, normalize_openai_completion_response, openai_message_reasoning_text,
    openai_response_to_claude,
};
use tools::{
    anthropic_tool_use_type_for_openai_tool_call, copy_non_replayable_tool_call_marker,
    insert_request_scoped_tool_bridge_context, normalized_openai_tool_call,
    normalized_openai_tool_definitions_from_request_with_request_scoped_custom_bridge,
    normalized_responses_tool_call, normalized_responses_tool_definitions_from_request,
    normalized_tool_definition_to_openai_with_request_scoped_custom_bridge,
    openai_responses_custom_tool_bridge_arguments,
    openai_responses_custom_tool_input_from_bridge_value,
    openai_tool_arguments_to_structured_value, openai_tool_call_partial_replay_text,
    request_scoped_openai_custom_bridge_expects_canonical_input_wrapper,
    request_scoped_tool_bridge_context_from_body, semantic_text_part_from_claude_block,
    semantic_text_part_from_openai_part, semantic_text_part_to_openai_value,
    semantic_tool_kind_from_value, semantic_tool_result_content_from_value,
    semantic_tool_result_content_to_value, tool_call_is_marked_non_replayable,
    validate_openai_public_tool_choice_identity, validate_openai_public_tool_identity,
    validate_public_selector_visible_identities, validate_public_selector_visible_identity,
    validate_public_tool_name_not_reserved,
    validate_responses_public_response_object_tool_identity,
    validate_responses_public_tool_metadata_identity,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResponseTranslationContext {
    request_scoped_tool_bridge_context: Option<tools::ToolBridgeContext>,
}

impl ResponseTranslationContext {
    pub fn with_request_scoped_tool_bridge_context_value(mut self, value: Option<Value>) -> Self {
        self.request_scoped_tool_bridge_context = value
            .as_ref()
            .and_then(tools::ToolBridgeContext::from_value);
        self
    }

    fn request_scoped_tool_bridge_context(&self) -> Option<&tools::ToolBridgeContext> {
        self.request_scoped_tool_bridge_context.as_ref()
    }
}

/// Translate response body from upstream format to client format.
/// Converts via OpenAI pivot: upstream → openai → client when formats differ.
pub fn translate_response(
    upstream_format: UpstreamFormat,
    client_format: UpstreamFormat,
    body: &Value,
) -> Result<Value, String> {
    translate_response_with_context(
        upstream_format,
        client_format,
        body,
        ResponseTranslationContext::default(),
    )
}

pub fn translate_response_with_context(
    upstream_format: UpstreamFormat,
    client_format: UpstreamFormat,
    body: &Value,
    context: ResponseTranslationContext,
) -> Result<Value, String> {
    if crate::internal_artifacts::contains_internal_context_field(body) {
        return Err(format!(
            "upstream response contained internal-only field `{}`",
            crate::internal_artifacts::REQUEST_SCOPED_TOOL_BRIDGE_CONTEXT_FIELD
        ));
    }

    if upstream_format == client_format {
        validate_public_response_tool_names(client_format, body)?;
        return Ok(body.clone());
    }
    let bridge_context = context.request_scoped_tool_bridge_context();
    let openai = if upstream_format == UpstreamFormat::Anthropic
        && client_format == UpstreamFormat::OpenAiResponses
    {
        claude_response_to_openai_with_reasoning_replay(body, bridge_context)?
    } else if upstream_format == UpstreamFormat::OpenAiResponses
        && client_format == UpstreamFormat::Anthropic
    {
        responses_response_to_openai_for_anthropic(body)?
    } else {
        upstream_response_to_openai(upstream_format, body, bridge_context)?
    };
    if client_format == UpstreamFormat::OpenAiCompletion {
        return Ok(openai);
    }
    openai_response_to_client(client_format, &openai, bridge_context)
}

/// Convert upstream non-streaming response to OpenAI completion shape.
fn upstream_response_to_openai(
    upstream_format: UpstreamFormat,
    body: &Value,
    bridge_context: Option<&tools::ToolBridgeContext>,
) -> Result<Value, String> {
    match upstream_format {
        UpstreamFormat::OpenAiCompletion => Ok(normalize_openai_completion_response(body)),
        UpstreamFormat::Anthropic => claude_response_to_openai(body, bridge_context),
        UpstreamFormat::OpenAiResponses => responses_response_to_openai(body),
    }
}

/// Convert OpenAI completion response to client format (Responses or Claude).
fn openai_response_to_client(
    client_format: UpstreamFormat,
    body: &Value,
    bridge_context: Option<&tools::ToolBridgeContext>,
) -> Result<Value, String> {
    if client_format == UpstreamFormat::Anthropic && openai_response_has_assistant_audio(body) {
        return Err(openai_assistant_audio_not_portable_message(
            translation_target_label(client_format),
        ));
    }
    match client_format {
        UpstreamFormat::OpenAiCompletion => Ok(body.clone()),
        UpstreamFormat::OpenAiResponses => openai_response_to_responses(body, bridge_context),
        UpstreamFormat::Anthropic => openai_response_to_claude(body),
    }
}

fn validate_public_request_tool_names(format: UpstreamFormat, body: &Value) -> Result<(), String> {
    match format {
        UpstreamFormat::OpenAiCompletion => validate_openai_request_tool_names(body),
        UpstreamFormat::OpenAiResponses => validate_responses_request_tool_names(body),
        UpstreamFormat::Anthropic => validate_anthropic_body_tool_names(body),
    }
}

fn validate_public_response_tool_names(format: UpstreamFormat, body: &Value) -> Result<(), String> {
    match format {
        UpstreamFormat::OpenAiCompletion => validate_openai_response_tool_names(body),
        UpstreamFormat::OpenAiResponses => validate_responses_response_tool_names(body),
        UpstreamFormat::Anthropic => validate_anthropic_body_tool_names(body),
    }
}

fn validate_openai_request_tool_names(body: &Value) -> Result<(), String> {
    let bridge_context = request_scoped_tool_bridge_context_from_body(body);
    normalized_openai_tool_definitions_from_request_with_request_scoped_custom_bridge(
        body,
        bridge_context.as_ref(),
    )?;
    validate_openai_legacy_function_definitions(body)?;
    if let Some(tool_choice) = body.get("tool_choice").filter(|value| !value.is_null()) {
        validate_openai_family_tool_choice_names(tool_choice)?;
    }
    if let Some(function_call) = body.get("function_call").filter(|value| !value.is_null()) {
        validate_openai_legacy_function_call_name(function_call)?;
    }
    if let Some(names) = body.get("allowed_tool_names") {
        validate_public_selector_visible_identities(names)?;
    }
    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        for message in messages {
            validate_openai_message_tool_call_names(message)?;
        }
    }
    Ok(())
}

fn validate_openai_response_tool_names(body: &Value) -> Result<(), String> {
    let Some(choices) = body.get("choices").and_then(Value::as_array) else {
        return Ok(());
    };
    for choice in choices {
        if let Some(message) = choice.get("message") {
            validate_openai_message_tool_call_names(message)?;
        }
    }
    Ok(())
}

fn validate_openai_message_tool_call_names(message: &Value) -> Result<(), String> {
    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
        for tool_call in tool_calls {
            validate_public_selector_visible_identity(tool_call)?;
            normalized_openai_tool_call(tool_call)?;
        }
    }
    if let Some(function_call) = message
        .get("function_call")
        .filter(|value| !value.is_null())
    {
        validate_openai_legacy_function_call_name(function_call)?;
    }
    Ok(())
}

fn validate_openai_legacy_function_definitions(body: &Value) -> Result<(), String> {
    let Some(functions) = body.get("functions").and_then(Value::as_array) else {
        return Ok(());
    };
    for function in functions {
        validate_public_selector_visible_identity(function)?;
        validate_openai_public_tool_identity(function)?;
    }
    Ok(())
}

fn validate_openai_legacy_function_call_name(function_call: &Value) -> Result<(), String> {
    validate_public_selector_visible_identity(function_call)?;
    validate_openai_public_tool_identity(function_call)
}

fn validate_responses_request_tool_names(body: &Value) -> Result<(), String> {
    validate_responses_public_tool_metadata_identity(body)?;
    normalized_responses_tool_definitions_from_request(body)?;
    if let Some(tool_choice) = body.get("tool_choice").filter(|value| !value.is_null()) {
        validate_openai_family_tool_choice_names(tool_choice)?;
    }
    if let Some(items) = body.get("input").and_then(Value::as_array) {
        for item in items {
            normalized_responses_tool_call(item)?;
        }
    }
    Ok(())
}

fn validate_responses_response_tool_names(body: &Value) -> Result<(), String> {
    validate_responses_public_response_object_tool_identity(body)?;
    if let Some(response) = body.get("response") {
        validate_responses_public_response_object_tool_identity(response)?;
    }
    if let Some(output) = body.get("output").and_then(Value::as_array) {
        for item in output {
            normalized_responses_tool_call(item)?;
        }
    }
    Ok(())
}

fn validate_openai_family_tool_choice_names(choice: &Value) -> Result<(), String> {
    validate_openai_public_tool_choice_identity(choice)
}

fn validate_anthropic_body_tool_names(body: &Value) -> Result<(), String> {
    if let Some(tool_choice) = body.get("tool_choice").filter(|value| !value.is_null()) {
        validate_public_selector_visible_identity(tool_choice)?;
    }
    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
        for tool in tools {
            validate_public_selector_visible_identity(tool)?;
            if let Some(name) = tool.get("name").and_then(Value::as_str) {
                validate_public_tool_name_not_reserved(name)?;
            }
        }
    }
    if let Some(content) = body.get("content") {
        validate_anthropic_content_tool_names(content)?;
    }
    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        for message in messages {
            if let Some(content) = message.get("content") {
                validate_anthropic_content_tool_names(content)?;
            }
        }
    }
    Ok(())
}

fn validate_anthropic_content_tool_names(content: &Value) -> Result<(), String> {
    let Some(blocks) = content.as_array() else {
        return Ok(());
    };
    for block in blocks {
        if matches!(
            block.get("type").and_then(Value::as_str),
            Some("tool_use" | "server_tool_use")
        ) {
            if let Some(name) = block.get("name").and_then(Value::as_str) {
                validate_public_tool_name_not_reserved(name)?;
            }
        }
    }
    Ok(())
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
            .unwrap_or(true)
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

fn claude_response_to_openai(
    body: &Value,
    bridge_context: Option<&tools::ToolBridgeContext>,
) -> Result<Value, String> {
    claude_response_to_openai_internal(body, false, bridge_context)
}

fn claude_response_to_openai_with_reasoning_replay(
    body: &Value,
    bridge_context: Option<&tools::ToolBridgeContext>,
) -> Result<Value, String> {
    claude_response_to_openai_internal(body, true, bridge_context)
}

fn claude_response_to_openai_internal(
    body: &Value,
    allow_reasoning_replay: bool,
    bridge_context: Option<&tools::ToolBridgeContext>,
) -> Result<Value, String> {
    let content = body.get("content").cloned().ok_or("missing content")?;
    let mut converted = convert_claude_message_to_openai_impl(
        &serde_json::json!({
            "role": "assistant",
            "content": content
        }),
        allow_reasoning_replay,
        bridge_context,
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

fn claude_to_openai(body: &mut Value, preserve_reasoning_replay: bool) -> Result<(), String> {
    let bridge_context = request_scoped_tool_bridge_context_from_body(body);
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
        let mut converted_tools: Vec<Value> = Vec::new();
        for t in tools {
            let Some(name) = t.get("name").and_then(Value::as_str) else {
                continue;
            };
            validate_public_tool_name_not_reserved(name)?;
            if preserve_reasoning_replay
                && request_scoped_openai_custom_bridge_expects_canonical_input_wrapper(
                    bridge_context.as_ref(),
                    name,
                )
            {
                converted_tools.push(serde_json::json!({
                        "type": "custom",
                        "custom": {
                            "name": name,
                            "description": t.get("description")
                        }
                }));
            } else {
                converted_tools.push(serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": name,
                        "description": t.get("description"),
                        "parameters": t.get("input_schema").or(t.get("parameters")).unwrap_or(&serde_json::json!({ "type": "object", "properties": {} }))
                    }
                }));
            }
        }
        if !converted_tools.is_empty() {
            result["tools"] = Value::Array(converted_tools);
        }
    }
    if let Some(bridge_context) = bridge_context.as_ref() {
        insert_request_scoped_tool_bridge_context(&mut result, bridge_context);
    }
    *body = result;
    Ok(())
}

fn anthropic_tool_choice_to_openai(
    tool_choice: &Value,
    decode_custom_bridge: bool,
    bridge_context: Option<&tools::ToolBridgeContext>,
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
            validate_public_tool_name_not_reserved(name)?;
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
    bridge_context: Option<&tools::ToolBridgeContext>,
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
                parts.push(anthropic_image_block_to_openai_part(block)?);
            }
            "tool_use" => {
                let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                validate_public_tool_name_not_reserved(name)?;
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
            "server_tool_use" => {
                if let Some(name) = block.get("name").and_then(Value::as_str) {
                    validate_public_tool_name_not_reserved(name)?;
                }
                tool_calls.push(serde_json::json!({
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
                }));
            }
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

fn anthropic_image_block_to_openai_part(block: &Value) -> Result<Value, String> {
    let source = block
        .get("source")
        .ok_or_else(|| anthropic_block_not_portable_message("image", "OpenAI Chat Completions"))?;
    match source.get("type").and_then(Value::as_str) {
        Some("base64") => {
            let media = source
                .get("media_type")
                .and_then(Value::as_str)
                .unwrap_or("image/png");
            let data = source.get("data").and_then(Value::as_str).unwrap_or("");
            let Some(data) = validate_inline_base64_payload(data) else {
                return Err(
                    "Anthropic image source.type=base64 requires canonical non-empty base64 `data`."
                        .to_string(),
                );
            };
            Ok(serde_json::json!({
                "type": "image_url",
                "image_url": { "url": format!("data:{};base64,{}", media, data) }
            }))
        }
        Some("url") => {
            let url = source.get("url").and_then(Value::as_str).ok_or_else(|| {
                "Anthropic image source.type=url requires a string `url` to translate to OpenAI Chat Completions."
                    .to_string()
            })?;
            let Some(url) = http_or_https_remote_url(url) else {
                return Err(
                    "Anthropic image source.type=url only supports clean http:// or https:// remote URLs for OpenAI targets."
                        .to_string(),
                );
            };
            Ok(serde_json::json!({
                "type": "image_url",
                "image_url": { "url": url }
            }))
        }
        Some(other) => Err(format!(
            "Anthropic image source type `{other}` cannot be faithfully translated to OpenAI Chat Completions."
        )),
        None => Err(
            "Anthropic image blocks require a source.type to translate to OpenAI Chat Completions."
                .to_string(),
        ),
    }
}

fn collapse_claude_text_parts_for_openai(parts: &[Value]) -> Value {
    collapse_openai_text_parts(parts)
}

fn openai_to_claude(body: &mut Value) -> Result<(), String> {
    let controls = openai_normalized_request_controls(body)?;
    let request_scoped_tool_bridge_context = request_scoped_tool_bridge_context_from_body(body);
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
        system_blocks.extend(openai_system_content_to_claude_blocks(
            role.unwrap_or("system"),
            msg.get("content"),
        )?);
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
    if let Some(bridge_context) = request_scoped_tool_bridge_context.as_ref() {
        insert_request_scoped_tool_bridge_context(&mut result, bridge_context);
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

fn openai_content_part_not_portable_to_anthropic_message(part_type: &str, reason: &str) -> String {
    format!(
        "OpenAI content part `{part_type}` cannot be faithfully translated to Anthropic: {reason}"
    )
}

fn openai_image_url_part_url(part: &Value) -> Option<&str> {
    match part.get("image_url") {
        Some(Value::String(url)) => Some(url.as_str()),
        Some(Value::Object(image_url)) => image_url.get("url").and_then(Value::as_str),
        _ => None,
    }
}

fn openai_image_url_part_to_claude_block(part: &Value) -> Result<Value, String> {
    let Some(url) = openai_image_url_part_url(part) else {
        return Err(openai_content_part_not_portable_to_anthropic_message(
            "image_url",
            "missing image URL; Anthropic image blocks require inline base64 image data.",
        ));
    };
    match classify_media_source_reference(url) {
        MediaSourceReference::MimeDataUri { mime_type, data } => {
            if !mime_type.starts_with("image/") {
                return Err(openai_content_part_not_portable_to_anthropic_message(
                    "image_url",
                    &format!("data URI MIME `{mime_type}` is not an image MIME type."),
                ));
            }
            Ok(serde_json::json!({
                "type": "image",
                "source": { "type": "base64", "media_type": mime_type, "data": data }
            }))
        }
        MediaSourceReference::HttpRemoteUrl { url } => Ok(serde_json::json!({
            "type": "image",
            "source": { "type": "url", "url": url }
        })),
        MediaSourceReference::ProviderOrLocalUri { uri } => Err(
            openai_content_part_not_portable_to_anthropic_message(
                "image_url",
                &format!(
                    "Anthropic image URL sources only support http:// or https:// remote URLs; provider/local URI `{uri}` is not portable."
                ),
            ),
        ),
        MediaSourceReference::BareBase64 { .. } | MediaSourceReference::Unsupported { .. } => {
            Err(openai_content_part_not_portable_to_anthropic_message(
                "image_url",
                "image_url must be a base64 data URI or an http:// or https:// remote URL.",
            ))
        }
    }
}

fn openai_file_part_error_to_anthropic(part_type: &str, reason: &str) -> String {
    openai_content_part_not_portable_to_anthropic_message(part_type, reason)
}

fn openai_file_part_to_claude_document_block(part: &Value) -> Result<Value, String> {
    openai_file_reference_payload(part)
        .map_err(|err| openai_file_part_error_to_anthropic("file/input_file", &err))?;
    if openai_file_part_field(part, "file_data")
        .or_else(|| openai_file_part_field(part, "file_url"))
        .is_none()
    {
        return Err(openai_content_part_not_portable_to_anthropic_message(
            "file/input_file",
            "file parts require file_data or file_url with PDF MIME/filename provenance; provider file_id references are not portable.",
        ));
    }

    let resolved_mime = openai_file_part_resolved_mime_type(part)
        .map_err(|err| openai_file_part_error_to_anthropic("file/input_file", &err))?;
    let reference = openai_file_data_reference_from_part(part)
        .map_err(|err| openai_file_part_error_to_anthropic("file/input_file", &err))?;
    match reference {
        OpenAiFileDataReference::InlineData { mime_type, data } => {
            if !is_pdf_mime(&mime_type) {
                return Err(openai_file_part_error_to_anthropic(
                    "file/input_file",
                    &format!("only application/pdf files can map to Anthropic document blocks; got MIME `{mime_type}`."),
                ));
            }
            Ok(serde_json::json!({
                "type": "document",
                "source": { "type": "base64", "media_type": "application/pdf", "data": data }
            }))
        }
        OpenAiFileDataReference::HttpRemoteUrl { mime_type, url } => {
            if !is_pdf_mime(&mime_type) {
                return Err(openai_file_part_error_to_anthropic(
                    "file/input_file",
                    &format!("only application/pdf HTTP(S) URLs can map to Anthropic document URL blocks; got MIME `{mime_type}`."),
                ));
            }
            Ok(serde_json::json!({
                "type": "document",
                "source": { "type": "url", "url": url }
            }))
        }
        OpenAiFileDataReference::ProviderOrLocalUri { mime_type, uri } => {
            let _ = mime_type;
            Err(openai_file_part_error_to_anthropic(
                "file/input_file",
                &format!(
                    "Anthropic document URL sources only support http:// or https:// PDF URLs; provider/local URI `{uri}` is not portable."
                ),
            ))
        }
        OpenAiFileDataReference::BareBase64 { mime_type, data } => {
            let _ = data;
            let mime = resolved_mime.or(mime_type);
            let detail = mime
                .as_deref()
                .map(|mime| format!(" with MIME `{mime}`"))
                .unwrap_or_default();
            Err(openai_file_part_error_to_anthropic(
                "file/input_file",
                &format!("bare base64 file_data{detail} must not be coerced into Anthropic document bytes; use a MIME-bearing PDF data URI."),
            ))
        }
    }
}

fn openai_audio_part_not_portable_to_anthropic_message(part: &Value) -> String {
    let format = part
        .get("input_audio")
        .and_then(|audio| audio.get("format"))
        .and_then(Value::as_str)
        .filter(|format| !format.is_empty())
        .map(|format| format!(" with format `{format}`"))
        .unwrap_or_default();
    openai_content_part_not_portable_to_anthropic_message(
        "input_audio",
        &format!("audio input{format} has no native Anthropic request mapping in this translator."),
    )
}

fn openai_text_part_to_claude_text_block(part: &Value) -> Result<Value, String> {
    let text_part =
        semantic_text_part_from_openai_part(part).ok_or("invalid OpenAI text content part")?;
    let mut block = serde_json::json!({ "type": "text", "text": text_part.text });
    if !text_part.annotations.is_empty() {
        block["citations"] = Value::Array(text_part.annotations);
    }
    Ok(block)
}

fn openai_refusal_part_to_claude_text_block(part: &Value) -> Value {
    serde_json::json!({
        "type": "text",
        "text": part.get("refusal").cloned().unwrap_or_else(|| Value::String(String::new()))
    })
}

fn openai_system_content_part_not_portable_to_anthropic_message(
    role: &str,
    part_type: &str,
    reason: &str,
) -> String {
    format!(
        "OpenAI {role} content part `{part_type}` cannot be faithfully translated to Anthropic system blocks: {reason}"
    )
}

fn openai_system_content_part_label(part_type: &str) -> &str {
    if part_type == "file" {
        "file/input_file"
    } else {
        part_type
    }
}

fn openai_system_part_to_claude_block(role: &str, part: &Value) -> Result<Option<Value>, String> {
    let Some(part_type) = part.get("type").and_then(Value::as_str) else {
        return Err(format!(
            "OpenAI {role} content array entries require a string `type` to translate to Anthropic system blocks."
        ));
    };
    match part_type {
        "text" => openai_text_part_to_claude_text_block(part).map(Some),
        "refusal" => Ok(Some(openai_refusal_part_to_claude_text_block(part))),
        other => Err(openai_system_content_part_not_portable_to_anthropic_message(
            role,
            openai_system_content_part_label(other),
            "system/developer content arrays only support text/refusal parts; unsupported typed parts must not be silently dropped.",
        )),
    }
}

fn normalized_output_shape_to_claude_output_config(
    shape: &NormalizedOutputShape,
) -> Result<Option<Value>, String> {
    match shape {
        NormalizedOutputShape::Text => Ok(None),
        NormalizedOutputShape::JsonSchema(schema) => Ok(Some(serde_json::json!({
            "format": {
                "type": "json_schema",
                "schema": schema.schema.clone()
            }
        }))),
        NormalizedOutputShape::JsonObject => Err(
            "OpenAI/Responses `json_object` structured output cannot be faithfully translated to Anthropic structured outputs"
                .to_string(),
        ),
    }
}

fn openai_portable_function_tools(
    body: &Value,
    restricted_tool_names: Option<&[String]>,
    selector_label: &str,
) -> Result<Vec<Value>, String> {
    let declared_tools = openai_declared_function_tools(body);
    if let Some(selected_names) = restricted_tool_names {
        openai_select_function_tools_by_name(&declared_tools, selected_names, selector_label)
    } else {
        Ok(declared_tools)
    }
}

fn openai_system_content_to_claude_blocks(
    role: &str,
    content: Option<&Value>,
) -> Result<Vec<Value>, String> {
    let Some(content) = content else {
        return Ok(Vec::new());
    };
    match content {
        Value::String(text) => {
            if text.is_empty() {
                Ok(Vec::new())
            } else {
                Ok(vec![serde_json::json!({ "type": "text", "text": text })])
            }
        }
        Value::Array(parts) => {
            let mut blocks = Vec::new();
            for part in parts {
                if let Some(block) = openai_system_part_to_claude_block(role, part)? {
                    blocks.push(block);
                }
            }
            Ok(blocks)
        }
        Value::Object(_) => Ok(openai_system_part_to_claude_block(role, content)?
            .into_iter()
            .collect()),
        _ => Ok(Vec::new()),
    }
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
                let Some(ty) = c.get("type").and_then(Value::as_str) else {
                    return Err(
                        "OpenAI content array entries require a string `type` to translate to Anthropic."
                            .to_string(),
                    );
                };
                match ty {
                    "text" => {
                        blocks.push(openai_text_part_to_claude_text_block(c)?);
                    }
                    "refusal" => {
                        blocks.push(openai_refusal_part_to_claude_text_block(c));
                    }
                    "image_url" => {
                        blocks.push(openai_image_url_part_to_claude_block(c)?);
                    }
                    "input_audio" => {
                        return Err(openai_audio_part_not_portable_to_anthropic_message(c));
                    }
                    "file" | "input_file" => {
                        blocks.push(openai_file_part_to_claude_document_block(c)?);
                    }
                    other => {
                        return Err(openai_content_part_not_portable_to_anthropic_message(
                            other,
                            "unsupported typed content parts must not be silently dropped.",
                        ));
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
