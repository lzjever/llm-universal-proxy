use std::collections::BTreeSet;

use serde_json::Value;

use crate::config::{ModelModality, ModelSurface};
use crate::formats::UpstreamFormat;

use super::media::{openai_file_part_mime_conflict_message, openai_file_part_resolved_mime_type};
use super::messages::{
    custom_tool_format_downgraded_message, custom_tool_format_reject_message,
    custom_tools_not_portable_message, openai_assistant_audio_history_not_portable_message,
    openai_request_audio_not_portable_message, responses_reasoning_continuity_not_portable_message,
    single_candidate_choice_contract_message, translation_target_label,
};
use super::models::{
    NormalizedLogprobsControls, NormalizedOpenAiAudioContract, NormalizedOpenAiFamilyToolDef,
    SemanticToolKind, SharedControlProfile, TranslationAssessment,
};
use super::openai_responses::{
    responses_compaction_summary_text, responses_input_item_is_compaction,
    responses_input_item_is_message, responses_input_item_type, responses_reasoning_summary_text,
};
use super::tools::{
    normalized_responses_tool_definition, openai_custom_tool_format_is_plain_text,
    openai_custom_tool_format_supports_anthropic_bridge, openai_tool_arguments_to_structured_value,
    responses_tool_call_item_to_openai_tool_call, responses_tool_call_to_structured_value,
    semantic_tool_kind_from_value, tool_call_is_marked_non_replayable,
};
use super::{
    anthropic_nonportable_content_block_message, anthropic_protocol_uses_cache_control,
    anthropic_request_has_nonportable_thinking_provenance,
    anthropic_request_nonportable_tool_definition_message,
    anthropic_request_tool_result_order_message,
};

pub(super) fn responses_stateful_request_controls_for_translate(body: &Value) -> Vec<&'static str> {
    let mut controls = Vec::new();
    for field in [
        "previous_response_id",
        "conversation",
        "background",
        "prompt",
        "context_management",
    ] {
        if body.get(field).is_some() {
            controls.push(field);
        }
    }
    if body.get("store").and_then(Value::as_bool) == Some(true) {
        controls.push("store");
    }
    controls
}

pub(super) fn cross_protocol_store_warning_message(
    client_format: UpstreamFormat,
    upstream_format: UpstreamFormat,
) -> String {
    format!(
        "{} request field `store` has provider-specific persistence semantics and will be dropped when translating to {}",
        translation_target_label(client_format),
        translation_target_label(upstream_format)
    )
}

pub(super) fn shared_control_profile_for_target(
    target_format: UpstreamFormat,
) -> SharedControlProfile {
    match target_format {
        UpstreamFormat::OpenAiCompletion => SharedControlProfile {
            metadata: true,
            user: true,
            service_tier: true,
            stream_include_obfuscation: true,
            verbosity: true,
            reasoning_effort: true,
            prompt_cache_key: true,
            prompt_cache_retention: true,
            safety_identifier: true,
            top_logprobs: true,
            parallel_tool_calls: true,
            logit_bias: true,
        },
        UpstreamFormat::OpenAiResponses => SharedControlProfile {
            metadata: true,
            user: true,
            service_tier: true,
            stream_include_obfuscation: true,
            verbosity: true,
            reasoning_effort: true,
            prompt_cache_key: true,
            prompt_cache_retention: true,
            safety_identifier: true,
            top_logprobs: true,
            parallel_tool_calls: true,
            logit_bias: false,
        },
        UpstreamFormat::Anthropic => SharedControlProfile {
            metadata: true,
            parallel_tool_calls: true,
            ..SharedControlProfile::default()
        },
    }
}

pub(super) fn request_stream_include_obfuscation(body: &Value) -> Option<Value> {
    body.get("stream_options")
        .and_then(Value::as_object)
        .and_then(|stream_options| stream_options.get("include_obfuscation"))
        .cloned()
}

pub(super) fn openai_normalized_logprobs_controls(
    body: &Value,
) -> Option<NormalizedLogprobsControls> {
    let enabled = body.get("logprobs").and_then(Value::as_bool) == Some(true);
    let top_logprobs = body.get("top_logprobs").cloned();
    (enabled || top_logprobs.is_some()).then_some(NormalizedLogprobsControls {
        enabled,
        top_logprobs,
    })
}

pub(super) fn responses_normalized_logprobs_controls(
    body: &Value,
) -> Option<NormalizedLogprobsControls> {
    let enabled = responses_include_requests_output_text_logprobs(body);
    let top_logprobs = body.get("top_logprobs").cloned();
    (enabled || top_logprobs.is_some()).then_some(NormalizedLogprobsControls {
        enabled,
        top_logprobs,
    })
}

pub(super) fn normalized_openai_audio_contract(
    body: &Value,
) -> Result<Option<NormalizedOpenAiAudioContract>, String> {
    let modalities = body
        .get("modalities")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|item| item.trim().to_ascii_lowercase())
                .filter(|item| !item.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let requests_audio =
        modalities.iter().any(|item| item == "audio") || body.get("audio").is_some();
    if !requests_audio {
        return Ok(None);
    }

    let audio = body.get("audio").and_then(Value::as_object).ok_or(
        "OpenAI Chat audio output requests require a top-level `audio` object.".to_string(),
    )?;
    if body.get("audio").is_some()
        && !modalities.is_empty()
        && !modalities.iter().any(|item| item == "audio")
    {
        return Err(
            "OpenAI Chat audio output requests require `modalities` to include `audio`."
                .to_string(),
        );
    }
    if let Some(format) = audio.get("format").and_then(Value::as_str) {
        return Err(format!(
            "OpenAI Chat audio field `audio.format` value `{format}` is outside the portable cross-protocol audio subset."
        ));
    }
    let voice_name = match audio.get("voice") {
        Some(Value::String(voice)) if !voice.trim().is_empty() => Some(voice.clone()),
        Some(Value::Object(voice)) => {
            let id = voice.get("id").and_then(Value::as_str).unwrap_or("");
            return Err(format!(
                "OpenAI Chat audio voice `{id}` is outside the portable cross-protocol audio subset."
            ));
        }
        Some(_) => {
            return Err(
                "OpenAI Chat audio voice must be a non-empty string for portable cross-protocol audio translation."
                    .to_string(),
            )
        }
        None => None,
    };

    let normalized_modalities = if modalities.is_empty() {
        vec!["audio".to_string()]
    } else {
        modalities
            .iter()
            .filter(|item| item.as_str() == "text" || item.as_str() == "audio")
            .cloned()
            .collect::<Vec<_>>()
    };
    if normalized_modalities.is_empty() {
        return Err(
            "OpenAI Chat audio output requests require `modalities` to include `audio`."
                .to_string(),
        );
    }

    Ok(Some(NormalizedOpenAiAudioContract {
        response_modalities: normalized_modalities,
        voice_name,
    }))
}

pub(super) fn openai_assistant_history_audio_present(body: &Value) -> bool {
    body.get("messages")
        .and_then(Value::as_array)
        .map(|messages| {
            messages.iter().any(|message| {
                message.get("role").and_then(Value::as_str) == Some("assistant")
                    && message
                        .get("audio")
                        .filter(|audio| !audio.is_null())
                        .is_some()
            })
        })
        .unwrap_or(false)
}

pub(super) fn responses_include_items(body: &Value) -> Vec<&str> {
    body.get("include")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

pub(super) fn responses_include_requests_output_text_logprobs(body: &Value) -> bool {
    responses_include_items(body).contains(&"message.output_text.logprobs")
}

pub(super) fn responses_include_has_nonportable_items(
    body: &Value,
    target_format: UpstreamFormat,
) -> bool {
    let include_items = responses_include_items(body);
    if include_items.is_empty() {
        return body.get("include").is_some();
    }

    include_items.iter().any(|item| {
        if *item == "reasoning.encrypted_content" {
            return false;
        }
        !matches!(
            (target_format, *item),
            (
                UpstreamFormat::OpenAiCompletion | UpstreamFormat::OpenAiResponses,
                "message.output_text.logprobs"
            )
        )
    })
}

pub(super) fn responses_text_verbosity(body: &Value) -> Option<Value> {
    body.get("text")
        .and_then(Value::as_object)
        .and_then(|text| text.get("verbosity"))
        .cloned()
}

pub(super) fn responses_reasoning_effort(body: &Value) -> Option<Value> {
    body.get("reasoning")
        .and_then(Value::as_object)
        .and_then(|reasoning| reasoning.get("effort"))
        .cloned()
}

pub(super) fn object_has_only_keys(
    object: &serde_json::Map<String, Value>,
    allowed_keys: &[&str],
) -> bool {
    object
        .keys()
        .all(|key| allowed_keys.contains(&key.as_str()))
}

pub(super) fn responses_text_has_nonportable_fields(
    body: &Value,
    profile: SharedControlProfile,
) -> bool {
    let Some(text) = body.get("text").and_then(Value::as_object) else {
        return false;
    };
    let mut allowed_keys = vec!["format"];
    if profile.verbosity {
        allowed_keys.push("verbosity");
    }
    !object_has_only_keys(text, &allowed_keys)
}

pub(super) fn responses_reasoning_has_nonportable_fields(
    body: &Value,
    profile: SharedControlProfile,
) -> bool {
    let Some(reasoning) = body.get("reasoning").and_then(Value::as_object) else {
        return false;
    };
    let mut allowed_keys = Vec::new();
    if profile.reasoning_effort {
        allowed_keys.push("effort");
    }
    !object_has_only_keys(reasoning, &allowed_keys)
}

pub(super) fn openai_to_responses_dropped_control_names(body: &Value) -> Vec<&'static str> {
    let mut controls = Vec::new();
    for field in [
        "stop",
        "seed",
        "presence_penalty",
        "frequency_penalty",
        "logit_bias",
        "prediction",
        "web_search_options",
    ] {
        if body.get(field).is_some() {
            controls.push(field);
        }
    }
    controls
}

pub(super) fn openai_to_anthropic_dropped_control_names(body: &Value) -> Vec<&'static str> {
    let mut controls = Vec::new();
    for field in ["seed", "presence_penalty", "frequency_penalty"] {
        if body.get(field).is_some() {
            controls.push(field);
        }
    }
    controls
}

pub(super) fn openai_warning_only_request_controls_for_translate(
    body: &Value,
    target_format: UpstreamFormat,
) -> Vec<String> {
    let profile = shared_control_profile_for_target(target_format);
    let mut controls = Vec::new();
    if body.get("metadata").is_some() && !profile.metadata {
        controls.push("metadata".to_string());
    }
    if body.get("user").is_some() && !profile.user {
        controls.push("user".to_string());
    }
    if body.get("service_tier").is_some() && !profile.service_tier {
        controls.push("service_tier".to_string());
    }
    if request_stream_include_obfuscation(body).is_some() && !profile.stream_include_obfuscation {
        controls.push("stream_options.include_obfuscation".to_string());
    }
    if body.get("verbosity").is_some() && !profile.verbosity {
        controls.push("verbosity".to_string());
    }
    if body.get("reasoning_effort").is_some() && !profile.reasoning_effort {
        controls.push("reasoning_effort".to_string());
    }
    if body.get("prompt_cache_key").is_some() && !profile.prompt_cache_key {
        controls.push("prompt_cache_key".to_string());
    }
    if body.get("prompt_cache_retention").is_some() && !profile.prompt_cache_retention {
        controls.push("prompt_cache_retention".to_string());
    }
    if body.get("safety_identifier").is_some() && !profile.safety_identifier {
        controls.push("safety_identifier".to_string());
    }
    if body.get("logprobs").and_then(Value::as_bool) == Some(true) && !profile.top_logprobs {
        controls.push("logprobs".to_string());
    }
    if body.get("top_logprobs").is_some() && !profile.top_logprobs {
        controls.push("top_logprobs".to_string());
    }
    if body.get("logit_bias").is_some() && !profile.logit_bias {
        controls.push("logit_bias".to_string());
    }
    if body.get("prediction").is_some() {
        controls.push("prediction".to_string());
    }
    if body.get("web_search_options").is_some() {
        controls.push("web_search_options".to_string());
    }
    controls
}

pub(super) fn responses_warning_only_request_controls_for_translate(
    body: &Value,
    target_format: UpstreamFormat,
) -> Vec<String> {
    let profile = shared_control_profile_for_target(target_format);
    let mut controls = Vec::new();
    for field in [
        "stop",
        "seed",
        "presence_penalty",
        "frequency_penalty",
        "max_tool_calls",
        "truncation",
    ] {
        if body.get(field).is_some() {
            controls.push(field.to_string());
        }
    }
    if responses_include_has_nonportable_items(body, target_format) {
        controls.push("include".to_string());
    }
    if responses_include_items(body).contains(&"reasoning.encrypted_content") {
        controls.push("reasoning.encrypted_content".to_string());
    }
    if responses_input_reasoning_encrypted_content_present(body) {
        controls.push("input[].reasoning.encrypted_content".to_string());
    }
    if responses_input_compaction_carrier_present(body) {
        controls.push("input[].compaction".to_string());
    }

    if body.get("reasoning").is_some()
        && (!profile.reasoning_effort || responses_reasoning_has_nonportable_fields(body, profile))
    {
        controls.push("reasoning".to_string());
    }
    if body.get("text").is_some() && responses_text_has_nonportable_fields(body, profile) {
        controls.push("text".to_string());
    }
    if body.get("metadata").is_some() && !profile.metadata {
        controls.push("metadata".to_string());
    }
    if body.get("user").is_some() && !profile.user {
        controls.push("user".to_string());
    }
    if body.get("service_tier").is_some() && !profile.service_tier {
        controls.push("service_tier".to_string());
    }
    if body.get("prompt_cache_key").is_some() && !profile.prompt_cache_key {
        controls.push("prompt_cache_key".to_string());
    }
    if body.get("prompt_cache_retention").is_some() && !profile.prompt_cache_retention {
        controls.push("prompt_cache_retention".to_string());
    }
    if body.get("safety_identifier").is_some() && !profile.safety_identifier {
        controls.push("safety_identifier".to_string());
    }
    if responses_include_requests_output_text_logprobs(body)
        && !profile.top_logprobs
        && !controls.iter().any(|control| control == "include")
    {
        controls.push("include".to_string());
    }
    if body.get("top_logprobs").is_some() && !profile.top_logprobs {
        controls.push("top_logprobs".to_string());
    }
    if request_stream_include_obfuscation(body).is_some() && !profile.stream_include_obfuscation {
        controls.push("stream_options.include_obfuscation".to_string());
    }
    if responses_text_verbosity(body).is_some() && !profile.verbosity {
        controls.push("text.verbosity".to_string());
    }
    if responses_reasoning_effort(body).is_some() && !profile.reasoning_effort {
        controls.push("reasoning.effort".to_string());
    }
    if body.get("parallel_tool_calls").and_then(Value::as_bool) == Some(false)
        && !profile.parallel_tool_calls
    {
        controls.push("parallel_tool_calls".to_string());
    }
    controls
}

pub(super) fn responses_tool_choice_allowed_tools_array(
    choice: &serde_json::Map<String, Value>,
) -> Option<&Vec<Value>> {
    choice.get("tools").and_then(Value::as_array).or_else(|| {
        choice
            .get("allowed_tools")
            .and_then(Value::as_object)
            .and_then(|allowed_tools| allowed_tools.get("tools"))
            .and_then(Value::as_array)
    })
}

pub(super) fn openai_named_tool_choice_name<'a>(
    value: &'a Value,
    tool_type: &str,
) -> Option<&'a str> {
    let object = value.as_object()?;
    if object.get("type").and_then(Value::as_str) != Some(tool_type) {
        return None;
    }
    object
        .get(tool_type)
        .and_then(Value::as_object)
        .and_then(|named| named.get("name"))
        .or_else(|| object.get("name"))
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
}

pub(super) fn openai_tool_choice_contains_custom(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    match object.get("type").and_then(Value::as_str) {
        Some("custom") => openai_named_tool_choice_name(value, "custom").is_some(),
        Some("allowed_tools") => {
            let tools = object
                .get("allowed_tools")
                .and_then(Value::as_object)
                .and_then(|allowed_tools| allowed_tools.get("tools"))
                .or_else(|| object.get("tools"))
                .and_then(Value::as_array);
            tools
                .map(|tools| {
                    tools.iter().any(|tool| {
                        tool.get("type").and_then(Value::as_str) == Some("custom")
                            && openai_named_tool_choice_name(tool, "custom").is_some()
                    })
                })
                .unwrap_or(false)
        }
        _ => false,
    }
}

pub(super) fn responses_nonportable_tool_choice_message(
    body: &Value,
    target_format: UpstreamFormat,
) -> Option<String> {
    let target_label = translation_target_label(target_format);
    let tool_choice = body.get("tool_choice").filter(|value| !value.is_null())?;
    if tool_choice.is_string() {
        return None;
    }
    let tool_choice = tool_choice.as_object()?;
    let choice_type = tool_choice.get("type").and_then(Value::as_str)?;
    match choice_type {
        "function" => None,
        "custom" => match target_format {
            UpstreamFormat::OpenAiCompletion | UpstreamFormat::Anthropic => None,
            _ => Some(format!(
                "OpenAI Responses tool_choice.type `custom` cannot be faithfully translated to {target_label}"
            )),
        },
        "allowed_tools" => responses_tool_choice_allowed_tools_array(tool_choice).and_then(
            |tools| {
                tools.iter().find_map(|tool| match tool.get("type").and_then(Value::as_str) {
                    Some("function") => None,
                    Some("custom")
                        if matches!(
                            target_format,
                            UpstreamFormat::OpenAiCompletion | UpstreamFormat::Anthropic
                        ) =>
                    {
                        None
                    }
                    Some("custom") => Some(format!(
                        "OpenAI Responses tool_choice.allowed_tools selected custom tool `{}` and cannot be faithfully translated to {target_label}",
                        tool.get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                    )),
                    Some("namespace") => Some(format!(
                        "OpenAI Responses tool_choice.allowed_tools selected namespace tool `{}` and cannot be faithfully translated to {target_label}",
                        tool.get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                    )),
                    Some(other) => Some(format!(
                        "OpenAI Responses tool_choice.allowed_tools selected hosted/built-in tool `{other}` and cannot be faithfully translated to {target_label}"
                    )),
                    None => Some(format!(
                        "OpenAI Responses tool_choice.allowed_tools selected an unnamed tool that cannot be faithfully translated to {target_label}"
                    )),
                })
            },
        ),
        other => Some(format!(
            "OpenAI Responses tool_choice.type `{other}` cannot be faithfully translated to {target_label}"
        )),
    }
}

pub(super) fn responses_nonportable_tool_definition_message(
    body: &Value,
    target_label: &str,
) -> Option<String> {
    let tools = body.get("tools").and_then(Value::as_array)?;
    tools.iter().find_map(|tool| match normalized_responses_tool_definition(tool) {
        Ok(Some(NormalizedOpenAiFamilyToolDef::Namespace(namespace))) => Some(format!(
            "OpenAI Responses namespace tool `{}` cannot be faithfully translated to {target_label}",
            namespace.name
        )),
        Err(message) => Some(message),
        _ => None,
    })
}

pub(super) fn responses_has_warning_only_nonportable_tool_definitions(body: &Value) -> bool {
    body.get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools.iter().any(|tool| {
                normalized_responses_tool_definition(tool)
                    .ok()
                    .flatten()
                    .is_none()
            })
        })
        .unwrap_or(false)
}

pub(super) fn responses_custom_tool_format_reject_message(
    body: &Value,
    target_format: UpstreamFormat,
) -> Option<String> {
    if !matches!(
        target_format,
        UpstreamFormat::OpenAiCompletion | UpstreamFormat::Anthropic
    ) {
        return None;
    }

    let tools = body.get("tools").and_then(Value::as_array)?;
    tools
        .iter()
        .find_map(|tool| match normalized_responses_tool_definition(tool) {
            Ok(Some(NormalizedOpenAiFamilyToolDef::Custom(custom)))
                if !openai_custom_tool_format_supports_anthropic_bridge(custom.format.as_ref()) =>
            {
                Some(custom_tool_format_reject_message(
                    "OpenAI Responses",
                    &custom.name,
                    translation_target_label(target_format),
                ))
            }
            _ => None,
        })
}

fn responses_custom_tool_bridge_warning_messages(
    body: &Value,
    target_format: UpstreamFormat,
) -> Vec<String> {
    if !matches!(
        target_format,
        UpstreamFormat::OpenAiCompletion | UpstreamFormat::Anthropic
    ) {
        return Vec::new();
    }

    body.get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| match normalized_responses_tool_definition(tool) {
                    Ok(Some(NormalizedOpenAiFamilyToolDef::Custom(custom)))
                        if openai_custom_tool_format_supports_anthropic_bridge(
                            custom.format.as_ref(),
                        ) && !openai_custom_tool_format_is_plain_text(
                            custom.format.as_ref(),
                        ) =>
                    {
                        Some(custom_tool_format_downgraded_message(
                            "OpenAI Responses",
                            &custom.name,
                            translation_target_label(target_format),
                        ))
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn responses_hosted_input_item_type(item_type: &str) -> bool {
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

pub(super) fn responses_portable_input_item_type(item_type: &str) -> bool {
    matches!(
        item_type,
        "message"
            | "function_call"
            | "custom_tool_call"
            | "function_call_output"
            | "custom_tool_call_output"
            | "reasoning"
    )
}

fn responses_input_compaction_carrier_present(body: &Value) -> bool {
    body.get("input")
        .and_then(Value::as_array)
        .map(|items| items.iter().any(responses_input_item_is_compaction))
        .unwrap_or(false)
}

fn responses_request_has_visible_portable_context(body: &Value) -> bool {
    match body.get("input") {
        Some(Value::String(text)) => !text.trim().is_empty(),
        Some(Value::Array(items)) => items
            .iter()
            .any(responses_input_item_has_visible_portable_context),
        _ => false,
    }
}

fn responses_request_has_non_compaction_visible_portable_context(body: &Value) -> bool {
    body.get("input")
        .and_then(Value::as_array)
        .map(|items| {
            items.iter().any(|item| {
                !responses_input_item_is_compaction(item)
                    && responses_input_item_has_visible_portable_context(item)
            })
        })
        .unwrap_or(false)
}

fn responses_compaction_item_can_drop_opaque_state(
    item: &Value,
    request_has_non_compaction_visible_context: bool,
) -> bool {
    !responses_compaction_summary_text(item).trim().is_empty()
        || request_has_non_compaction_visible_context
}

fn responses_reasoning_item_can_drop_opaque_state(
    item: &Value,
    request_has_visible_context: bool,
) -> bool {
    !responses_reasoning_summary_text(item).trim().is_empty() || request_has_visible_context
}

fn responses_input_item_has_visible_portable_context(item: &Value) -> bool {
    match responses_input_item_type(item) {
        Some("message") => responses_content_has_visible_portable_context(item.get("content")),
        Some("reasoning") => !responses_reasoning_summary_text(item).trim().is_empty(),
        Some("compaction" | "compaction_summary") => {
            !responses_compaction_summary_text(item).trim().is_empty()
        }
        _ => false,
    }
}

fn responses_content_has_visible_portable_context(content: Option<&Value>) -> bool {
    match content {
        Some(Value::String(text)) => !text.trim().is_empty(),
        Some(Value::Array(parts)) => parts
            .iter()
            .any(responses_content_part_has_visible_portable_context),
        Some(Value::Object(_)) => {
            content.is_some_and(responses_content_part_has_visible_portable_context)
        }
        _ => false,
    }
}

fn responses_content_part_has_visible_portable_context(part: &Value) -> bool {
    match part.get("type").and_then(Value::as_str) {
        Some("input_text" | "output_text" | "refusal") => part
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|text| !text.trim().is_empty()),
        Some("input_image" | "image_url") => part.get("image_url").is_some(),
        Some("input_audio") => part.get("input_audio").is_some(),
        Some("input_file" | "file") => [
            "file_id",
            "file_data",
            "file_url",
            "filename",
            "mime_type",
            "mimeType",
        ]
        .iter()
        .any(|field| part.get(*field).is_some()),
        _ => false,
    }
}

pub(super) fn responses_nonportable_input_item_message(
    body: &Value,
    target_format: UpstreamFormat,
) -> Option<String> {
    let target_label = translation_target_label(target_format);
    let items = body.get("input").and_then(Value::as_array)?;
    let has_non_compaction_visible_portable_context =
        responses_request_has_non_compaction_visible_portable_context(body);
    items.iter().find_map(|item| {
        let item_type = responses_input_item_type(item)?;
        if matches!(item_type, "function_call" | "custom_tool_call")
            && item.get("namespace").is_some()
        {
            return Some(format!(
                "OpenAI Responses namespaced tool call `{}` cannot be faithfully translated to {target_label}",
                item.get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            ));
        }
        if responses_input_item_is_compaction(item) {
            if responses_compaction_item_can_drop_opaque_state(
                item,
                has_non_compaction_visible_portable_context,
            )
            {
                return None;
            }
            return Some(format!(
                "OpenAI Responses compaction input item contains provider-owned opaque state and cannot be safely dropped when translating to {target_label} because no visible portable transcript or summary remains"
            ));
        }
        if responses_portable_input_item_type(item_type) {
            return None;
        }
        if responses_hosted_input_item_type(item_type) {
            return Some(format!(
                "OpenAI Responses input item `{item_type}` cannot be faithfully translated to {target_label}"
            ));
        }
        Some(format!(
            "OpenAI Responses input item type `{item_type}` is outside the portable cross-protocol subset and cannot be faithfully translated to {target_label}"
        ))
    })
}

fn responses_input_reasoning_encrypted_content_present(body: &Value) -> bool {
    body.get("input")
        .and_then(Value::as_array)
        .map(|items| {
            items.iter().any(|item| {
                responses_input_item_type(item) == Some("reasoning")
                    && item.get("encrypted_content").is_some()
            })
        })
        .unwrap_or(false)
}

fn responses_input_reasoning_encrypted_content_requires_native_continuity(body: &Value) -> bool {
    let has_visible_portable_context = responses_request_has_visible_portable_context(body);
    body.get("input")
        .and_then(Value::as_array)
        .map(|items| {
            items.iter().any(|item| {
                responses_input_item_type(item) == Some("reasoning")
                    && item.get("encrypted_content").is_some()
                    && !responses_reasoning_item_can_drop_opaque_state(
                        item,
                        has_visible_portable_context,
                    )
            })
        })
        .unwrap_or(false)
}

pub(super) fn responses_reasoning_continuity_request_message(
    body: &Value,
    target_format: UpstreamFormat,
) -> Option<String> {
    if responses_input_reasoning_encrypted_content_requires_native_continuity(body) {
        let target_label = translation_target_label(target_format);
        return Some(responses_reasoning_continuity_not_portable_message(
            "input[].reasoning.encrypted_content",
            target_label,
        ));
    }
    None
}

pub(super) fn cross_protocol_requested_choice_count(
    client_format: UpstreamFormat,
    body: &Value,
) -> Option<(&'static str, u64)> {
    match client_format {
        UpstreamFormat::OpenAiCompletion => body.get("n").and_then(Value::as_u64).map(|n| ("n", n)),
        _ => None,
    }
}

pub(super) fn cross_protocol_requested_choice_count_message(
    client_format: UpstreamFormat,
    upstream_format: UpstreamFormat,
    body: &Value,
) -> Option<String> {
    let (field_name, count) = cross_protocol_requested_choice_count(client_format, body)?;
    if count <= 1 {
        return None;
    }
    Some(single_candidate_choice_contract_message(
        translation_target_label(client_format),
        translation_target_label(upstream_format),
        field_name,
        count as usize,
    ))
}

pub(super) fn request_has_custom_tools(client_format: UpstreamFormat, body: &Value) -> bool {
    match client_format {
        UpstreamFormat::OpenAiCompletion => {
            body.get("tools")
                .and_then(Value::as_array)
                .map(|tools| {
                    tools.iter().any(|tool| {
                        semantic_tool_kind_from_value(tool) == SemanticToolKind::OpenAiCustom
                    })
                })
                .unwrap_or(false)
                || body
                    .get("messages")
                    .and_then(Value::as_array)
                    .map(|messages| {
                        messages.iter().any(|message| {
                            message
                                .get("tool_calls")
                                .and_then(Value::as_array)
                                .map(|tool_calls| {
                                    tool_calls.iter().any(|tool_call| {
                                        semantic_tool_kind_from_value(tool_call)
                                            == SemanticToolKind::OpenAiCustom
                                    })
                                })
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false)
                || body
                    .get("tool_choice")
                    .map(openai_tool_choice_contains_custom)
                    .unwrap_or(false)
        }
        UpstreamFormat::OpenAiResponses => {
            body.get("tools")
                .and_then(Value::as_array)
                .map(|tools| {
                    tools.iter().any(|tool| {
                        semantic_tool_kind_from_value(tool) == SemanticToolKind::OpenAiCustom
                    })
                })
                .unwrap_or(false)
                || body
                    .get("input")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items.iter().any(|item| {
                            responses_tool_call_item_to_openai_tool_call(item)
                                .map(|tool_call| {
                                    semantic_tool_kind_from_value(&tool_call)
                                        == SemanticToolKind::OpenAiCustom
                                })
                                .unwrap_or_else(|| {
                                    semantic_tool_kind_from_value(item)
                                        == SemanticToolKind::OpenAiCustom
                                })
                        })
                    })
                    .unwrap_or(false)
                || body
                    .get("tool_choice")
                    .map(openai_tool_choice_contains_custom)
                    .unwrap_or(false)
        }
        _ => false,
    }
}

pub(super) fn request_invalid_structured_tool_arguments_message(
    client_format: UpstreamFormat,
    body: &Value,
    target_label: &str,
) -> Option<String> {
    match client_format {
        UpstreamFormat::OpenAiCompletion => body
            .get("messages")
            .and_then(Value::as_array)
            .and_then(|messages| {
                messages.iter().find_map(|message| {
                    message
                        .get("tool_calls")
                        .and_then(Value::as_array)
                        .and_then(|tool_calls| {
                            tool_calls.iter().find_map(|tool_call| {
                                (semantic_tool_kind_from_value(tool_call)
                                    != SemanticToolKind::OpenAiCustom
                                    && !tool_call_is_marked_non_replayable(tool_call))
                                .then(|| {
                                    openai_tool_arguments_to_structured_value(
                                        tool_call,
                                        target_label,
                                    )
                                    .err()
                                })
                                .flatten()
                            })
                        })
                })
            }),
        UpstreamFormat::OpenAiResponses => {
            body.get("input")
                .and_then(Value::as_array)
                .and_then(|items| {
                    items.iter().find_map(|item| {
                        matches!(
                            item.get("type").and_then(Value::as_str),
                            Some("function_call") | Some("custom_tool_call")
                        )
                        .then(|| {
                            (semantic_tool_kind_from_value(item) != SemanticToolKind::OpenAiCustom
                                && !tool_call_is_marked_non_replayable(item))
                            .then(|| {
                                responses_tool_call_to_structured_value(item, target_label).err()
                            })
                            .flatten()
                        })
                        .flatten()
                    })
                })
        }
        _ => None,
    }
}

fn assess_surface_request_policy(
    assessment: &mut TranslationAssessment,
    client_format: UpstreamFormat,
    body: &Value,
    surface: &ModelSurface,
) {
    if surface
        .tools
        .as_ref()
        .and_then(|tools| tools.supports_parallel_calls)
        == Some(false)
        && request_has_surface_tooling(client_format, body)
        && request_explicitly_enables_parallel_tool_calls(client_format, body)
    {
        assessment.reject(
            "request explicitly enables parallel tool execution (`parallel_tool_calls=true` or `tool_choice.disable_parallel_tool_use=false`), but model surface `tools.supports_parallel_calls=false`"
                .to_string(),
        );
    }

    for modality in request_input_modalities(client_format, body) {
        if !surface_allows_modality(
            surface
                .modalities
                .as_ref()
                .and_then(|modalities| modalities.input.as_ref()),
            modality,
        ) {
            assessment.reject(format!(
                "request uses {} input, but model surface `modalities.input` does not include `{}`",
                modality_label(modality),
                modality_label(modality),
            ));
        }
    }

    for modality in request_output_modalities(client_format, body) {
        if !surface_allows_modality(
            surface
                .modalities
                .as_ref()
                .and_then(|modalities| modalities.output.as_ref()),
            modality,
        ) {
            assessment.reject(format!(
                "request asks for {} output, but model surface `modalities.output` does not include `{}`",
                modality_label(modality),
                modality_label(modality),
            ));
        }
    }
}

fn surface_allows_modality(allowed: Option<&Vec<ModelModality>>, modality: ModelModality) -> bool {
    allowed
        .map(|allowed| {
            allowed.contains(&modality)
                || (modality == ModelModality::Pdf && allowed.contains(&ModelModality::File))
        })
        .unwrap_or(true)
}

fn modality_label(modality: ModelModality) -> &'static str {
    match modality {
        ModelModality::Text => "text",
        ModelModality::Image => "image",
        ModelModality::Audio => "audio",
        ModelModality::Pdf => "pdf",
        ModelModality::File => "file",
        ModelModality::Video => "video",
    }
}

fn request_has_surface_tooling(client_format: UpstreamFormat, body: &Value) -> bool {
    match client_format {
        UpstreamFormat::OpenAiCompletion
        | UpstreamFormat::OpenAiResponses
        | UpstreamFormat::Anthropic => body
            .get("tools")
            .and_then(Value::as_array)
            .is_some_and(|tools| !tools.is_empty()),
    }
}

fn request_explicitly_enables_parallel_tool_calls(
    client_format: UpstreamFormat,
    body: &Value,
) -> bool {
    match client_format {
        UpstreamFormat::OpenAiCompletion | UpstreamFormat::OpenAiResponses => {
            body.get("parallel_tool_calls").and_then(Value::as_bool) == Some(true)
        }
        UpstreamFormat::Anthropic => {
            body.get("tool_choice")
                .and_then(Value::as_object)
                .and_then(|tool_choice| tool_choice.get("disable_parallel_tool_use"))
                .and_then(Value::as_bool)
                == Some(false)
        }
    }
}

fn openai_request_file_mime_conflict_message(
    client_format: UpstreamFormat,
    body: &Value,
) -> Option<String> {
    match client_format {
        UpstreamFormat::OpenAiCompletion => openai_completion_file_mime_conflict_message(body),
        UpstreamFormat::OpenAiResponses => openai_responses_file_mime_conflict_message(body),
        UpstreamFormat::Anthropic => None,
    }
}

fn openai_completion_file_mime_conflict_message(body: &Value) -> Option<String> {
    body.get("messages")
        .and_then(Value::as_array)?
        .iter()
        .find_map(|message| openai_content_file_mime_conflict_message(message.get("content")))
}

fn openai_responses_file_mime_conflict_message(body: &Value) -> Option<String> {
    body.get("input")
        .and_then(Value::as_array)?
        .iter()
        .find_map(openai_responses_input_item_file_mime_conflict_message)
}

fn openai_responses_input_item_file_mime_conflict_message(item: &Value) -> Option<String> {
    openai_input_part_file_mime_conflict_message(item).or_else(|| {
        responses_input_item_is_message(item)
            .then(|| openai_content_file_mime_conflict_message(item.get("content")))
            .flatten()
    })
}

fn openai_content_file_mime_conflict_message(content: Option<&Value>) -> Option<String> {
    match content {
        Some(Value::Array(parts)) => parts
            .iter()
            .find_map(openai_input_part_file_mime_conflict_message),
        Some(Value::Object(_)) => content.and_then(openai_input_part_file_mime_conflict_message),
        _ => None,
    }
}

fn openai_input_part_file_mime_conflict_message(part: &Value) -> Option<String> {
    matches!(
        part.get("type").and_then(Value::as_str),
        Some("file") | Some("input_file")
    )
    .then(|| openai_file_part_mime_conflict_message(part))
    .flatten()
}

fn request_input_modalities(
    client_format: UpstreamFormat,
    body: &Value,
) -> BTreeSet<ModelModality> {
    let mut modalities = BTreeSet::new();
    match client_format {
        UpstreamFormat::OpenAiCompletion => {
            openai_collect_completion_input_modalities(body, &mut modalities);
        }
        UpstreamFormat::OpenAiResponses => {
            openai_collect_responses_input_modalities(body, &mut modalities);
        }
        UpstreamFormat::Anthropic => {
            anthropic_collect_request_input_modalities(body, &mut modalities);
        }
    }
    modalities
}

fn insert_input_modality(modalities: &mut BTreeSet<ModelModality>, modality: ModelModality) {
    if modality != ModelModality::Text {
        modalities.insert(modality);
    }
}

fn anthropic_collect_request_input_modalities(
    body: &Value,
    modalities: &mut BTreeSet<ModelModality>,
) {
    anthropic_collect_content_input_modalities(body.get("system"), modalities);
    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        for message in messages {
            anthropic_collect_content_input_modalities(message.get("content"), modalities);
        }
    }
}

fn anthropic_collect_content_input_modalities(
    content: Option<&Value>,
    modalities: &mut BTreeSet<ModelModality>,
) {
    match content {
        Some(Value::Array(blocks)) => {
            for block in blocks {
                anthropic_collect_block_input_modalities(block, modalities);
            }
        }
        Some(Value::Object(_)) => {
            if let Some(block) = content {
                anthropic_collect_block_input_modalities(block, modalities);
            }
        }
        _ => {}
    }
}

fn anthropic_collect_block_input_modalities(
    block: &Value,
    modalities: &mut BTreeSet<ModelModality>,
) {
    match block.get("type").and_then(Value::as_str) {
        Some("image") => insert_input_modality(modalities, ModelModality::Image),
        Some("audio") => insert_input_modality(modalities, ModelModality::Audio),
        Some("document") => {
            let modality = block
                .get("source")
                .and_then(|source| source.get("media_type"))
                .and_then(Value::as_str)
                .and_then(mime_type_to_input_modality)
                .filter(|modality| *modality == ModelModality::Pdf)
                .unwrap_or(ModelModality::File);
            insert_input_modality(modalities, modality);
        }
        _ => {
            if let Some(modality) = block
                .get("source")
                .and_then(|source| source.get("media_type"))
                .and_then(Value::as_str)
                .and_then(mime_type_to_input_modality)
            {
                insert_input_modality(modalities, modality);
            }
        }
    }
}

fn openai_collect_completion_input_modalities(
    body: &Value,
    modalities: &mut BTreeSet<ModelModality>,
) {
    if openai_assistant_history_audio_present(body) {
        insert_input_modality(modalities, ModelModality::Audio);
    }

    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        for message in messages {
            openai_collect_completion_message_input_modalities(message, modalities);
        }
    }
}

fn openai_collect_completion_message_input_modalities(
    message: &Value,
    modalities: &mut BTreeSet<ModelModality>,
) {
    if message
        .get("audio")
        .filter(|audio| !audio.is_null())
        .is_some()
    {
        insert_input_modality(modalities, ModelModality::Audio);
    }

    if let Some(parts) = message.get("content").and_then(Value::as_array) {
        for part in parts {
            openai_collect_input_part_modalities(part, modalities);
        }
    }
}

fn openai_collect_responses_input_modalities(
    body: &Value,
    modalities: &mut BTreeSet<ModelModality>,
) {
    if let Some(items) = body.get("input").and_then(Value::as_array) {
        for item in items {
            openai_collect_responses_input_item_modalities(item, modalities);
        }
    }
}

fn openai_collect_responses_input_item_modalities(
    item: &Value,
    modalities: &mut BTreeSet<ModelModality>,
) {
    openai_collect_input_part_modalities(item, modalities);

    match responses_input_item_type(item) {
        Some("message") => {
            if let Some(parts) = item.get("content").and_then(Value::as_array) {
                for part in parts {
                    openai_collect_input_part_modalities(part, modalities);
                }
            }
        }
        Some("output_audio") => insert_input_modality(modalities, ModelModality::Audio),
        _ => {}
    }
}

fn openai_collect_input_part_modalities(part: &Value, modalities: &mut BTreeSet<ModelModality>) {
    match part.get("type").and_then(Value::as_str) {
        Some("image_url") | Some("input_image") => {
            insert_input_modality(modalities, ModelModality::Image);
        }
        Some("input_audio") => {
            insert_input_modality(modalities, ModelModality::Audio);
        }
        Some("file") | Some("input_file") => {
            insert_input_modality(modalities, openai_file_part_modality(part));
        }
        _ => {}
    }
}

fn openai_file_part_modality(part: &Value) -> ModelModality {
    openai_file_part_resolved_mime_type(part)
        .ok()
        .flatten()
        .and_then(|mime_type| mime_type_to_input_modality(&mime_type))
        .unwrap_or(ModelModality::File)
}

fn mime_type_to_input_modality(mime_type: &str) -> Option<ModelModality> {
    let mime_type = mime_type
        .split_once(';')
        .map_or(mime_type, |(mime_type, _)| mime_type)
        .trim()
        .to_ascii_lowercase();
    if mime_type.is_empty() {
        return None;
    }
    if mime_type.starts_with("text/") {
        Some(ModelModality::Text)
    } else if mime_type.starts_with("image/") {
        Some(ModelModality::Image)
    } else if mime_type.starts_with("audio/") {
        Some(ModelModality::Audio)
    } else if mime_type.starts_with("video/") {
        Some(ModelModality::Video)
    } else if mime_type == "application/pdf" {
        Some(ModelModality::Pdf)
    } else {
        Some(ModelModality::File)
    }
}

fn request_output_modalities(
    client_format: UpstreamFormat,
    body: &Value,
) -> BTreeSet<ModelModality> {
    let mut modalities = BTreeSet::new();
    match client_format {
        UpstreamFormat::OpenAiCompletion | UpstreamFormat::OpenAiResponses => {
            openai_collect_output_modalities(body, &mut modalities);
        }
        UpstreamFormat::Anthropic => {}
    }
    modalities
}

fn openai_collect_output_modalities(body: &Value, modalities: &mut BTreeSet<ModelModality>) {
    if body.get("audio").is_some()
        || body
            .get("modalities")
            .and_then(Value::as_array)
            .is_some_and(|modalities| {
                modalities.iter().any(|requested_modality| {
                    requested_modality
                        .as_str()
                        .is_some_and(|requested_modality| {
                            requested_modality.eq_ignore_ascii_case("audio")
                        })
                })
            })
    {
        modalities.insert(ModelModality::Audio);
    }
}

pub(crate) fn assess_request_translation_with_surface(
    client_format: UpstreamFormat,
    upstream_format: UpstreamFormat,
    body: &Value,
    surface: &ModelSurface,
) -> TranslationAssessment {
    let mut assessment = TranslationAssessment::default();
    assess_surface_request_policy(&mut assessment, client_format, body, surface);
    assessment
        .issues
        .extend(assess_request_translation(client_format, upstream_format, body).issues);
    assessment
}

pub(super) fn anthropic_warning_only_request_controls_for_translate(
    body: &Value,
) -> Vec<&'static str> {
    let mut controls = Vec::new();
    for field in ["top_k", "service_tier"] {
        if body.get(field).is_some() {
            controls.push(field);
        }
    }
    if anthropic_protocol_uses_cache_control(body) {
        controls.push("cache_control");
    }
    controls
}

pub(super) fn anthropic_nonportable_request_controls_for_translate(
    body: &Value,
) -> Vec<&'static str> {
    let mut controls = Vec::new();
    for field in ["container", "thinking", "context_management"] {
        if body.get(field).is_some() {
            controls.push(field);
        }
    }
    controls
}

pub(crate) fn assess_request_translation(
    client_format: UpstreamFormat,
    upstream_format: UpstreamFormat,
    body: &Value,
) -> TranslationAssessment {
    let mut assessment = TranslationAssessment::default();

    if let Some(message) = openai_request_file_mime_conflict_message(client_format, body) {
        assessment.reject(message);
    }

    if client_format == upstream_format {
        return assessment;
    }

    if let Some(message) =
        cross_protocol_requested_choice_count_message(client_format, upstream_format, body)
    {
        assessment.reject(message);
    }

    if client_format == UpstreamFormat::OpenAiResponses
        && upstream_format != UpstreamFormat::OpenAiResponses
    {
        let controls = responses_stateful_request_controls_for_translate(body);
        if !controls.is_empty() {
            let quoted = controls
                .iter()
                .map(|field| format!("`{field}`"))
                .collect::<Vec<_>>()
                .join(", ");
            assessment.reject(format!(
                "Responses request controls {quoted} require a native OpenAI Responses upstream and cannot be translated to {upstream_format}; the proxy does not reconstruct provider state"
            ));
        }
        if let Some(message) = responses_reasoning_continuity_request_message(body, upstream_format)
        {
            assessment.reject(message);
        }
        let dropped_controls =
            responses_warning_only_request_controls_for_translate(body, upstream_format);
        if !dropped_controls.is_empty() {
            let quoted = dropped_controls
                .iter()
                .map(|field| format!("`{field}`"))
                .collect::<Vec<_>>()
                .join(", ");
            assessment.warning(format!(
                "OpenAI Responses controls {quoted} are not portable on this translation path to {} and will be dropped",
                translation_target_label(upstream_format)
            ));
        }
        if let Some(message) = responses_nonportable_tool_choice_message(body, upstream_format) {
            assessment.reject(message);
        }
        if let Some(message) = responses_nonportable_input_item_message(body, upstream_format) {
            assessment.reject(message);
        }
        if let Some(message) = responses_nonportable_tool_definition_message(
            body,
            translation_target_label(upstream_format),
        ) {
            assessment.reject(message);
        } else if responses_has_warning_only_nonportable_tool_definitions(body) {
            assessment.warning(format!(
                "non-function Responses tools are not portable to {upstream_format} and will be dropped"
            ));
        }
        if let Some(message) = responses_custom_tool_format_reject_message(body, upstream_format) {
            assessment.reject(message);
        }
        for warning in responses_custom_tool_bridge_warning_messages(body, upstream_format) {
            assessment.warning(warning);
        }
    }

    if body.get("store").is_some()
        && !(client_format == UpstreamFormat::OpenAiResponses
            && body.get("store").and_then(Value::as_bool) == Some(true))
    {
        assessment.warning(cross_protocol_store_warning_message(
            client_format,
            upstream_format,
        ));
    }

    if client_format == UpstreamFormat::OpenAiCompletion
        && upstream_format == UpstreamFormat::OpenAiResponses
    {
        if let Some(message) = normalized_openai_audio_contract(body).err().or_else(|| {
            normalized_openai_audio_contract(body)
                .ok()
                .flatten()
                .map(|_| openai_request_audio_not_portable_message("OpenAI Responses"))
        }) {
            assessment.reject(message);
        }
        if openai_assistant_history_audio_present(body) {
            assessment.reject(openai_assistant_audio_history_not_portable_message(
                "OpenAI Responses",
            ));
        }
        let controls = openai_to_responses_dropped_control_names(body);
        if !controls.is_empty() {
            let quoted = controls
                .iter()
                .map(|field| format!("`{field}`"))
                .collect::<Vec<_>>()
                .join(", ");
            assessment.warning(format!(
                "OpenAI Chat Completions controls {quoted} have no direct OpenAI Responses equivalent in this translator and will be dropped"
            ));
        }
    }

    if client_format == UpstreamFormat::OpenAiCompletion
        && upstream_format == UpstreamFormat::Anthropic
    {
        if let Some(message) = normalized_openai_audio_contract(body).err().or_else(|| {
            normalized_openai_audio_contract(body)
                .ok()
                .flatten()
                .map(|_| openai_request_audio_not_portable_message("Anthropic"))
        }) {
            assessment.reject(message);
        }
        if openai_assistant_history_audio_present(body) {
            assessment.reject(openai_assistant_audio_history_not_portable_message(
                "Anthropic",
            ));
        }
        let mut controls = openai_to_anthropic_dropped_control_names(body)
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        controls.extend(openai_warning_only_request_controls_for_translate(
            body,
            upstream_format,
        ));
        if !controls.is_empty() {
            let quoted = controls
                .iter()
                .map(|field| format!("`{field}`"))
                .collect::<Vec<_>>()
                .join(", ");
            assessment.warning(format!(
                "OpenAI Chat Completions controls {quoted} are not portable to Anthropic and will be dropped"
            ));
        }
    }

    if client_format == UpstreamFormat::Anthropic && upstream_format != UpstreamFormat::Anthropic {
        let reject_controls = anthropic_nonportable_request_controls_for_translate(body);
        if !reject_controls.is_empty() {
            let quoted = reject_controls
                .iter()
                .map(|field| format!("`{field}`"))
                .collect::<Vec<_>>()
                .join(", ");
            assessment.reject(format!(
                "Anthropic request controls {quoted} require native provider semantics and cannot be faithfully translated to {}",
                translation_target_label(upstream_format)
            ));
        }
        let warning_controls = anthropic_warning_only_request_controls_for_translate(body);
        if !warning_controls.is_empty() {
            let quoted = warning_controls
                .iter()
                .map(|field| format!("`{field}`"))
                .collect::<Vec<_>>()
                .join(", ");
            assessment.warning(format!(
                "Anthropic request controls {quoted} are not portable to {} and will be dropped",
                translation_target_label(upstream_format)
            ));
        }
        if anthropic_request_has_nonportable_thinking_provenance(body) {
            assessment.reject(format!(
                "Anthropic thinking content blocks with `signature` or omitted/non-string `thinking` require native Anthropic semantics and cannot be faithfully translated to {}",
                translation_target_label(upstream_format)
            ));
        }
        if let Some(message) = anthropic_request_nonportable_tool_definition_message(
            body,
            translation_target_label(upstream_format),
        ) {
            assessment.reject(message);
        }
        if let Some(message) = anthropic_request_tool_result_order_message(
            body,
            translation_target_label(upstream_format),
        ) {
            assessment.reject(message);
        }
        if let Some(message) = anthropic_nonportable_content_block_message(
            body,
            translation_target_label(upstream_format),
        ) {
            assessment.reject(message);
        }
    }

    let responses_custom_bridge_supported = client_format == UpstreamFormat::OpenAiResponses
        && upstream_format == UpstreamFormat::Anthropic;
    if upstream_format == UpstreamFormat::Anthropic
        && request_has_custom_tools(client_format, body)
        && !responses_custom_bridge_supported
    {
        assessment.reject(custom_tools_not_portable_message(upstream_format));
    }

    if upstream_format == UpstreamFormat::Anthropic {
        if let Some(message) = request_invalid_structured_tool_arguments_message(
            client_format,
            body,
            translation_target_label(upstream_format),
        ) {
            assessment.reject(message);
        }
    }

    assessment
}
