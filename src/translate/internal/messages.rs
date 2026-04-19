use serde_json::Value;

use crate::formats::UpstreamFormat;

pub(crate) const OPENAI_REASONING_TO_ANTHROPIC_REJECT_MESSAGE: &str =
    "OpenAI reasoning cannot be replayed to Anthropic without provenance; refusing to translate reasoning as plain text";

pub(crate) fn translation_target_label(format: UpstreamFormat) -> &'static str {
    match format {
        UpstreamFormat::OpenAiCompletion => "OpenAI Chat Completions",
        UpstreamFormat::OpenAiResponses => "OpenAI Responses",
        UpstreamFormat::Anthropic => "Anthropic",
        UpstreamFormat::Google => "Gemini",
    }
}

pub(crate) fn single_candidate_choice_contract_message(
    source_label: &str,
    target_label: &str,
    field_name: &str,
    count: usize,
) -> String {
    format!(
        "{source_label} field `{field_name}` has {count} items; cross-protocol translation to {target_label} only supports a single candidate/choice"
    )
}

pub(crate) fn single_required_array_item<'a>(
    items: Option<&'a [Value]>,
    source_label: &str,
    target_label: &str,
    field_name: &str,
) -> Result<&'a Value, String> {
    match items {
        Some([item]) => Ok(item),
        Some([]) => Err(format!("missing {field_name}")),
        Some(items) => Err(single_candidate_choice_contract_message(
            source_label,
            target_label,
            field_name,
            items.len(),
        )),
        None => Err(format!("missing {field_name}")),
    }
}

pub(crate) fn single_optional_array_item<'a>(
    items: Option<&'a [Value]>,
    source_label: &str,
    target_label: &str,
    field_name: &str,
) -> Result<Option<&'a Value>, String> {
    match items {
        Some([item]) => Ok(Some(item)),
        Some([]) | None => Ok(None),
        Some(items) => Err(single_candidate_choice_contract_message(
            source_label,
            target_label,
            field_name,
            items.len(),
        )),
    }
}

pub(crate) fn custom_tools_not_portable_message(upstream_format: UpstreamFormat) -> String {
    format!(
        "OpenAI custom tools cannot be faithfully translated to {}; refusing to downgrade them to function tools",
        translation_target_label(upstream_format)
    )
}

pub(crate) fn reserved_openai_custom_bridge_prefix_message(name: &str) -> String {
    format!(
        "OpenAI Responses function name `{name}` uses reserved bridge prefix `__llmup_custom__`; this namespace is reserved for synthetic custom-tool bridging to OpenAI Chat Completions"
    )
}

pub(crate) fn anthropic_thinking_provenance_dropped_message(target_label: &str) -> String {
    format!(
        "Anthropic thinking provenance (`signature` or omitted thinking) is not portable to {target_label}; provenance-only reasoning details will be dropped while preserving any portable assistant, tool, and visible text semantics"
    )
}

pub(crate) fn anthropic_request_tool_definition_not_portable_message(
    detail: &str,
    target_label: &str,
) -> String {
    format!(
        "Anthropic tool definitions with {detail} cannot be faithfully translated to {target_label}"
    )
}

pub(crate) fn anthropic_tool_result_order_not_portable_message(target_label: &str) -> String {
    format!(
        "Anthropic user turns that mix `tool_result` blocks with surrounding content cannot be faithfully translated to {target_label} without reordering blocks"
    )
}

pub(crate) fn gemini_function_response_parts_not_portable_message(target_label: &str) -> String {
    format!("Gemini functionResponse.parts cannot be faithfully translated to {target_label}")
}

pub(crate) fn openai_assistant_audio_not_portable_message(target_label: &str) -> String {
    format!("OpenAI assistant audio output cannot be faithfully translated to {target_label}")
}

pub(crate) fn openai_assistant_audio_field_not_portable_message(
    field: &str,
    target_label: &str,
) -> String {
    format!(
        "OpenAI assistant audio field `{field}` cannot be faithfully translated to {target_label}"
    )
}

pub(crate) fn openai_request_audio_not_portable_message(target_label: &str) -> String {
    format!("OpenAI Chat audio output intent cannot be faithfully translated to {target_label}")
}

pub(crate) fn openai_assistant_audio_history_not_portable_message(target_label: &str) -> String {
    format!(
        "OpenAI assistant history field `messages[].audio` cannot be faithfully translated to {target_label}"
    )
}

pub(crate) fn responses_multiple_output_audio_items_not_portable_message(
    target_label: &str,
) -> String {
    format!(
        "OpenAI Responses output has multiple `output_audio` items and cannot be faithfully translated to {target_label}"
    )
}
