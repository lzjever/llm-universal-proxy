use serde_json::Value;

use crate::formats::UpstreamFormat;

pub(crate) fn translation_target_label(format: UpstreamFormat) -> &'static str {
    match format {
        UpstreamFormat::OpenAiCompletion => "OpenAI Chat Completions",
        UpstreamFormat::OpenAiResponses => "OpenAI Responses",
        UpstreamFormat::Anthropic => "Anthropic",
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

pub(crate) fn custom_tools_not_portable_message(upstream_format: UpstreamFormat) -> String {
    format!(
        "OpenAI custom tools cannot be faithfully translated to {}; refusing to downgrade them to function tools",
        translation_target_label(upstream_format)
    )
}

pub(crate) fn custom_tool_format_downgraded_message(
    source_label: &str,
    tool_name: &str,
    target_label: &str,
) -> String {
    format!(
        "{source_label} custom tool `{tool_name}` format constraints cannot be faithfully translated to {target_label}; downgrading to raw string input semantics"
    )
}

pub(crate) fn reserved_openai_custom_bridge_prefix_message(name: &str) -> String {
    format!(
        "OpenAI Responses function name `{name}` uses reserved bridge prefix `__llmup_custom__`; this namespace is reserved for synthetic custom-tool bridging to OpenAI Chat Completions"
    )
}

pub(crate) fn responses_reasoning_continuity_not_portable_message(
    field: &str,
    target_label: &str,
) -> String {
    format!(
        "OpenAI Responses reasoning-continuity field `{field}` carries provider-owned opaque state and cannot be faithfully translated to {target_label}; use a native OpenAI Responses upstream to preserve it"
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
