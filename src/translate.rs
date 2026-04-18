//! Request/response translation between formats (pivot: OpenAI Chat Completions).
//!
//! Reference: 9router open-sse/translator/index.js — source → openai → target.

use serde_json::Value;

use crate::formats::UpstreamFormat;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TranslationIssueLevel {
    Warning,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TranslationIssue {
    pub level: TranslationIssueLevel,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TranslationAssessment {
    pub issues: Vec<TranslationIssue>,
}

impl TranslationAssessment {
    fn warning(&mut self, message: impl Into<String>) {
        self.issues.push(TranslationIssue {
            level: TranslationIssueLevel::Warning,
            message: message.into(),
        });
    }

    fn reject(&mut self, message: impl Into<String>) {
        self.issues.push(TranslationIssue {
            level: TranslationIssueLevel::Reject,
            message: message.into(),
        });
    }

    pub(crate) fn decision(&self) -> TranslationDecision {
        let mut warnings = Vec::new();
        for issue in &self.issues {
            match issue.level {
                TranslationIssueLevel::Reject => {
                    return TranslationDecision::Reject(issue.message.clone());
                }
                TranslationIssueLevel::Warning => warnings.push(issue.message.clone()),
            }
        }
        if warnings.is_empty() {
            TranslationDecision::Allow
        } else {
            TranslationDecision::AllowWithWarnings(warnings)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TranslationDecision {
    Allow,
    AllowWithWarnings(Vec<String>),
    Reject(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SemanticToolKind {
    Function,
    OpenAiCustom,
    AnthropicServerTool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SemanticToolResultContent {
    Text(String),
    Json(Value),
    TypedBlocks(Vec<Value>),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SemanticTextPart {
    pub text: String,
    pub annotations: Vec<Value>,
}

pub(crate) const OPENAI_REASONING_TO_ANTHROPIC_REJECT_MESSAGE: &str =
    "OpenAI reasoning cannot be replayed to Anthropic without provenance; refusing to translate reasoning as plain text";

fn translation_target_label(format: UpstreamFormat) -> &'static str {
    match format {
        UpstreamFormat::OpenAiCompletion => "OpenAI Chat Completions",
        UpstreamFormat::OpenAiResponses => "OpenAI Responses",
        UpstreamFormat::Anthropic => "Anthropic",
        UpstreamFormat::Google => "Gemini",
    }
}

fn single_candidate_choice_contract_message(
    source_label: &str,
    target_label: &str,
    field_name: &str,
    count: usize,
) -> String {
    format!(
        "{source_label} field `{field_name}` has {count} items; cross-protocol translation to {target_label} only supports a single candidate/choice"
    )
}

fn single_required_array_item<'a>(
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

fn single_optional_array_item<'a>(
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

fn anthropic_thinking_provenance_not_portable_message() -> String {
    "Anthropic thinking provenance (`signature` or omitted thinking) cannot be faithfully translated to non-Anthropic downstreams".to_string()
}

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
        UpstreamFormat::OpenAiCompletion => Ok(normalize_openai_completion_response(body)),
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

pub(crate) fn anthropic_tool_use_type_for_openai_tool_call(
    tool_call: &Value,
) -> Result<&'static str, String> {
    match semantic_tool_kind_from_value(tool_call) {
        SemanticToolKind::OpenAiCustom => {
            Err(custom_tools_not_portable_message(UpstreamFormat::Anthropic))
        }
        SemanticToolKind::AnthropicServerTool => Ok("server_tool_use"),
        SemanticToolKind::Function => Ok("tool_use"),
    }
}

pub(crate) fn semantic_tool_kind_from_value(value: &Value) -> SemanticToolKind {
    match value.get("proxied_tool_kind").and_then(Value::as_str) {
        Some("anthropic_server_tool_use") => SemanticToolKind::AnthropicServerTool,
        _ => match value.get("type").and_then(Value::as_str) {
            Some("custom") | Some("custom_tool_call") => SemanticToolKind::OpenAiCustom,
            _ => SemanticToolKind::Function,
        },
    }
}

fn semantic_tool_output_item_type(kind: SemanticToolKind) -> &'static str {
    match kind {
        SemanticToolKind::OpenAiCustom => "custom_tool_call_output",
        SemanticToolKind::AnthropicServerTool | SemanticToolKind::Function => {
            "function_call_output"
        }
    }
}

fn responses_item_is_tool_output(item: &Value) -> bool {
    matches!(
        item.get("type").and_then(Value::as_str),
        Some("function_call_output") | Some("custom_tool_call_output")
    )
}

fn content_value_is_effectively_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(text) => text.is_empty(),
        Value::Array(items) => items.is_empty(),
        _ => false,
    }
}

fn semantic_tool_result_content_from_value(content: Option<&Value>) -> SemanticToolResultContent {
    match content {
        Some(Value::String(text)) => SemanticToolResultContent::Text(text.clone()),
        Some(Value::Array(items))
            if items.iter().all(|item| {
                item.get("type")
                    .and_then(Value::as_str)
                    .map(|value| !value.is_empty())
                    .unwrap_or(false)
            }) =>
        {
            SemanticToolResultContent::TypedBlocks(items.clone())
        }
        Some(other) => SemanticToolResultContent::Json(other.clone()),
        None => SemanticToolResultContent::Text(String::new()),
    }
}

fn semantic_tool_result_content_to_value(content: &SemanticToolResultContent) -> Value {
    match content {
        SemanticToolResultContent::Text(text) => Value::String(text.clone()),
        SemanticToolResultContent::Json(value) => value.clone(),
        SemanticToolResultContent::TypedBlocks(items) => Value::Array(items.clone()),
    }
}

fn semantic_text_part_from_claude_block(block: &Value) -> Option<SemanticTextPart> {
    let text = block.get("text").and_then(Value::as_str)?.to_string();
    let annotations = block
        .get("citations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Some(SemanticTextPart { text, annotations })
}

fn semantic_text_part_from_openai_part(part: &Value) -> Option<SemanticTextPart> {
    if part.get("type").and_then(Value::as_str) != Some("text") {
        return None;
    }
    Some(SemanticTextPart {
        text: part.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
        annotations: part
            .get("annotations")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    })
}

fn semantic_text_part_to_openai_value(part: &SemanticTextPart) -> Value {
    let mut value = serde_json::json!({
        "type": "text",
        "text": part.text,
    });
    if !part.annotations.is_empty() {
        value["annotations"] = Value::Array(part.annotations.clone());
    }
    value
}

fn semantic_text_part_to_responses_value(part: &SemanticTextPart, content_type: &str) -> Value {
    let mut value = serde_json::json!({
        "type": content_type,
        "text": part.text,
    });
    if !part.annotations.is_empty() {
        value["annotations"] = Value::Array(part.annotations.clone());
    }
    value
}

fn openai_tool_arguments_raw(tool_call: &Value) -> Option<&str> {
    tool_call
        .get("function")
        .and_then(|function| function.get("arguments"))
        .or_else(|| tool_call.get("arguments"))
        .and_then(Value::as_str)
}

fn openai_tool_arguments_to_structured_value(
    tool_call: &Value,
    target_label: &str,
) -> Result<Value, String> {
    let Some(raw) = openai_tool_arguments_raw(tool_call) else {
        return Ok(serde_json::json!({}));
    };
    if raw.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    raw_json_object_to_structured_value(raw, target_label)
}

fn raw_json_object_to_structured_value(raw: &str, target_label: &str) -> Result<Value, String> {
    let value: Value = serde_json::from_str(raw).map_err(|error| {
        format!(
            "tool arguments for {target_label} must be valid JSON; received `{raw}`: {error}"
        )
    })?;
    if !value.is_object() {
        return Err(format!(
            "tool arguments for {target_label} must be a JSON object; received `{raw}`"
        ));
    }
    Ok(value)
}

fn responses_tool_call_input_raw(item: &Value) -> Option<&str> {
    match semantic_tool_kind_from_value(item) {
        SemanticToolKind::OpenAiCustom => item.get("input").and_then(Value::as_str),
        _ => item.get("arguments").and_then(Value::as_str),
    }
}

fn responses_tool_call_to_structured_value(
    item: &Value,
    target_label: &str,
) -> Result<Value, String> {
    let Some(raw) = responses_tool_call_input_raw(item) else {
        return Ok(serde_json::json!({}));
    };
    if raw.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    raw_json_object_to_structured_value(raw, target_label)
}

fn responses_tool_call_item_to_openai_tool_call(item: &Value) -> Option<Value> {
    match item.get("type").and_then(Value::as_str) {
        Some("function_call") | Some("custom_tool_call") => {}
        _ => return None,
    }

    let kind = semantic_tool_kind_from_value(item);
    let default_input = match kind {
        SemanticToolKind::OpenAiCustom => "",
        _ => "{}",
    };
    let mut tool_call = serde_json::json!({
        "id": item.get("call_id"),
        "type": match kind {
            SemanticToolKind::OpenAiCustom => "custom",
            _ => "function",
        },
        "function": {
            "name": item.get("name"),
            "arguments": responses_tool_call_input_raw(item).unwrap_or(default_input)
        }
    });
    if let Some(proxied_tool_kind) = item.get("proxied_tool_kind").cloned() {
        tool_call["proxied_tool_kind"] = proxied_tool_kind;
    }
    Some(tool_call)
}

fn openai_tool_call_to_responses_item(tool_call: &Value) -> Value {
    let kind = semantic_tool_kind_from_value(tool_call);
    let default_input = match kind {
        SemanticToolKind::OpenAiCustom => "",
        _ => "{}",
    };
    let mut item = match kind {
        SemanticToolKind::OpenAiCustom => serde_json::json!({
            "type": "custom_tool_call",
            "call_id": tool_call.get("id"),
            "name": tool_call.get("function").and_then(|f| f.get("name")),
            "input": openai_tool_arguments_raw(tool_call).unwrap_or(default_input)
        }),
        _ => serde_json::json!({
            "type": "function_call",
            "call_id": tool_call.get("id"),
            "name": tool_call.get("function").and_then(|f| f.get("name")),
            "arguments": openai_tool_arguments_raw(tool_call).unwrap_or(default_input)
        }),
    };
    if let Some(proxied_tool_kind) = tool_call.get("proxied_tool_kind").cloned() {
        item["proxied_tool_kind"] = proxied_tool_kind;
    }
    item
}

fn anthropic_block_has_cache_control(block: &Value) -> bool {
    block.get("cache_control").is_some()
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
        Some(anthropic_block_not_portable_message(block_type, target_label))
    }
}

fn anthropic_nonportable_content_block_message(
    body: &Value,
    target_label: &str,
) -> Option<String> {
    if let Some(system) = body.get("system") {
        match system {
            Value::Array(blocks) => {
                for block in blocks {
                    if let Some(message) = anthropic_nonportable_block_message(block, target_label) {
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

fn extract_openai_refusal(message: &Value) -> Option<String> {
    message
        .get("refusal")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|text| !text.is_empty())
}

fn extract_openai_content_text(content: Option<&Value>) -> String {
    let Some(content) = content else {
        return String::new();
    };
    match content {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
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

fn collapse_openai_text_parts(parts: &[Value]) -> Value {
    let all_plain_text = parts.iter().all(|part| {
        part.get("type").and_then(Value::as_str) == Some("text")
            && part
                .get("annotations")
                .and_then(Value::as_array)
                .map(|annotations| annotations.is_empty())
                .unwrap_or(true)
    });
    if all_plain_text {
        return Value::String(
            parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<String>(),
        );
    }
    Value::Array(parts.to_vec())
}

fn copy_remaining_usage_fields(
    source: &Value,
    target: &mut Value,
    consumed_fields: &[&str],
) {
    let Some(source_map) = source.as_object() else {
        return;
    };
    let Some(target_map) = target.as_object_mut() else {
        return;
    };
    for (key, value) in source_map {
        if consumed_fields.iter().any(|consumed| consumed == key) {
            continue;
        }
        target_map.entry(key.clone()).or_insert_with(|| value.clone());
    }
}

fn responses_stateful_request_controls_for_translate(body: &Value) -> Vec<&'static str> {
    let mut controls = Vec::new();
    for field in ["previous_response_id", "conversation", "background", "store", "prompt"] {
        if body.get(field).is_some() {
            controls.push(field);
        }
    }
    controls
}

fn cross_protocol_requested_choice_count(
    client_format: UpstreamFormat,
    body: &Value,
) -> Option<(&'static str, u64)> {
    match client_format {
        UpstreamFormat::OpenAiCompletion => body.get("n").and_then(Value::as_u64).map(|n| ("n", n)),
        UpstreamFormat::Google => {
            let generation_config = body.get("generationConfig").or_else(|| body.get("generation_config"));
            generation_config
                .and_then(|config| {
                    config
                        .get("candidateCount")
                        .or_else(|| config.get("candidate_count"))
                })
                .or_else(|| body.get("candidateCount").or_else(|| body.get("candidate_count")))
                .and_then(Value::as_u64)
                .map(|count| ("candidateCount", count))
        }
        _ => None,
    }
}

fn cross_protocol_requested_choice_count_message(
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

fn request_contains_openai_reasoning_without_provenance(
    client_format: UpstreamFormat,
    body: &Value,
) -> bool {
    match client_format {
        UpstreamFormat::OpenAiCompletion => body
            .get("messages")
            .and_then(Value::as_array)
            .map(|messages| {
                messages.iter().any(|message| {
                    message.get("role").and_then(Value::as_str) == Some("assistant")
                        && openai_message_reasoning_text(message).is_some()
                })
            })
            .unwrap_or(false),
        UpstreamFormat::OpenAiResponses => body
            .get("input")
            .and_then(Value::as_array)
            .map(|items| {
                items.iter().any(|item| {
                    item.get("type").and_then(Value::as_str) == Some("reasoning")
                        || (item.get("type").and_then(Value::as_str) == Some("message")
                            && item.get("role").and_then(Value::as_str) == Some("assistant")
                            && extract_responses_text_content(item.get("content")).is_empty()
                            && item
                                .get("content")
                                .and_then(Value::as_array)
                                .map(|content| {
                                    content.iter().any(|part| {
                                        part.get("type").and_then(Value::as_str)
                                            == Some("summary_text")
                                    })
                                })
                                .unwrap_or(false))
                })
            })
            .unwrap_or(false),
        _ => false,
    }
}

fn request_has_custom_tools(client_format: UpstreamFormat, body: &Value) -> bool {
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
        }
        _ => false,
    }
}

fn request_invalid_structured_tool_arguments_message(
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
                    message.get("tool_calls").and_then(Value::as_array).and_then(|tool_calls| {
                        tool_calls.iter().find_map(|tool_call| {
                            (semantic_tool_kind_from_value(tool_call)
                                != SemanticToolKind::OpenAiCustom)
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
        UpstreamFormat::OpenAiResponses => body
            .get("input")
            .and_then(Value::as_array)
            .and_then(|items| {
                items.iter().find_map(|item| {
                    matches!(
                        item.get("type").and_then(Value::as_str),
                        Some("function_call") | Some("custom_tool_call")
                    )
                    .then(|| {
                        (semantic_tool_kind_from_value(item) != SemanticToolKind::OpenAiCustom)
                            .then(|| {
                                responses_tool_call_to_structured_value(item, target_label).err()
                            })
                            .flatten()
                    })
                    .flatten()
                })
            }),
        _ => None,
    }
}

fn anthropic_cross_protocol_control_names(body: &Value) -> Vec<&'static str> {
    let mut controls = Vec::new();
    for field in [
        "thinking",
        "top_k",
        "service_tier",
        "container",
        "context_management",
        "tool_choice",
    ] {
        if body.get(field).is_some() {
            controls.push(field);
        }
    }
    if anthropic_protocol_uses_cache_control(body) {
        controls.push("cache_control");
    }
    controls
}

pub(crate) fn assess_request_translation(
    client_format: UpstreamFormat,
    upstream_format: UpstreamFormat,
    body: &Value,
) -> TranslationAssessment {
    let mut assessment = TranslationAssessment::default();

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
        if let Some(tools) = body.get("tools").and_then(Value::as_array) {
            if tools.iter().any(|tool| {
                tool.get("name").is_none()
                    && semantic_tool_kind_from_value(tool) == SemanticToolKind::Function
            }) {
                assessment.warning(format!(
                    "non-function Responses tools are not portable to {upstream_format} and will be dropped"
                ));
            }
        }
    }

    if client_format == UpstreamFormat::Anthropic && upstream_format != UpstreamFormat::Anthropic {
        let controls = anthropic_cross_protocol_control_names(body);
        if !controls.is_empty() {
            let quoted = controls
                .iter()
                .map(|field| format!("`{field}`"))
                .collect::<Vec<_>>()
                .join(", ");
            assessment.reject(format!(
                "Anthropic request controls {quoted} have native provider semantics and cannot be faithfully translated to {upstream_format}"
            ));
        }
        if let Some(message) =
            anthropic_nonportable_content_block_message(body, translation_target_label(upstream_format))
        {
            assessment.reject(message);
        }
    }

    if upstream_format == UpstreamFormat::Anthropic
        && request_contains_openai_reasoning_without_provenance(client_format, body)
    {
        assessment.reject(OPENAI_REASONING_TO_ANTHROPIC_REJECT_MESSAGE);
    }

    if matches!(upstream_format, UpstreamFormat::Anthropic | UpstreamFormat::Google)
        && request_has_custom_tools(client_format, body)
    {
        assessment.reject(custom_tools_not_portable_message(upstream_format));
    }

    if matches!(upstream_format, UpstreamFormat::Anthropic | UpstreamFormat::Google) {
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

fn claude_response_to_openai(body: &Value) -> Result<Value, String> {
    if anthropic_response_has_nonportable_thinking_provenance(body) {
        return Err(anthropic_thinking_provenance_not_portable_message());
    }
    let content = body.get("content").cloned().ok_or("missing content")?;
    let mut converted = convert_claude_message_to_openai(&serde_json::json!({
        "role": "assistant",
        "content": content
    }))
    ?
    .ok_or("missing content")?;
    let mut message = converted
        .drain(..)
        .find(|item| item.get("role").and_then(Value::as_str) == Some("assistant"))
        .ok_or("missing assistant message")?;
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

fn anthropic_response_has_nonportable_thinking_provenance(body: &Value) -> bool {
    let Some(content) = body.get("content").and_then(Value::as_array) else {
        return false;
    };

    content.iter().any(|block| {
        if block.get("type").and_then(Value::as_str) != Some("thinking") {
            return false;
        }
        block.get("signature").is_some()
            || block
                .get("thinking")
                .map(|thinking| !thinking.is_string())
                .unwrap_or(false)
    })
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

fn openai_finish_reason_to_gemini(finish_reason: &str) -> &'static str {
    match finish_reason {
        "stop" | "tool_calls" => "STOP",
        "length" => "MAX_TOKENS",
        "content_filter" => "SAFETY",
        "pause_turn" | "context_length_exceeded" | "tool_error" | "error" => "OTHER",
        _ => "STOP",
    }
}

const GEMINI_DUMMY_THOUGHT_SIGNATURE: &str = "skip_thought_signature_validator";

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

fn responses_finish_reason_to_openai(body: &Value, has_tool_calls: bool) -> String {
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

fn push_gemini_function_call_part(
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

fn gemini_response_to_openai(body: &Value) -> Result<Value, String> {
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
        if thoughts_tokens > 0 {
            usage_json["completion_tokens_details"] = serde_json::json!({
                "reasoning_tokens": thoughts_tokens
            });
        }

        result["usage"] = usage_json;
    }
    Ok(result)
}

fn gemini_candidate_to_openai_assistant_message(content: Option<&Value>) -> Result<Value, String> {
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

fn gemini_assistant_message_has_non_text_content_parts(message: &Value) -> bool {
    message
        .get("content")
        .and_then(Value::as_array)
        .map(|parts| {
            parts.iter().any(|part| {
                part.get("type").and_then(Value::as_str) != Some("text")
            })
        })
        .unwrap_or(false)
}

fn is_minimax_model(model: &str) -> bool {
    model.starts_with("MiniMax-")
}

fn reasoning_details_to_text(value: Option<&Value>) -> Option<String> {
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

fn openai_message_reasoning_text(message: &Value) -> Option<String> {
    if let Some(text) = message.get("reasoning_content").and_then(Value::as_str) {
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }
    reasoning_details_to_text(message.get("reasoning_details"))
}

fn normalize_openai_completion_response(body: &Value) -> Value {
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

fn openai_response_to_claude(body: &Value) -> Result<Value, String> {
    let choice = single_required_array_item(
        body.get("choices").and_then(Value::as_array).map(Vec::as_slice),
        "OpenAI response",
        "Anthropic",
        "choices",
    )?;
    let message = choice.get("message").ok_or("missing message")?;
    if let Some(rc) = openai_message_reasoning_text(message) {
        if !rc.is_empty() {
            return Ok(serde_json::json!({
                "type": "error",
                "error": {
                    "type": "invalid_request_error",
                    "message": format!("{OPENAI_REASONING_TO_ANTHROPIC_REJECT_MESSAGE}.")
                }
            }));
        }
    }
    let content = openai_message_to_claude_blocks(message)?
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

fn openai_response_to_gemini(body: &Value) -> Result<Value, String> {
    let choice = single_required_array_item(
        body.get("choices").and_then(Value::as_array).map(Vec::as_slice),
        "OpenAI response",
        "Gemini",
        "choices",
    )?;
    let message = choice.get("message").ok_or("missing message")?;
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
    let mut content_parts: Vec<Value> = vec![];
    let mut reasoning_content = String::new();
    let mut refusal = String::new();
    let mut tool_calls: Vec<Value> = vec![];
    for item in output {
        let ty = item.get("type").and_then(Value::as_str);
        if ty == Some("message") {
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
        if let Some(tool_call) = responses_tool_call_item_to_openai_tool_call(item) {
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
    if let Some(u) = body.get("usage") {
        result["usage"] = responses_usage_to_openai_usage(u);
    }
    Ok(result)
}

fn openai_response_to_responses(body: &Value) -> Result<Value, String> {
    let choice = single_required_array_item(
        body.get("choices").and_then(Value::as_array).map(Vec::as_slice),
        "OpenAI response",
        "OpenAI Responses",
        "choices",
    )?;
    let message = choice.get("message").ok_or("missing message")?;
    let mut output: Vec<Value> = vec![];
    if let Some(reasoning) = openai_message_reasoning_text(message) {
        if !reasoning.is_empty() {
            output.push(serde_json::json!({
                "type": "reasoning",
                "summary": [{ "type": "summary_text", "text": reasoning }]
            }));
        }
    }
    let content = openai_message_to_responses_content(message, "output_text");
    if !content.is_empty() {
        output.push(serde_json::json!({
            "type": "message",
            "role": "assistant",
            "content": content
        }));
    } else if message.get("tool_calls").is_none() && output.is_empty() {
        output.push(serde_json::json!({
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": "" }]
        }));
    }
    if let Some(tc) = message.get("tool_calls").and_then(Value::as_array) {
        for t in tc {
            output.push(openai_tool_call_to_responses_item(t));
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

/// Translate request body from client format to upstream format.
/// If client_format == upstream_format, returns body as-is (passthrough).
pub fn translate_request(
    client_format: UpstreamFormat,
    upstream_format: UpstreamFormat,
    model: &str,
    body: &mut Value,
    stream: bool,
) -> Result<(), String> {
    if let TranslationDecision::Reject(message) =
        assess_request_translation(client_format, upstream_format, body).decision()
    {
        return Err(message);
    }

    if client_format == upstream_format {
        if stream && (client_format != UpstreamFormat::OpenAiCompletion || !is_minimax_model(model))
        {
            normalize_openai_roles_for_compatibility(client_format, body);
        }
        if client_format == UpstreamFormat::OpenAiCompletion {
            apply_openai_completion_compat_overrides(model, body);
        }
        return Ok(());
    }
    let translated_from_openai_completion = client_format == UpstreamFormat::OpenAiCompletion;
    // Step 1: client → openai (if client is not openai)
    if client_format != UpstreamFormat::OpenAiCompletion {
        client_to_openai_completion(client_format, body)?;
    }
    // Step 2: openai → upstream (if upstream is not openai)
    if upstream_format != UpstreamFormat::OpenAiCompletion {
        openai_completion_to_upstream(upstream_format, body)?;
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
        if stream
            && (upstream_format != UpstreamFormat::OpenAiCompletion || !is_minimax_model(model))
        {
            normalize_openai_roles_for_compatibility(upstream_format, body);
        }
        apply_openai_completion_compat_overrides(model, body);
    }
    Ok(())
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
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .map(str::to_string);
        let content = message
            .get("content")
            .and_then(Value::as_str)
            .map(str::to_string);
        let Some(role) = role else {
            coalesced.push(message);
            continue;
        };
        let Some(content) = content else {
            coalesced.push(message);
            continue;
        };

        if let Some(previous) = coalesced.last_mut() {
            let previous_role = previous.get("role").and_then(Value::as_str);
            let previous_content = previous.get("content").and_then(Value::as_str);
            if previous_role == Some(role.as_str()) {
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

fn extract_responses_text_content(content: Option<&Value>) -> String {
    let Some(content) = content else {
        return String::new();
    };
    let Some(parts) = content.as_array() else {
        return String::new();
    };

    parts
        .iter()
        .filter_map(|part| match part.get("type").and_then(Value::as_str) {
            Some("input_text") | Some("output_text") => part.get("text").and_then(Value::as_str),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
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
                    && items.get(idx + 1).is_some_and(responses_item_is_tool_output)
                {
                    deferred_user_after_tool_results =
                        Some(serde_json::json!({ "role": "user", "content": content }));
                } else {
                    flush_assistant(&mut messages, &mut current_assistant);
                    messages.push(serde_json::json!({ "role": role, "content": content }));
                }
            }
            "function_call" | "custom_tool_call" => {
                let Some(tc) = responses_tool_call_item_to_openai_tool_call(&item) else {
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
                messages.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": call_id,
                    "content": item
                        .get("output")
                        .cloned()
                        .unwrap_or_else(|| Value::String(String::new()))
                }));
                let next_is_function_output =
                    items.get(idx + 1).is_some_and(responses_item_is_tool_output);
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
    let stop = body.get("stop").cloned();
    let tools = body.get("tools").cloned();
    let parallel_tool_calls = body.get("parallel_tool_calls").cloned();
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
    if let Some(stop) = stop {
        out.insert("stop".to_string(), stop);
    }
    if let Some(tool_choice) = body.get("tool_choice").cloned() {
        if let Some(mapped_tool_choice) = responses_tool_choice_to_openai_tool_choice(&tool_choice)
        {
            out.insert("tool_choice".to_string(), mapped_tool_choice);
        }
    }
    if let Some(parallel_tool_calls) = parallel_tool_calls {
        out.insert("parallel_tool_calls".to_string(), parallel_tool_calls);
    }

    // Convert tools from Responses API format to Chat Completions format
    // Responses: { "name": "...", "description": "...", "parameters": {...} }
    // Chat: { "type": "function", "function": { "name": "...", "description": "...", "parameters": {...} } }
    // Note: Responses API may include non-function tools like web_search which don't have names.
    // We only convert tools that have a "name" field (function tools).
    if let Some(tools) = tools.as_ref().and_then(Value::as_array) {
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
                let text = c.get("text").and_then(Value::as_str).unwrap_or("").to_string();
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

fn messages_to_responses(body: &mut Value) -> Result<(), String> {
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
                .unwrap_or(SemanticToolKind::Function);
            input.push(serde_json::json!({
                "type": semantic_tool_output_item_type(tool_kind),
                "call_id": msg.get("tool_call_id"),
                "output": msg.get("content")
            }));
        }
    }
    body["input"] = Value::Array(input);
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
    if let Some(max_output_tokens) = body
        .get("max_completion_tokens")
        .cloned()
        .or_else(|| body.get("max_tokens").cloned())
    {
        body["max_output_tokens"] = max_output_tokens;
    }
    if let Some(obj) = body.as_object_mut() {
        obj.remove("instructions");
        obj.remove("messages");
        obj.remove("max_completion_tokens");
        obj.remove("max_tokens");
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
            if let Some(openai_msg) = convert_claude_message_to_openai(msg)? {
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

fn convert_claude_message_to_openai(msg: &Value) -> Result<Option<Vec<Value>>, String> {
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
                    "arguments": block
                        .get("input")
                        .and_then(|input| serde_json::to_string(input).ok())
                        .unwrap_or_else(|| "{}".to_string())
                }
            })),
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
                let semantic_content = semantic_tool_result_content_from_value(block.get("content"));
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
    if let Some(tool_choice) = body.get("tool_choice") {
        if let Some(mapped_tool_choice) =
            openai_tool_choice_to_claude_tool_choice(tool_choice, body.get("parallel_tool_calls"))
        {
            result["tool_choice"] = mapped_tool_choice;
        }
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

fn collapse_gemini_parts_for_openai(parts: &[Value]) -> Value {
    if parts.len() == 1 && parts[0].get("type").and_then(Value::as_str) == Some("text") {
        return parts[0]
            .get("text")
            .cloned()
            .unwrap_or(Value::String(String::new()));
    }
    Value::Array(parts.to_vec())
}

fn openai_message_to_claude_blocks(msg: &Value) -> Result<Option<Vec<Value>>, String> {
    let Some(role) = msg.get("role").and_then(Value::as_str) else {
        return Ok(None);
    };
    if role == "tool" {
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
        if openai_message_reasoning_text(msg).is_some() {
            return Err(OPENAI_REASONING_TO_ANTHROPIC_REJECT_MESSAGE.to_string());
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
        return Ok(Some(vec![serde_json::json!({ "type": "text", "text": "" })]));
    }
    if blocks.is_empty() {
        return Ok(None);
    }
    Ok(Some(blocks))
}

#[cfg(test)]
mod translate_regression_tests {
    use super::{
        can_attach_cache_control_to_content_block, convert_claude_message_to_openai,
        openai_message_to_claude_blocks, openai_to_claude,
        OPENAI_REASONING_TO_ANTHROPIC_REJECT_MESSAGE,
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

        let blocks = openai_message_to_claude_blocks(&msg)
            .expect("translate blocks")
            .expect("assistant blocks");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[1]["type"], "tool_use");
        assert_eq!(blocks[1]["id"], "call_123");
        assert_eq!(blocks[1]["name"], "exec_command");
    }

    #[test]
    fn assistant_reasoning_content_without_provenance_is_rejected_for_claude_blocks() {
        let msg = json!({
            "role": "assistant",
            "reasoning_content": "I should call a tool.",
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

        let err = openai_message_to_claude_blocks(&msg)
            .expect_err("reasoning without provenance should fail closed");
        assert_eq!(err, OPENAI_REASONING_TO_ANTHROPIC_REJECT_MESSAGE);
    }

    #[test]
    fn claude_server_tool_use_is_preserved_as_marked_openai_tool_call() {
        let message = json!({
            "role": "assistant",
            "content": [{
                "type": "server_tool_use",
                "id": "toolu_server_1",
                "name": "web_search",
                "input": { "query": "rust" }
            }]
        });

        let translated = convert_claude_message_to_openai(&message)
            .expect("translated message")
            .expect("openai messages");
        assert_eq!(translated.len(), 1);
        let tool_calls = translated[0]["tool_calls"].as_array().expect("tool calls");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0]["function"]["name"], "web_search");
        assert_eq!(
            tool_calls[0]["proxied_tool_kind"],
            "anthropic_server_tool_use"
        );
    }

    #[test]
    fn marked_openai_server_tool_call_restores_server_tool_use_block() {
        let message = json!({
            "role": "assistant",
            "tool_calls": [{
                "id": "toolu_server_1",
                "type": "function",
                "proxied_tool_kind": "anthropic_server_tool_use",
                "function": {
                    "name": "web_search",
                    "arguments": "{\"query\":\"rust\"}"
                }
            }]
        });

        let blocks = openai_message_to_claude_blocks(&message)
            .expect("translate blocks")
            .expect("assistant blocks");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["type"], "server_tool_use");
        assert_eq!(blocks[0]["name"], "web_search");
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
        let assistant_content = messages[0]["content"]
            .as_array()
            .expect("assistant content");
        assert_eq!(assistant_content[1]["type"], "tool_use");
        assert!(assistant_content[1].get("cache_control").is_none());
        assert!(can_attach_cache_control_to_content_block(
            &assistant_content[0]
        ));
        assert!(!can_attach_cache_control_to_content_block(
            &assistant_content[1]
        ));
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
            for msg in convert_gemini_content_to_openai(content)? {
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
    if let Some(config) = body
        .get("toolConfig")
        .and_then(|tool_config| tool_config.get("functionCallingConfig"))
    {
        let mode = config.get("mode").and_then(Value::as_str);
        let allowed = config
            .get("allowedFunctionNames")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        match mode {
            Some("NONE") => result["tool_choice"] = serde_json::json!("none"),
            Some("AUTO") => result["tool_choice"] = serde_json::json!("auto"),
            Some("ANY") => {
                if allowed.len() == 1 {
                    result["tool_choice"] = serde_json::json!({
                        "type": "function",
                        "function": { "name": allowed[0].clone() }
                    });
                } else {
                    result["tool_choice"] = serde_json::json!("required");
                    if !allowed.is_empty() {
                        result["allowed_tool_names"] = Value::Array(allowed);
                    }
                }
            }
            _ => {}
        }
    }
    *body = result;
    Ok(())
}

fn gemini_part_field<'a>(part: &'a Value, camel: &str, snake: &str) -> Option<&'a Value> {
    part.get(camel).or_else(|| part.get(snake))
}

fn gemini_nested_field<'a>(value: &'a Value, camel: &str, snake: &str) -> Option<&'a Value> {
    value.get(camel).or_else(|| value.get(snake))
}

fn gemini_part_kind_label(part: &Value) -> String {
    if part.get("text").is_some() {
        return "text".to_string();
    }
    for (camel, snake, label) in [
        ("inlineData", "inline_data", "inlineData"),
        ("fileData", "file_data", "fileData"),
        ("functionCall", "function_call", "functionCall"),
        ("functionResponse", "function_response", "functionResponse"),
    ] {
        if part.get(camel).is_some() || part.get(snake).is_some() {
            return label.to_string();
        }
    }
    if part.get("thought").is_some() {
        return "thought".to_string();
    }
    part.as_object()
        .and_then(|obj| obj.keys().next().cloned())
        .unwrap_or_else(|| "unknown".to_string())
}

fn normalize_audio_format_from_mime(mime: &str) -> String {
    let subtype = mime
        .split(';')
        .next()
        .unwrap_or(mime)
        .split('/')
        .nth(1)
        .unwrap_or("wav")
        .trim()
        .to_ascii_lowercase();
    match subtype.as_str() {
        "x-wav" => "wav".to_string(),
        other => other.to_string(),
    }
}

fn openai_audio_mime_type(format: &str) -> String {
    let normalized = format.trim().to_ascii_lowercase();
    if normalized.contains('/') {
        return normalized;
    }
    match normalized.as_str() {
        "" => "audio/wav".to_string(),
        "x-wav" => "audio/wav".to_string(),
        other => format!("audio/{other}"),
    }
}

fn base64_data_uri_parts(value: &str) -> Option<(&str, &str)> {
    value
        .strip_prefix("data:")
        .and_then(|rest| rest.split_once(";base64,"))
}

enum OpenAiFileDataReference<'a> {
    InlineData { mime_type: String, data: &'a str },
    FileUri(&'a str),
}

fn looks_like_uri_reference(value: &str) -> bool {
    let Some((scheme, _)) = value.split_once("://") else {
        return false;
    };
    !scheme.is_empty()
        && scheme
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))
}

fn looks_like_base64_payload(value: &str) -> bool {
    let compact = value.trim();
    !compact.is_empty()
        && compact.chars().all(|ch| {
            ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '=' | '\r' | '\n')
        })
}

fn mime_type_from_filename(filename: &str) -> Option<&'static str> {
    let extension = filename.rsplit('.').next()?.trim().to_ascii_lowercase();
    match extension.as_str() {
        "pdf" => Some("application/pdf"),
        "json" => Some("application/json"),
        "txt" => Some("text/plain"),
        "csv" => Some("text/csv"),
        "md" => Some("text/markdown"),
        "html" | "htm" => Some("text/html"),
        "xml" => Some("application/xml"),
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "wav" => Some("audio/wav"),
        "mp3" => Some("audio/mpeg"),
        _ => None,
    }
}

fn openai_file_data_reference<'a>(
    file_data: &'a str,
    filename: Option<&str>,
) -> Result<OpenAiFileDataReference<'a>, String> {
    if let Some((mime_type, data)) = base64_data_uri_parts(file_data) {
        return Ok(OpenAiFileDataReference::InlineData {
            mime_type: mime_type.to_string(),
            data,
        });
    }
    if looks_like_uri_reference(file_data) {
        return Ok(OpenAiFileDataReference::FileUri(file_data));
    }
    if looks_like_base64_payload(file_data) {
        let Some(mime_type) = filename.and_then(mime_type_from_filename) else {
            return Err(
                "OpenAI file_data payloads need MIME or filename provenance to translate to Gemini; use a MIME-bearing data URI or include a filename with a known extension."
                    .to_string(),
            );
        };
        return Ok(OpenAiFileDataReference::InlineData {
            mime_type: mime_type.to_string(),
            data: file_data,
        });
    }
    Err(
        "OpenAI file_data must be a MIME-bearing data URI, a recognized fileUri-style reference, or a base64 payload with filename provenance to translate to Gemini."
            .to_string(),
    )
}

fn openai_file_part_from_gemini_inline_data(mime: &str, data: &str) -> Value {
    serde_json::json!({
        "type": "file",
        "file": {
            "file_data": format!("data:{mime};base64,{data}")
        }
    })
}

fn gemini_file_data_to_openai_part(file_data: &Value) -> Value {
    let file_uri = file_data
        .get("fileUri")
        .or_else(|| file_data.get("file_uri"))
        .cloned()
        .unwrap_or_else(|| Value::String(String::new()));
    let mut file = serde_json::Map::new();
    file.insert("file_data".to_string(), file_uri);
    if let Some(filename) = file_data
        .get("displayName")
        .or_else(|| file_data.get("display_name"))
        .cloned()
    {
        file.insert("filename".to_string(), filename);
    }
    serde_json::json!({
        "type": "file",
        "file": Value::Object(file)
    })
}

fn convert_gemini_content_to_openai(content: &Value) -> Result<Vec<Value>, String> {
    let role = content
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("user");
    let openai_role = if role == "user" { "user" } else { "assistant" };
    let Some(parts) = content.get("parts").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut messages = Vec::new();
    let mut openai_parts: Vec<Value> = vec![];
    let mut tool_calls: Vec<Value> = vec![];
    let mut reasoning_content = String::new();
    for part in parts {
        let mut recognized = false;
        if part.get("thought").and_then(Value::as_bool) == Some(true) {
            if role != "user" {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    reasoning_content.push_str(text);
                }
            }
            continue;
        }
        if part.get("text").is_some() {
            recognized = true;
            openai_parts.push(serde_json::json!({ "type": "text", "text": part.get("text") }));
        }
        if let Some(inline) = gemini_part_field(part, "inlineData", "inline_data") {
            recognized = true;
            let mime = inline
                .get("mimeType")
                .or_else(|| inline.get("mime_type"))
                .and_then(Value::as_str)
                .unwrap_or("application/octet-stream");
            let data = inline.get("data").and_then(Value::as_str).unwrap_or("");
            if mime.starts_with("image/") {
                openai_parts.push(serde_json::json!({
                    "type": "image_url",
                    "image_url": { "url": format!("data:{};base64,{}", mime, data) }
                }));
            } else if mime.starts_with("audio/") {
                openai_parts.push(serde_json::json!({
                    "type": "input_audio",
                    "input_audio": {
                        "data": data,
                        "format": normalize_audio_format_from_mime(mime)
                    }
                }));
            } else {
                openai_parts.push(openai_file_part_from_gemini_inline_data(mime, data));
            }
        }
        if let Some(file_data) = gemini_part_field(part, "fileData", "file_data") {
            recognized = true;
            openai_parts.push(gemini_file_data_to_openai_part(file_data));
        }
        if let Some(fc) = gemini_part_field(part, "functionCall", "function_call") {
            recognized = true;
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
        if let Some(fr) = gemini_part_field(part, "functionResponse", "function_response") {
            if !openai_parts.is_empty() {
                messages.push(serde_json::json!({
                    "role": openai_role,
                    "content": collapse_gemini_parts_for_openai(&openai_parts)
                }));
                openai_parts.clear();
            }
            let call_id = fr.get("id").or(fr.get("name")).cloned();
            let resp = gemini_nested_field(fr, "response", "response");
            let content_str = resp
                .and_then(|r| r.get("result").cloned())
                .or_else(|| resp.cloned())
                .map(|v| serde_json::to_string(&v).unwrap_or_default())
                .unwrap_or_default();
            messages.push(serde_json::json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": content_str
            }));
            continue;
        }
        if !recognized {
            return Err(format!(
                "Gemini content part `{}` cannot be faithfully translated to OpenAI Chat Completions.",
                gemini_part_kind_label(part)
            ));
        }
    }
    if !tool_calls.is_empty() {
        let mut m = serde_json::json!({ "role": "assistant", "tool_calls": tool_calls });
        if !openai_parts.is_empty() {
            m["content"] = collapse_gemini_parts_for_openai(&openai_parts);
        }
        if !reasoning_content.is_empty() {
            m["reasoning_content"] = Value::String(reasoning_content);
        }
        messages.push(m);
        return Ok(messages);
    }
    if openai_parts.is_empty() {
        if !reasoning_content.is_empty() {
            messages.push(serde_json::json!({
                "role": openai_role,
                "content": "",
                "reasoning_content": reasoning_content
            }));
        }
        return Ok(messages);
    }
    let mut message = serde_json::json!({
        "role": openai_role,
        "content": collapse_gemini_parts_for_openai(&openai_parts)
    });
    if !reasoning_content.is_empty() {
        message["reasoning_content"] = Value::String(reasoning_content);
    }
    messages.push(message);
    Ok(messages)
}

fn uuid_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{t:x}")
}

fn openai_tool_choice_to_gemini_function_calling_config(
    choice: &Value,
    allowed_tool_names: Option<&Value>,
) -> Option<Value> {
    let allowed_names = allowed_tool_names
        .and_then(Value::as_array)
        .cloned()
        .filter(|names| !names.is_empty());

    if let Some(choice_str) = choice.as_str() {
        return match choice_str {
            "auto" => Some(serde_json::json!({ "mode": "AUTO" })),
            "none" => Some(serde_json::json!({ "mode": "NONE" })),
            "required" => {
                let mut config = serde_json::json!({ "mode": "ANY" });
                if let Some(allowed_names) = allowed_names {
                    config["allowedFunctionNames"] = Value::Array(allowed_names);
                }
                Some(config)
            }
            _ => None,
        };
    }

    let obj = choice.as_object()?;
    let ty = obj.get("type").and_then(Value::as_str)?;
    if ty != "function" {
        return None;
    }
    let name = obj.get("name").or_else(|| {
        obj.get("function")
            .and_then(|function| function.get("name"))
    })?;
    Some(serde_json::json!({
        "mode": "ANY",
        "allowedFunctionNames": [name]
    }))
}

fn flush_pending_gemini_function_responses(
    contents: &mut Vec<Value>,
    pending_tool_parts: &mut Vec<(usize, Value)>,
) {
    if pending_tool_parts.is_empty() {
        return;
    }

    pending_tool_parts.sort_by_key(|(sort_key, _)| *sort_key);
    let parts = pending_tool_parts
        .drain(..)
        .map(|(_, part)| part)
        .collect::<Vec<_>>();
    contents.push(serde_json::json!({ "role": "user", "parts": parts }));
}

fn openai_content_part_to_gemini_part(part: &Value) -> Result<Option<Value>, String> {
    let Some(part_type) = part.get("type").and_then(Value::as_str) else {
        return Ok(None);
    };

    match part_type {
        "text" => Ok(part
            .get("text")
            .cloned()
            .map(|text| serde_json::json!({ "text": text }))),
        "image_url" => {
            let url = part
                .get("image_url")
                .and_then(|image| image.get("url"))
                .and_then(Value::as_str)
                .or_else(|| part.get("image_url").and_then(Value::as_str))
                .unwrap_or("");
            let Some((mime, data)) = base64_data_uri_parts(url) else {
                return Err(
                    "OpenAI image_url parts require base64 data URIs to translate to Gemini; remote image URLs need explicit upload or fileUri provenance."
                        .to_string(),
                );
            };
            Ok(Some(serde_json::json!({
                "inlineData": { "mimeType": mime, "data": data }
            })))
        }
        "input_audio" => {
            let input_audio = part.get("input_audio").unwrap_or(&Value::Null);
            let data = input_audio
                .get("data")
                .and_then(Value::as_str)
                .unwrap_or("");
            if data.is_empty() {
                return Err(
                    "OpenAI input_audio parts require inline base64 audio data to translate to Gemini."
                        .to_string(),
                );
            }
            let format = input_audio
                .get("format")
                .and_then(Value::as_str)
                .unwrap_or("wav");
            Ok(Some(serde_json::json!({
                "inlineData": {
                    "mimeType": openai_audio_mime_type(format),
                    "data": data
                }
            })))
        }
        "file" => {
            let file = part
                .get("file")
                .and_then(Value::as_object)
                .ok_or("OpenAI file parts require a file object to translate to Gemini.")?;
            if let Some(file_data) = file.get("file_data").and_then(Value::as_str) {
                return match openai_file_data_reference(
                    file_data,
                    file.get("filename").and_then(Value::as_str),
                )? {
                    OpenAiFileDataReference::InlineData { mime_type, data } => {
                        Ok(Some(serde_json::json!({
                            "inlineData": { "mimeType": mime_type, "data": data }
                        })))
                    }
                    OpenAiFileDataReference::FileUri(file_uri) => {
                        let mut file_data_part = serde_json::json!({
                            "fileData": { "fileUri": file_uri }
                        });
                        if let Some(filename) = file.get("filename").cloned() {
                            file_data_part["fileData"]["displayName"] = filename;
                        }
                        Ok(Some(file_data_part))
                    }
                };
            }
            if let Some(file_id) = file.get("file_id").and_then(Value::as_str) {
                return Err(format!(
                    "OpenAI file references like `{file_id}` cannot be faithfully translated to Gemini without file_data or fileUri provenance."
                ));
            }
            Err("OpenAI file parts require file_data to translate to Gemini.".to_string())
        }
        other => Err(format!(
            "OpenAI content part `{other}` cannot be faithfully translated to Gemini."
        )),
    }
}

fn openai_to_gemini(body: &mut Value) -> Result<(), String> {
    let mut result = serde_json::json!({
        "model": body.get("model").cloned().unwrap_or(serde_json::Value::Null),
        "contents": [],
        "generationConfig": {},
        "safetySettings": []
    });
    if let Some(mt) = body
        .get("max_completion_tokens")
        .cloned()
        .or_else(|| body.get("max_tokens").cloned())
    {
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
    let mut tool_name_by_call_id = std::collections::HashMap::new();
    let mut tool_sort_key_by_call_id = std::collections::HashMap::new();
    let mut next_tool_sort_key = 0usize;
    let mut contents: Vec<Value> = Vec::new();
    let mut system_segments: Vec<String> = Vec::new();
    let mut pending_tool_parts: Vec<(usize, Value)> = Vec::new();
    for msg in messages {
        let role = msg.get("role").and_then(Value::as_str).unwrap_or("user");
        if role != "tool" {
            flush_pending_gemini_function_responses(&mut contents, &mut pending_tool_parts);
        }
        if role == "system" || role == "developer" {
            let text = extract_text_content(msg.get("content"));
            if !text.is_empty() {
                system_segments.push(text);
            }
            continue;
        }
        if role == "user" {
            let parts = openai_content_to_gemini_parts(msg.get("content"))?;
            if !parts.is_empty() {
                contents.push(serde_json::json!({ "role": "user", "parts": parts }));
            }
        }
        if role == "assistant" {
            let mut parts = openai_content_to_gemini_parts(msg.get("content"))?;
            if let Some(tc) = msg.get("tool_calls").and_then(Value::as_array) {
                for (idx, t) in tc.iter().enumerate() {
                    let name = t.get("function").and_then(|f| f.get("name")).cloned();
                    if let (Some(id), Some(name)) =
                        (t.get("id").and_then(Value::as_str), name.clone())
                    {
                        tool_name_by_call_id.insert(id.to_string(), name);
                        tool_sort_key_by_call_id.insert(id.to_string(), next_tool_sort_key);
                        next_tool_sort_key += 1;
                    }
                    push_gemini_function_call_part(&mut parts, t, idx == 0)?;
                }
            }
            if !parts.is_empty() {
                contents.push(serde_json::json!({ "role": "model", "parts": parts }));
            }
        }
        if role == "tool" {
            let call_id = msg.get("tool_call_id").cloned();
            let call_id_str = msg.get("tool_call_id").and_then(Value::as_str);
            let function_name = msg
                .get("tool_call_id")
                .and_then(Value::as_str)
                .and_then(|id| tool_name_by_call_id.get(id).cloned())
                .or_else(|| msg.get("name").cloned())
                .unwrap_or_else(|| call_id.clone().unwrap_or(Value::Null));
            let content = msg
                .get("content")
                .cloned()
                .unwrap_or(Value::String(String::new()));
            let sort_key = call_id_str
                .and_then(|id| tool_sort_key_by_call_id.get(id).copied())
                .unwrap_or(next_tool_sort_key + pending_tool_parts.len());
            pending_tool_parts.push((
                sort_key,
                serde_json::json!({
                    "functionResponse": {
                        "id": call_id,
                        "name": function_name,
                        "response": { "result": content }
                    }
                }),
            ));
        }
    }
    flush_pending_gemini_function_responses(&mut contents, &mut pending_tool_parts);
    if !system_segments.is_empty() {
        result["systemInstruction"] = serde_json::json!({
            "role": "user",
            "parts": [{ "text": system_segments.join("\n\n") }]
        });
    }
    result["contents"] = Value::Array(contents);
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
    if let Some(tool_choice) = body.get("tool_choice") {
        if let Some(config) = openai_tool_choice_to_gemini_function_calling_config(
            tool_choice,
            body.get("allowed_tool_names"),
        ) {
            result["toolConfig"]["functionCallingConfig"] = config;
        }
    }
    *body = result;
    Ok(())
}

fn openai_content_to_gemini_parts(content: Option<&Value>) -> Result<Vec<Value>, String> {
    let content = match content {
        Some(c) => c,
        None => return Ok(vec![]),
    };
    if let Some(s) = content.as_str() {
        return Ok(vec![serde_json::json!({ "text": s })]);
    }
    let arr = match content.as_array() {
        Some(a) => a,
        None => return Ok(vec![]),
    };
    let mut parts = vec![];
    for c in arr {
        if let Some(part) = openai_content_part_to_gemini_part(c)? {
            parts.push(part);
        }
    }
    Ok(parts)
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
    fn openai_responses_round_trip_preserves_role_order_and_multimodal_parts() {
        let original = json!({
            "model": "gpt-4o",
            "messages": [
                { "role": "system", "content": "System A" },
                {
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "Look at this" },
                        { "type": "image_url", "image_url": { "url": "https://example.com/cat.png", "detail": "high" } },
                        { "type": "input_audio", "input_audio": { "data": "AAAA", "format": "wav" } },
                        { "type": "file", "file": { "file_id": "file_123" } }
                    ]
                },
                { "role": "developer", "content": "Developer B" },
                { "role": "user", "content": "Continue" }
            ]
        });
        let mut body = original.clone();
        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        assert!(body.get("instructions").is_none(), "body = {body}");
        let input = body["input"].as_array().expect("responses input");
        assert_eq!(input.len(), 4);
        assert_eq!(input[0]["role"], "system");
        assert_eq!(input[1]["role"], "user");
        assert_eq!(input[1]["content"][0]["type"], "input_text");
        assert_eq!(input[1]["content"][1]["type"], "input_image");
        assert_eq!(input[1]["content"][2]["type"], "input_audio");
        assert_eq!(input[1]["content"][3]["type"], "input_file");
        assert_eq!(input[2]["role"], "developer");
        assert_eq!(input[3]["role"], "user");

        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 4, "messages = {messages:?}");
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[2]["role"], "developer");
        assert_eq!(messages[3]["role"], "user");
        assert_eq!(messages[1]["content"][0]["type"], "text");
        assert_eq!(messages[1]["content"][1]["type"], "image_url");
        assert_eq!(messages[1]["content"][2]["type"], "input_audio");
        assert_eq!(messages[1]["content"][3]["type"], "file");
    }

    #[test]
    fn translate_request_chat_to_responses_maps_user_image_audio_and_file_legally() {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "Describe these inputs" },
                    { "type": "image_url", "image_url": { "url": "https://example.com/cat.png", "detail": "high" } },
                    { "type": "input_audio", "input_audio": { "data": "AAAA", "format": "wav" } },
                    { "type": "file", "file": { "file_id": "file_123" } }
                ]
            }]
        });

        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        let input = body["input"].as_array().expect("responses input");
        assert_eq!(input.len(), 1);
        let content = input[0]["content"].as_array().expect("content");
        assert_eq!(content[0]["type"], "input_text");
        assert_eq!(content[1]["type"], "input_image");
        assert_eq!(content[1]["image_url"], "https://example.com/cat.png");
        assert_eq!(content[1]["detail"], "high");
        assert_eq!(content[2]["type"], "input_audio");
        assert_eq!(content[2]["input_audio"]["data"], "AAAA");
        assert_eq!(content[2]["input_audio"]["format"], "wav");
        assert_eq!(content[3]["type"], "input_file");
        assert_eq!(content[3]["file_id"], "file_123");
    }

    #[test]
    fn translate_request_chat_to_responses_maps_user_image_to_input_image_legally() {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "Describe this image" },
                    { "type": "image_url", "image_url": { "url": "https://example.com/cat.png", "detail": "high" } }
                ]
            }]
        });

        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        let input = body["input"].as_array().expect("responses input");
        let content = input[0]["content"].as_array().expect("content");
        assert_eq!(content[0]["type"], "input_text");
        assert_eq!(content[1]["type"], "input_image");
        assert_eq!(content[1]["image_url"], "https://example.com/cat.png");
        assert_eq!(content[1]["detail"], "high");
    }

    #[test]
    fn translate_request_chat_to_responses_uses_custom_tool_call_output_for_custom_tool_results() {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [
                {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_custom",
                        "type": "custom",
                        "function": {
                            "name": "code_exec",
                            "arguments": "print('hi')"
                        }
                    }]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_custom",
                    "content": "exit 0"
                }
            ]
        });

        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        let input = body["input"].as_array().expect("responses input");
        assert_eq!(input[0]["type"], "custom_tool_call");
        assert_eq!(input[1]["type"], "custom_tool_call_output");
        assert_eq!(input[1]["call_id"], "call_custom");
        assert_eq!(input[1]["output"], "exit 0");
    }

    #[test]
    fn translate_request_responses_to_openai_keeps_custom_tool_call_output_messages() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": "run this" }]
                },
                {
                    "type": "custom_tool_call",
                    "call_id": "call_custom",
                    "name": "code_exec",
                    "input": "print('hi')"
                },
                {
                    "type": "custom_tool_call_output",
                    "call_id": "call_custom",
                    "output": "exit 0"
                }
            ]
        });

        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 3, "messages = {messages:?}");
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["tool_calls"][0]["type"], "custom");
        assert_eq!(messages[2]["role"], "tool");
        assert_eq!(messages[2]["tool_call_id"], "call_custom");
        assert_eq!(messages[2]["content"], "exit 0");
    }

    #[test]
    fn translate_request_chat_to_responses_maps_top_level_refusal_to_refusal_part() {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [{
                "role": "assistant",
                "content": null,
                "refusal": "I can't help with that."
            }]
        });

        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        let input = body["input"].as_array().expect("responses input");
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["type"], "message");
        let content = input[0]["content"].as_array().expect("responses content");
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "refusal");
        assert_eq!(content[0]["refusal"], "I can't help with that.");
    }

    #[test]
    fn translate_request_responses_to_openai_maps_refusal_to_top_level_message_field() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": [{
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "refusal", "refusal": "No." }]
            }]
        });

        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "assistant");
        assert_eq!(messages[0]["refusal"], "No.");
        assert!(messages[0]["content"].is_null() || messages[0]["content"] == "");
        assert_ne!(messages[0]["content"][0]["type"], "refusal");
    }

    #[test]
    fn translate_request_gemini_to_openai_maps_audio_inline_data_and_file_parts_without_image_spoofing(
    ) {
        let mut body = json!({
            "model": "gemini-2.5-flash",
            "contents": [{
                "role": "user",
                "parts": [
                    { "text": "Inspect these" },
                    { "inlineData": { "mimeType": "audio/wav", "data": "AAAA" } },
                    { "inlineData": { "mimeType": "application/pdf", "data": "JVBERi0x" } },
                    { "fileData": { "mimeType": "application/pdf", "fileUri": "gs://bucket/doc.pdf" } }
                ]
            }]
        });

        translate_request(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiCompletion,
            "gemini-2.5-flash",
            &mut body,
            false,
        )
        .unwrap();

        let messages = body["messages"].as_array().expect("messages");
        let content = messages[0]["content"].as_array().expect("content");
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "input_audio");
        assert_eq!(content[1]["input_audio"]["data"], "AAAA");
        assert_eq!(content[1]["input_audio"]["format"], "wav");
        assert_eq!(content[2]["type"], "file");
        assert_eq!(content[2]["file"]["file_data"], "data:application/pdf;base64,JVBERi0x");
        assert_eq!(content[3]["type"], "file");
        assert_eq!(content[3]["file"]["file_data"], "gs://bucket/doc.pdf");
        assert!(content.iter().all(|part| part["type"] != "image_url"));
    }

    #[test]
    fn translate_request_openai_to_gemini_maps_input_audio_and_file_parts() {
        let mut body = json!({
            "model": "gemini-2.5-flash",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "Inspect these" },
                    { "type": "input_audio", "input_audio": { "data": "AAAA", "format": "wav" } },
                    { "type": "file", "file": { "file_data": "data:application/pdf;base64,JVBERi0x", "filename": "doc.pdf" } }
                ]
            }]
        });

        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-2.5-flash",
            &mut body,
            false,
        )
        .unwrap();

        let parts = body["contents"][0]["parts"].as_array().expect("parts");
        assert_eq!(parts[0]["text"], "Inspect these");
        assert_eq!(parts[1]["inlineData"]["mimeType"], "audio/wav");
        assert_eq!(parts[1]["inlineData"]["data"], "AAAA");
        assert_eq!(parts[2]["inlineData"]["mimeType"], "application/pdf");
        assert_eq!(parts[2]["inlineData"]["data"], "JVBERi0x");
    }

    #[test]
    fn translate_request_openai_to_gemini_maps_file_uris_to_file_data() {
        let mut body = json!({
            "model": "gemini-2.5-flash",
            "messages": [{
                "role": "user",
                "content": [{ "type": "file", "file": { "file_data": "gs://bucket/doc.pdf", "filename": "doc.pdf" } }]
            }]
        });

        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-2.5-flash",
            &mut body,
            false,
        )
        .unwrap();

        let parts = body["contents"][0]["parts"].as_array().expect("parts");
        assert_eq!(parts[0]["fileData"]["fileUri"], "gs://bucket/doc.pdf");
        assert_eq!(parts[0]["fileData"]["displayName"], "doc.pdf");
    }

    #[test]
    fn translate_request_openai_to_gemini_maps_plain_base64_file_data_with_filename() {
        let mut body = json!({
            "model": "gemini-2.5-flash",
            "messages": [{
                "role": "user",
                "content": [{ "type": "file", "file": { "file_data": "JVBERi0x", "filename": "doc.pdf" } }]
            }]
        });

        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-2.5-flash",
            &mut body,
            false,
        )
        .unwrap();

        let parts = body["contents"][0]["parts"].as_array().expect("parts");
        assert_eq!(parts[0]["inlineData"]["mimeType"], "application/pdf");
        assert_eq!(parts[0]["inlineData"]["data"], "JVBERi0x");
    }

    #[test]
    fn translate_request_openai_to_gemini_rejects_plain_base64_file_data_without_mime_or_provenance()
    {
        let mut body = json!({
            "model": "gemini-2.5-flash",
            "messages": [{
                "role": "user",
                "content": [{ "type": "file", "file": { "file_data": "JVBERi0x" } }]
            }]
        });

        let err = translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-2.5-flash",
            &mut body,
            false,
        )
        .expect_err("plain base64 file_data without MIME should fail closed");

        assert!(err.contains("file_data"), "err = {err}");
        assert!(err.contains("Gemini"), "err = {err}");
    }

    #[test]
    fn translate_request_openai_to_gemini_rejects_unmappable_file_references() {
        let mut body = json!({
            "model": "gemini-2.5-flash",
            "messages": [{
                "role": "user",
                "content": [{ "type": "file", "file": { "file_id": "file_123" } }]
            }]
        });

        let err = translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-2.5-flash",
            &mut body,
            false,
        )
        .expect_err("unmappable file references should fail closed");

        assert!(err.contains("file"), "err = {err}");
    }

    #[test]
    fn translate_request_openai_to_responses_preserves_multiple_instruction_segments_without_merge()
    {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [
                { "role": "system", "content": "System A" },
                { "role": "user", "content": "User 1" },
                { "role": "developer", "content": "Developer B" },
                { "role": "user", "content": "User 2" }
            ]
        });

        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        assert!(body.get("instructions").is_none(), "body = {body}");
        let input = body["input"].as_array().expect("responses input");
        assert_eq!(input.len(), 4, "input = {input:?}");
        assert_eq!(input[0]["role"], "system");
        assert_eq!(input[0]["content"][0]["text"], "System A");
        assert_eq!(input[1]["role"], "user");
        assert_eq!(input[2]["role"], "developer");
        assert_eq!(input[2]["content"][0]["text"], "Developer B");
        assert_eq!(input[3]["role"], "user");
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
    fn translate_request_openai_same_format_coalesces_adjacent_string_messages() {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [
                { "role": "system", "content": "System A" },
                { "role": "developer", "content": "System B" },
                { "role": "user", "content": "User A" },
                { "role": "user", "content": "User B" }
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
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "System A\n\nSystem B");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "User A\n\nUser B");
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
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages[0]["content"], "Hello");
    }

    #[test]
    fn translate_request_responses_to_minimax_openai_enables_reasoning_split() {
        let mut body = json!({
            "model": "claude-openai",
            "input": [
                {
                    "type": "reasoning",
                    "summary": [{ "type": "summary_text", "text": "internal thinking" }]
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": "Hello" }]
                }
            ]
        });
        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "MiniMax-M2.7-highspeed",
            &mut body,
            true,
        )
        .unwrap();
        assert_eq!(body["reasoning_split"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(
            messages[0]["reasoning_details"][0]["text"],
            "internal thinking"
        );
    }

    #[test]
    fn translate_request_responses_to_openai_coalesces_adjacent_string_messages() {
        let mut body = json!({
            "model": "gpt-4o",
            "instructions": "System A",
            "input": [
                { "type": "message", "role": "developer", "content": [{ "type": "input_text", "text": "System B" }] },
                { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "User A" }] },
                { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "User B" }] }
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
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "System A\n\nSystem B");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "User A\n\nUser B");
    }

    #[test]
    fn translate_request_responses_to_openai_flattens_text_only_content_arrays() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": [
                        { "type": "input_text", "text": "Hello " },
                        { "type": "input_text", "text": "world" }
                    ]
                }
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
        assert_eq!(messages[0]["content"], "Hello world");
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
        assert_eq!(messages[0]["content"], "Hi");
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
    fn translate_request_responses_to_openai_translates_portable_fields_and_drops_responses_only_fields()
    {
        let mut body = json!({
            "model": "gpt-4o",
            "input": "Hello",
            "stream": true,
            "max_output_tokens": 123,
            "include": ["reasoning.encrypted_content"],
            "text": { "format": { "type": "text" } },
            "reasoning": { "effort": "medium" },
            "prompt_cache_key": "cache-key",
            "truncation": "auto"
        });
        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            true,
        )
        .unwrap();
        assert_eq!(body["max_completion_tokens"], 123);
        assert!(body.get("max_output_tokens").is_none());
        assert!(body.get("include").is_none());
        assert!(body.get("text").is_none());
        assert!(body.get("reasoning").is_none());
        assert!(body.get("prompt_cache_key").is_none());
        assert!(body.get("truncation").is_none());
    }

    #[test]
    fn translate_request_responses_to_openai_rejects_stateful_responses_controls() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": "Hello",
            "store": true,
            "previous_response_id": "resp_123"
        });

        let err = translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            true,
        )
        .expect_err("stateful responses controls should fail closed");

        assert_eq!(
            err,
            "Responses request controls `previous_response_id`, `store` require a native OpenAI Responses upstream and cannot be translated to openai-completion; the proxy does not reconstruct provider state"
        );
        assert_eq!(body["previous_response_id"], "resp_123");
        assert_eq!(body["store"], true);
    }

    #[test]
    fn translate_request_responses_to_openai_preserves_empty_input_and_uses_max_completion_tokens()
    {
        let mut body = json!({
            "model": "gpt-4o",
            "input": "",
            "max_output_tokens": 123
        });
        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "");
        assert_eq!(body["max_completion_tokens"], 123);
        assert!(body.get("max_output_tokens").is_none());
    }

    #[test]
    fn translate_request_responses_to_openai_keeps_empty_input_empty() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": ""
        });

        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "");
    }

    #[test]
    fn translate_request_responses_to_openai_preserves_mid_thread_instruction_segments_in_order() {
        let mut body = json!({
            "model": "MiniMax-M2.7-highspeed",
            "input": [
                { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "Earlier user message" }] },
                { "type": "message", "role": "developer", "content": [{ "type": "input_text", "text": "Compacted thread summary" }] },
                { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "Continue" }] }
            ]
        });
        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "MiniMax-M2.7-highspeed",
            &mut body,
            true,
        )
        .unwrap();
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "Earlier user message");
        assert_eq!(messages[1]["role"], "developer");
        assert_eq!(messages[1]["content"], "Compacted thread summary");
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(messages[2]["content"], "Continue");
    }

    #[test]
    fn translate_response_openai_reasoning_details_maps_to_responses_reasoning() {
        let body = json!({
            "id": "chatcmpl_1",
            "model": "MiniMax-M2.7-highspeed",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Hello",
                    "reasoning_details": [{ "text": "internal thinking" }]
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
        let output = out["output"].as_array().unwrap();
        assert_eq!(output[0]["type"], "reasoning");
        assert_eq!(output[0]["summary"][0]["text"], "internal thinking");
        assert_eq!(output[1]["type"], "message");
        assert_eq!(output[1]["content"][0]["text"], "Hello");
    }

    #[test]
    fn translate_response_openai_tool_only_turn_does_not_emit_empty_responses_message() {
        let body = json!({
            "id": "chatcmpl_tool_only",
            "model": "gpt-4o",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "lookup",
                            "arguments": "{\"city\":\"Tokyo\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            &body,
        )
        .unwrap();
        let output = out["output"].as_array().expect("responses output");
        assert_eq!(output.len(), 1, "output = {output:?}");
        assert_eq!(output[0]["type"], "function_call");
    }

    #[test]
    fn translate_response_openai_tool_only_turn_to_responses_does_not_create_empty_message_item() {
        let body = json!({
            "id": "chatcmpl_tool_only",
            "model": "gpt-4o",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "lookup",
                            "arguments": "{\"city\":\"Tokyo\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });

        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            &body,
        )
        .unwrap();

        let output = out["output"].as_array().expect("responses output");
        assert_eq!(output.len(), 1, "output = {output:?}");
        assert_eq!(output[0]["type"], "function_call");
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
        assert!(body.get("messages").is_some());
        assert!(!body["messages"].as_array().unwrap().is_empty());
    }

    #[test]
    fn translate_request_openai_to_claude_rejects_reasoning_without_provenance_before_mutating_blocks_or_cache_control()
    {
        let mut body = json!({
            "model": "claude-3",
            "messages": [
                {
                    "role": "assistant",
                    "reasoning_content": "private reasoning",
                    "content": "Visible answer"
                }
            ]
        });
        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Anthropic,
            "claude-3",
            &mut body,
            false,
        )
        .expect_err("reasoning replay to claude should fail closed");

        assert_eq!(
            body["messages"][0]["reasoning_content"],
            "private reasoning"
        );
        assert_eq!(body["messages"][0]["content"], "Visible answer");
        assert!(body["messages"][0].get("cache_control").is_none());
    }

    #[test]
    fn translate_request_openai_reasoning_to_claude_rejects_without_replay_provenance() {
        let mut body = json!({
            "model": "claude-3",
            "messages": [{
                "role": "assistant",
                "reasoning_content": "internal chain of thought",
                "content": "Visible answer"
            }]
        });

        let err = translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Anthropic,
            "claude-3",
            &mut body,
            false,
        )
        .expect_err("reasoning replay to claude should fail closed");

        assert_eq!(err, OPENAI_REASONING_TO_ANTHROPIC_REJECT_MESSAGE);
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
    fn translate_request_openai_to_gemini_preserves_function_response_name() {
        let mut body = json!({
            "model": "gemini-1.5",
            "messages": [
                { "role": "user", "content": "Hi" },
                {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "lookup_weather", "arguments": "{\"city\":\"Tokyo\"}" }
                    }]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_1",
                    "content": "{\"temperature\":22}"
                }
            ]
        });
        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-1.5",
            &mut body,
            false,
        )
        .unwrap();
        assert_eq!(
            body["contents"][2]["parts"][0]["functionResponse"]["id"],
            "call_1"
        );
        assert_eq!(
            body["contents"][2]["parts"][0]["functionResponse"]["name"],
            "lookup_weather"
        );
    }

    #[test]
    fn translate_request_openai_to_gemini_merges_parallel_function_responses_in_original_call_order(
    ) {
        let mut body = json!({
            "model": "gemini-1.5",
            "messages": [
                { "role": "user", "content": "Hi" },
                {
                    "role": "assistant",
                    "tool_calls": [
                        {
                            "id": "call_1",
                            "type": "function",
                            "function": { "name": "first", "arguments": "{\"step\":1}" }
                        },
                        {
                            "id": "call_2",
                            "type": "function",
                            "function": { "name": "second", "arguments": "{\"step\":2}" }
                        }
                    ]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_2",
                    "content": "{\"ok\":2}"
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_1",
                    "content": "{\"ok\":1}"
                }
            ]
        });
        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-1.5",
            &mut body,
            false,
        )
        .unwrap();

        let contents = body["contents"].as_array().expect("gemini contents");
        assert_eq!(contents.len(), 3, "contents = {contents:?}");
        let responses = contents[2]["parts"].as_array().expect("function responses");
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["functionResponse"]["id"], "call_1");
        assert_eq!(responses[1]["functionResponse"]["id"], "call_2");
    }

    #[test]
    fn translate_request_openai_to_gemini_maps_tool_choice_and_allowlist() {
        let mut body = json!({
            "model": "gemini-1.5",
            "messages": [{ "role": "user", "content": "Hi" }],
            "tool_choice": "required",
            "allowed_tool_names": ["lookup_weather", "lookup_time"]
        });
        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-1.5",
            &mut body,
            false,
        )
        .unwrap();

        assert_eq!(body["toolConfig"]["functionCallingConfig"]["mode"], "ANY");
        assert_eq!(
            body["toolConfig"]["functionCallingConfig"]["allowedFunctionNames"][0],
            "lookup_weather"
        );
        assert_eq!(
            body["toolConfig"]["functionCallingConfig"]["allowedFunctionNames"][1],
            "lookup_time"
        );
    }

    #[test]
    fn translate_request_openai_to_gemini_maps_tool_choice_modes() {
        let cases = [
            (json!("auto"), json!({ "mode": "AUTO" })),
            (json!("none"), json!({ "mode": "NONE" })),
            (json!("required"), json!({ "mode": "ANY" })),
            (
                json!({ "type": "function", "function": { "name": "lookup_weather" } }),
                json!({
                    "mode": "ANY",
                    "allowedFunctionNames": ["lookup_weather"]
                }),
            ),
        ];

        for (tool_choice, expected) in cases {
            let mut body = json!({
                "model": "gemini-1.5",
                "messages": [{ "role": "user", "content": "Hi" }],
                "tool_choice": tool_choice
            });
            translate_request(
                UpstreamFormat::OpenAiCompletion,
                UpstreamFormat::Google,
                "gemini-1.5",
                &mut body,
                false,
            )
            .unwrap();

            assert_eq!(body["toolConfig"]["functionCallingConfig"], expected);
        }
    }

    #[test]
    fn translate_request_openai_to_gemini_does_not_attach_allowlist_for_auto_or_none() {
        let cases = [("auto", "AUTO"), ("none", "NONE")];

        for (tool_choice, expected_mode) in cases {
            let mut body = json!({
                "model": "gemini-1.5",
                "messages": [{ "role": "user", "content": "Hi" }],
                "tool_choice": tool_choice,
                "allowed_tool_names": ["lookup_weather", "lookup_time"]
            });
            translate_request(
                UpstreamFormat::OpenAiCompletion,
                UpstreamFormat::Google,
                "gemini-1.5",
                &mut body,
                false,
            )
            .unwrap();

            let config = &body["toolConfig"]["functionCallingConfig"];
            assert_eq!(config["mode"], expected_mode);
            assert!(
                config.get("allowedFunctionNames").is_none(),
                "config = {config:?}"
            );
        }
    }

    #[test]
    fn translate_request_openai_to_gemini_tool_turns_use_dummy_signature_without_reasoning_replay()
    {
        let mut body = json!({
            "model": "gemini-1.5",
            "messages": [
                { "role": "user", "content": "Hi" },
                {
                    "role": "assistant",
                    "reasoning_content": "internal reasoning",
                    "content": "Calling tool.",
                    "tool_calls": [
                        {
                            "id": "call_1",
                            "type": "function",
                            "function": { "name": "lookup_weather", "arguments": "{\"city\":\"Tokyo\"}" }
                        },
                        {
                            "id": "call_2",
                            "type": "function",
                            "function": { "name": "lookup_time", "arguments": "{\"city\":\"Tokyo\"}" }
                        }
                    ]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_1",
                    "content": "{\"temperature\":22}"
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_2",
                    "content": "{\"time\":\"10:00\"}"
                }
            ]
        });
        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-1.5",
            &mut body,
            false,
        )
        .unwrap();
        let assistant_parts = body["contents"][1]["parts"]
            .as_array()
            .expect("assistant parts");
        assert!(assistant_parts.iter().all(|part| part["thought"] != true));
        assert_eq!(assistant_parts[0]["text"], "Calling tool.");
        assert!(assistant_parts[1].get("functionCall").is_some());
        assert_eq!(
            assistant_parts[1]["thoughtSignature"],
            "skip_thought_signature_validator"
        );
        assert!(assistant_parts[2].get("functionCall").is_some());
        assert!(assistant_parts[2].get("thoughtSignature").is_none());
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
    fn translate_request_openai_custom_tool_to_responses_preserves_custom_type() {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "Hi" }],
            "tools": [{
                "type": "custom",
                "name": "code_exec",
                "description": "Executes code with provider-managed semantics"
            }]
        });

        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        let tools = body["tools"].as_array().expect("responses tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "custom");
        assert_eq!(tools[0]["name"], "code_exec");
    }

    #[test]
    fn translate_request_openai_history_custom_tool_call_to_responses_preserves_custom_type() {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [{
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "custom",
                    "function": {
                        "name": "code_exec",
                        "arguments": "print('hi')"
                    }
                }]
            }]
        });

        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        let input = body["input"].as_array().expect("responses input");
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["type"], "custom_tool_call");
        assert_eq!(input[0]["call_id"], "call_1");
        assert_eq!(input[0]["name"], "code_exec");
        assert_eq!(input[0]["input"], "print('hi')");
    }

    #[test]
    fn translate_request_openai_history_custom_tool_call_to_gemini_rejects() {
        let mut body = json!({
            "model": "gemini-2.5-flash",
            "messages": [{
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "custom",
                    "function": {
                        "name": "code_exec",
                        "arguments": "print('hi')"
                    }
                }]
            }]
        });

        let err = translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-2.5-flash",
            &mut body,
            false,
        )
        .expect_err("custom tool calls in history should fail closed");

        assert_eq!(
            err,
            "OpenAI custom tools cannot be faithfully translated to Gemini; refusing to downgrade them to function tools"
        );
    }

    #[test]
    fn translate_request_openai_custom_tool_to_anthropic_rejects() {
        let mut body = json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "tools": [{
                "type": "custom",
                "name": "code_exec",
                "description": "Executes code with provider-managed semantics"
            }]
        });

        let err = translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Anthropic,
            "claude-3",
            &mut body,
            false,
        )
        .expect_err("custom tools should fail closed for anthropic");

        assert_eq!(
            err,
            "OpenAI custom tools cannot be faithfully translated to Anthropic; refusing to downgrade them to function tools"
        );
    }

    #[test]
    fn translate_request_openai_to_responses_maps_max_completion_tokens_to_max_output_tokens() {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "Hi" }],
            "max_completion_tokens": 222
        });

        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        assert_eq!(body["max_output_tokens"], 222);
        assert!(body.get("max_completion_tokens").is_none());
    }

    #[test]
    fn translate_request_responses_to_chat_maps_max_output_tokens_to_max_completion_tokens() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": "Hello",
            "max_output_tokens": 321
        });

        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        assert_eq!(body["max_completion_tokens"], 321);
        assert!(body.get("max_output_tokens").is_none());
    }

    #[test]
    fn translate_request_gemini_to_openai_maps_tool_policies() {
        let mut body = json!({
            "model": "gemini-1.5",
            "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
            "toolConfig": {
                "functionCallingConfig": {
                    "mode": "ANY",
                    "allowedFunctionNames": ["lookup_weather", "lookup_time"]
                }
            }
        });
        translate_request(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiCompletion,
            "gemini-1.5",
            &mut body,
            false,
        )
        .unwrap();

        assert_eq!(body["tool_choice"], "required");
        assert_eq!(body["allowed_tool_names"][0], "lookup_weather");
        assert_eq!(body["allowed_tool_names"][1], "lookup_time");
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
        let assistant_blocks = messages[1]["content"]
            .as_array()
            .expect("assistant content");
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
        let assistant_blocks = messages[1]["content"]
            .as_array()
            .expect("assistant content");
        assert_eq!(assistant_blocks[0]["type"], "text");
        assert_eq!(assistant_blocks[0]["text"], "Let me check.");
        assert_eq!(assistant_blocks[1]["type"], "tool_use");
        assert_eq!(assistant_blocks[1]["id"], "call_1");
        let user_blocks = messages[2]["content"].as_array().expect("user content");
        assert_eq!(user_blocks[0]["type"], "tool_result");
        assert_eq!(user_blocks[0]["tool_use_id"], "call_1");
    }

    #[test]
    fn translate_request_responses_to_claude_moves_user_warning_after_tool_result() {
        let mut body = json!({
            "model": "codex-anthropic",
            "input": [
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": "Running test..." }]
                },
                {
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "exec_command",
                    "arguments": "{\"cmd\":\"python test.py\"}"
                },
                {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": "Warning: process limit reached" }]
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_1",
                    "output": "Done"
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
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1]["role"], "user");
        let user_blocks = messages[1]["content"].as_array().expect("user content");
        assert_eq!(user_blocks[0]["type"], "tool_result");
        assert_eq!(user_blocks[0]["tool_use_id"], "call_1");
        assert_eq!(user_blocks[1]["type"], "text");
        assert_eq!(user_blocks[1]["text"], "Warning: process limit reached");
    }

    #[test]
    fn translate_request_claude_structured_tool_result_content_round_trips() {
        let mut body = json!({
            "model": "claude-3",
            "messages": [
                {
                    "role": "assistant",
                    "content": [{
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": "lookup_weather",
                        "input": { "city": "Tokyo" }
                    }]
                },
                {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": "toolu_1",
                        "content": [
                            { "type": "text", "text": "done" },
                            { "type": "json", "json": { "temperature": 22 } }
                        ]
                    }]
                }
            ]
        });

        translate_request(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiCompletion,
            "claude-3",
            &mut body,
            false,
        )
        .unwrap();

        let messages = body["messages"].as_array().expect("openai messages");
        assert_eq!(messages[1]["role"], "tool");
        assert!(
            messages[1]["content"].is_array(),
            "content should stay structured, body = {body:?}"
        );

        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Anthropic,
            "claude-3",
            &mut body,
            false,
        )
        .unwrap();

        let content = body["messages"][1]["content"].as_array().expect("claude content");
        assert_eq!(content[0]["type"], "tool_result");
        assert!(content[0]["content"].is_array(), "body = {body:?}");
        assert_eq!(content[0]["content"][0]["type"], "text");
        assert_eq!(content[0]["content"][1]["type"], "json");
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
    fn translate_request_claude_to_openai_rejects_top_level_cache_control_cross_protocol() {
        let mut body = json!({
            "model": "claude-3",
            "cache_control": { "type": "ephemeral" },
            "messages": [{ "role": "user", "content": "Hi" }]
        });

        let err = translate_request(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiCompletion,
            "claude-3",
            &mut body,
            false,
        )
        .expect_err("top-level cache_control should fail closed");

        assert!(err.contains("cache_control"), "err = {err}");
    }

    #[test]
    fn translate_request_claude_to_openai_rejects_unsupported_document_block() {
        let mut body = json!({
            "model": "claude-3",
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "document",
                    "source": {
                        "type": "base64",
                        "media_type": "application/pdf",
                        "data": "JVBERi0x"
                    }
                }]
            }]
        });

        let err = translate_request(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiCompletion,
            "claude-3",
            &mut body,
            false,
        )
        .expect_err("unsupported typed blocks should not be silently dropped");

        assert!(err.contains("document"), "err = {err}");
    }

    #[test]
    fn translate_request_claude_to_openai_rejects_future_unknown_block() {
        let mut body = json!({
            "model": "claude-3",
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "mystery_block",
                    "payload": "???"
                }]
            }]
        });

        let err = translate_request(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiCompletion,
            "claude-3",
            &mut body,
            false,
        )
        .expect_err("future unknown anthropic blocks should fail closed");

        assert!(err.contains("mystery_block"), "err = {err}");
    }

    #[test]
    fn translate_request_claude_to_openai_allows_business_cache_control_keys() {
        let mut body = json!({
            "model": "claude-3",
            "messages": [
                { "role": "user", "content": "Use the tool" },
                {
                    "role": "assistant",
                    "content": [{
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": "lookup_weather",
                        "input": {
                            "city": "Tokyo",
                            "cache_control": "business-value"
                        }
                    }]
                }
            ],
            "tools": [{
                "name": "lookup_weather",
                "description": "Weather lookup",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "city": { "type": "string" },
                        "cache_control": { "type": "string" }
                    }
                }
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

        let tool_calls = body["messages"][1]["tool_calls"]
            .as_array()
            .expect("assistant tool calls");
        let arguments: Value = serde_json::from_str(
            tool_calls[0]["function"]["arguments"]
                .as_str()
                .expect("arguments string"),
        )
        .expect("tool arguments json");
        assert_eq!(arguments["cache_control"], "business-value");
        assert_eq!(
            body["tools"][0]["function"]["parameters"]["properties"]["cache_control"]["type"],
            "string"
        );
    }

    #[test]
    fn translate_request_openai_invalid_tool_arguments_to_claude_rejects_instead_of_coercing_empty_object(
    ) {
        let mut body = json!({
            "model": "claude-3",
            "messages": [{
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "lookup_weather",
                        "arguments": "{\"city\":\"Tokyo\""
                    }
                }]
            }]
        });

        let err = translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Anthropic,
            "claude-3",
            &mut body,
            false,
        )
        .expect_err("invalid JSON arguments should fail closed");

        assert!(err.contains("arguments"), "err = {err}");
        assert!(err.contains("JSON"), "err = {err}");
    }

    #[test]
    fn translate_request_openai_non_object_tool_arguments_to_gemini_rejects() {
        let mut body = json!({
            "model": "gemini-2.5-flash",
            "messages": [{
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "lookup_weather",
                        "arguments": "[]"
                    }
                }]
            }]
        });

        let err = translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-2.5-flash",
            &mut body,
            false,
        )
        .expect_err("non-object tool input should fail closed");

        assert!(
            err.contains("JSON object"),
            "expected object-specific failure, got {err}"
        );
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
    fn translate_request_openai_to_gemini_rejects_n_greater_than_one() {
        let mut body = json!({
            "model": "gemini-2.5-flash",
            "n": 2,
            "messages": [{ "role": "user", "content": "Hi" }]
        });

        let err = translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-2.5-flash",
            &mut body,
            false,
        )
        .expect_err("cross-protocol n > 1 should fail closed");

        assert!(err.contains("n"), "err = {err}");
        assert!(err.contains("single"), "err = {err}");
    }

    #[test]
    fn translate_request_gemini_to_openai_rejects_candidate_count_greater_than_one() {
        let mut body = json!({
            "model": "gemini-2.5-flash",
            "generationConfig": { "candidateCount": 2 },
            "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }]
        });

        let err = translate_request(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiCompletion,
            "gemini-2.5-flash",
            &mut body,
            false,
        )
        .expect_err("cross-protocol candidateCount > 1 should fail closed");

        assert!(err.contains("candidateCount"), "err = {err}");
        assert!(err.contains("single"), "err = {err}");
    }

    #[test]
    fn translate_request_gemini_to_openai_accepts_snake_case_parts() {
        let mut body = json!({
            "model": "gemini-1.5",
            "contents": [{
                "parts": [
                    {
                        "inline_data": {
                            "mime_type": "image/jpeg",
                            "data": "abc123"
                        }
                    },
                    {
                        "function_call": {
                            "id": "call_1",
                            "name": "lookup_weather",
                            "args": { "city": "Tokyo" }
                        }
                    }
                ]
            }]
        });
        translate_request(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiCompletion,
            "gemini-1.5",
            &mut body,
            false,
        )
        .unwrap();
        assert_eq!(body["messages"][0]["role"], "assistant");
        assert_eq!(body["messages"][0]["tool_calls"][0]["id"], "call_1");
        assert_eq!(
            body["messages"][0]["tool_calls"][0]["function"]["name"],
            "lookup_weather"
        );
        assert!(body["messages"][0]["content"]
            .as_array()
            .expect("array content")
            .iter()
            .any(|part| part["type"] == "image_url"));
    }

    #[test]
    fn translate_request_gemini_to_openai_preserves_text_and_function_response_order() {
        let mut body = json!({
            "model": "gemini-1.5",
            "contents": [{
                "role": "user",
                "parts": [
                    { "text": "Before tool." },
                    {
                        "functionResponse": {
                            "id": "call_1",
                            "name": "lookup_weather",
                            "response": { "result": { "temperature": 22 } }
                        }
                    },
                    { "text": "After tool." }
                ]
            }]
        });
        translate_request(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiCompletion,
            "gemini-1.5",
            &mut body,
            false,
        )
        .unwrap();
        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "Before tool.");
        assert_eq!(messages[1]["role"], "tool");
        assert_eq!(messages[1]["tool_call_id"], "call_1");
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(messages[2]["content"], "After tool.");
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
        assert_eq!(
            out["choices"][0]["finish_reason"],
            "context_length_exceeded"
        );
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
    fn translate_response_claude_refusal_sets_openai_refusal_surface() {
        let body = json!({
            "id": "msg_1",
            "content": [{ "type": "text", "text": "I can't help with that." }],
            "stop_reason": "refusal",
            "model": "claude-3"
        });

        let openai = translate_response(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .unwrap();
        assert_eq!(openai["choices"][0]["finish_reason"], "content_filter");
        assert_eq!(
            openai["choices"][0]["message"]["refusal"],
            "I can't help with that."
        );
        assert!(openai["choices"][0]["message"]["content"].is_null());

        let responses = translate_response(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiResponses,
            &body,
        )
        .unwrap();
        assert_eq!(responses["status"], "incomplete");
        let output = responses["output"].as_array().expect("responses output");
        assert_eq!(output[0]["type"], "message");
        assert_eq!(output[0]["content"][0]["type"], "refusal");
        assert_eq!(
            output[0]["content"][0]["refusal"],
            "I can't help with that."
        );
    }

    #[test]
    fn translate_response_responses_to_openai_preserves_text_and_refusal_together() {
        let body = json!({
            "id": "resp_refusal_mix",
            "object": "response",
            "created_at": 1,
            "status": "completed",
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [
                    { "type": "output_text", "text": "Visible answer." },
                    { "type": "refusal", "refusal": "But I can't help with the unsafe part." }
                ]
            }]
        });

        let out = translate_response(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .unwrap();

        let message = &out["choices"][0]["message"];
        assert_eq!(message["content"], "Visible answer.");
        assert_eq!(message["refusal"], "But I can't help with the unsafe part.");
    }

    #[test]
    fn translate_response_claude_to_openai_rejects_unsupported_redacted_thinking_block() {
        let body = json!({
            "id": "msg_redacted",
            "content": [{
                "type": "redacted_thinking",
                "data": "opaque"
            }],
            "stop_reason": "end_turn",
            "model": "claude-3"
        });

        let err = translate_response(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .expect_err("unsupported response blocks should fail closed");

        assert!(err.contains("redacted_thinking"), "err = {err}");
    }

    #[test]
    fn translate_response_claude_to_openai_rejects_future_unknown_block() {
        let body = json!({
            "id": "msg_unknown",
            "content": [{
                "type": "mystery_block",
                "payload": "???"
            }],
            "stop_reason": "end_turn",
            "model": "claude-3"
        });

        let err = translate_response(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .expect_err("future unknown anthropic response blocks should fail closed");

        assert!(err.contains("mystery_block"), "err = {err}");
    }

    #[test]
    fn translate_response_claude_server_tool_use_preserved_non_streaming() {
        let body = json!({
            "id": "msg_server_tool",
            "content": [{
                "type": "server_tool_use",
                "id": "toolu_server_1",
                "name": "web_search",
                "input": { "query": "rust" }
            }],
            "stop_reason": "tool_use",
            "model": "claude-3"
        });

        let out = translate_response(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .unwrap();

        let tool_calls = out["choices"][0]["message"]["tool_calls"]
            .as_array()
            .expect("tool calls");
        assert_eq!(tool_calls[0]["proxied_tool_kind"], "anthropic_server_tool_use");
        assert_eq!(tool_calls[0]["function"]["name"], "web_search");
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
    fn translate_response_openai_error_finish_to_claude_error_body() {
        let body = json!({
            "id": "chatcmpl-1",
            "choices": [{ "message": { "role": "assistant", "content": "" }, "finish_reason": "error" }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Anthropic,
            &body,
        )
        .unwrap();
        assert_eq!(out["type"], "error");
        assert_eq!(out["error"]["type"], "api_error");
        assert!(out.get("stop_reason").is_none());
    }

    #[test]
    fn translate_response_openai_tool_error_finish_to_claude_error_body() {
        let body = json!({
            "id": "chatcmpl-1",
            "choices": [{ "message": { "role": "assistant", "content": "" }, "finish_reason": "tool_error" }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Anthropic,
            &body,
        )
        .unwrap();
        assert_eq!(out["type"], "error");
        assert_eq!(out["error"]["type"], "invalid_request_error");
        assert!(out.get("stop_reason").is_none());
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
    fn translate_response_responses_incomplete_to_openai_preserves_terminal_and_usage_details() {
        let body = json!({
            "id": "resp_1",
            "object": "response",
            "created_at": 42,
            "status": "incomplete",
            "incomplete_details": { "reason": "max_output_tokens" },
            "output": [{
                "id": "msg_1",
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
        assert_eq!(out["choices"][0]["message"]["content"], "Hi");
        assert_eq!(out["choices"][0]["finish_reason"], "length");
        assert_eq!(out["usage"]["prompt_tokens_details"]["cached_tokens"], 3);
        assert_eq!(
            out["usage"]["completion_tokens_details"]["reasoning_tokens"],
            2
        );
    }

    #[test]
    fn translate_response_responses_failed_to_openai_maps_context_failure() {
        let body = json!({
            "id": "resp_1",
            "object": "response",
            "created_at": 42,
            "status": "failed",
            "error": { "code": "context_length_exceeded" },
            "output": [{
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": "" }]
            }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .unwrap();
        assert_eq!(
            out["choices"][0]["finish_reason"],
            "context_length_exceeded"
        );
    }

    #[test]
    fn translate_response_responses_failed_unknown_code_maps_to_error() {
        let body = json!({
            "id": "resp_1",
            "object": "response",
            "created_at": 42,
            "status": "failed",
            "error": { "code": "server_error" },
            "output": [{
                "id": "fc_1",
                "type": "function_call",
                "call_id": "call_1",
                "name": "lookup_weather",
                "arguments": "{\"city\":\"Tokyo\"}"
            }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .unwrap();
        assert_eq!(out["choices"][0]["finish_reason"], "error");
        assert!(out["choices"][0]["message"]["tool_calls"].is_array());
    }

    #[test]
    fn translate_response_responses_failed_tool_validation_maps_to_tool_error() {
        let body = json!({
            "id": "resp_1",
            "object": "response",
            "created_at": 42,
            "status": "failed",
            "error": { "code": "tool_validation_error" },
            "output": [{
                "id": "fc_1",
                "type": "function_call",
                "call_id": "call_1",
                "name": "lookup_weather",
                "arguments": "{\"city\":\"Tokyo\"}"
            }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .unwrap();
        assert_eq!(out["choices"][0]["finish_reason"], "tool_error");
        assert!(out["choices"][0]["message"]["tool_calls"].is_array());
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
    fn translate_response_openai_annotations_round_trip_to_responses() {
        let body = json!({
            "id": "chatcmpl_annotations",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": [{
                        "type": "text",
                        "text": "Rust",
                        "annotations": [{
                            "type": "url_citation",
                            "url": "https://www.rust-lang.org"
                        }]
                    }]
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

        let content = out["output"][0]["content"].as_array().expect("responses content");
        assert_eq!(content[0]["type"], "output_text");
        assert_eq!(
            content[0]["annotations"][0]["url"],
            "https://www.rust-lang.org"
        );
    }

    #[test]
    fn translate_response_responses_to_openai_preserves_interleaved_annotation_order() {
        let body = json!({
            "id": "resp_annotations",
            "object": "response",
            "created_at": 1,
            "status": "completed",
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [
                    {
                        "type": "output_text",
                        "text": "annotated-1",
                        "annotations": [{
                            "type": "url_citation",
                            "url": "https://one.example"
                        }]
                    },
                    {
                        "type": "output_text",
                        "text": "plain-middle"
                    },
                    {
                        "type": "output_text",
                        "text": "annotated-2",
                        "annotations": [{
                            "type": "url_citation",
                            "url": "https://two.example"
                        }]
                    }
                ]
            }]
        });

        let out = translate_response(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .unwrap();

        let content = out["choices"][0]["message"]["content"]
            .as_array()
            .expect("openai content array");
        assert_eq!(content[0]["text"], "annotated-1");
        assert_eq!(content[1]["text"], "plain-middle");
        assert_eq!(content[2]["text"], "annotated-2");
        assert_eq!(content[0]["annotations"][0]["url"], "https://one.example");
        assert!(content[1].get("annotations").is_none(), "content = {content:?}");
        assert_eq!(content[2]["annotations"][0]["url"], "https://two.example");
    }

    #[test]
    fn translate_response_responses_to_openai_preserves_custom_and_proxied_tool_kinds() {
        let body = json!({
            "id": "resp_tools",
            "object": "response",
            "created_at": 1,
            "status": "completed",
            "output": [
                {
                    "type": "custom_tool_call",
                    "call_id": "call_custom",
                    "name": "code_exec",
                    "input": "print('hi')"
                },
                {
                    "type": "function_call",
                    "call_id": "call_server",
                    "name": "web_search",
                    "arguments": "{\"query\":\"rust\"}",
                    "proxied_tool_kind": "anthropic_server_tool_use"
                }
            ]
        });

        let out = translate_response(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .unwrap();

        let tool_calls = out["choices"][0]["message"]["tool_calls"]
            .as_array()
            .expect("tool calls");
        assert_eq!(tool_calls[0]["type"], "custom");
        assert_eq!(tool_calls[0]["function"]["name"], "code_exec");
        assert_eq!(tool_calls[0]["function"]["arguments"], "print('hi')");
        assert_eq!(
            tool_calls[1]["proxied_tool_kind"],
            "anthropic_server_tool_use"
        );
    }

    #[test]
    fn translate_response_openai_to_claude_restores_server_tool_use_from_marker() {
        let body = json!({
            "id": "chatcmpl_server_tool",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "server_1",
                        "type": "function",
                        "proxied_tool_kind": "anthropic_server_tool_use",
                        "function": {
                            "name": "web_search",
                            "arguments": "{\"query\":\"rust\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });

        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Anthropic,
            &body,
        )
        .unwrap();

        let content = out["content"].as_array().expect("anthropic content");
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "server_tool_use");
        assert_eq!(content[0]["name"], "web_search");
    }

    #[test]
    fn translate_response_openai_to_responses_preserves_custom_and_proxied_tool_kinds() {
        let body = json!({
            "id": "chatcmpl_tools",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "tool_calls": [
                        {
                            "id": "call_custom",
                            "type": "custom",
                            "function": {
                                "name": "code_exec",
                                "arguments": "print('hi')"
                            }
                        },
                        {
                            "id": "call_server",
                            "type": "function",
                            "proxied_tool_kind": "anthropic_server_tool_use",
                            "function": {
                                "name": "web_search",
                                "arguments": "{\"query\":\"rust\"}"
                            }
                        }
                    ]
                },
                "finish_reason": "tool_calls"
            }]
        });

        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            &body,
        )
        .unwrap();

        let output = out["output"].as_array().expect("responses output");
        assert_eq!(output[0]["type"], "custom_tool_call");
        assert_eq!(output[0]["input"], "print('hi')");
        assert_eq!(output[1]["type"], "function_call");
        assert_eq!(output[1]["proxied_tool_kind"], "anthropic_server_tool_use");
    }

    #[test]
    fn translate_response_openai_to_claude_preserves_text_annotations_as_citations() {
        let body = json!({
            "id": "chatcmpl_citations",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": [{
                        "type": "text",
                        "text": "Fact.",
                        "annotations": [{
                            "type": "url_citation",
                            "url": "https://example.com/fact"
                        }]
                    }]
                },
                "finish_reason": "stop"
            }]
        });

        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Anthropic,
            &body,
        )
        .unwrap();

        let content = out["content"].as_array().expect("anthropic content");
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["citations"][0]["url"], "https://example.com/fact");
    }

    #[test]
    fn translate_response_claude_usage_preserves_extra_usage_fields() {
        let body = json!({
            "id": "msg_usage",
            "content": [{ "type": "text", "text": "Hi" }],
            "stop_reason": "end_turn",
            "model": "claude-3",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 5,
                "cache_read_input_tokens": 2,
                "cache_creation_input_tokens": 3,
                "service_tier": "priority",
                "output_tokens_details": {
                    "reasoning_tokens": 4
                }
            }
        });

        let out = translate_response(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .unwrap();

        assert_eq!(out["usage"]["prompt_tokens"], 15);
        assert_eq!(out["usage"]["completion_tokens"], 5);
        assert_eq!(out["usage"]["cache_read_input_tokens"], 2);
        assert_eq!(out["usage"]["cache_creation_input_tokens"], 3);
        assert_eq!(out["usage"]["service_tier"], "priority");
        assert_eq!(
            out["usage"]["output_tokens_details"]["reasoning_tokens"],
            4
        );
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
    fn translate_response_openai_to_responses_maps_context_window_to_failed() {
        let body = json!({
            "id": "chatcmpl_1",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "" },
                "finish_reason": "context_length_exceeded"
            }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            &body,
        )
        .unwrap();
        assert_eq!(out["status"], "failed");
        assert_eq!(out["error"]["code"], "context_length_exceeded");
        assert_eq!(out["incomplete_details"], serde_json::Value::Null);
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

    #[test]
    fn translate_response_openai_to_gemini_tool_calls_include_dummy_signature() {
        let body = json!({
            "id": "chatcmpl_gem_fc",
            "object": "chat.completion",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "reasoning_content": "Need tools.",
                    "content": "Calling tools.",
                    "tool_calls": [
                        {
                            "id": "call_1",
                            "type": "function",
                            "function": { "name": "lookup_weather", "arguments": "{\"city\":\"Tokyo\"}" }
                        },
                        {
                            "id": "call_2",
                            "type": "function",
                            "function": { "name": "lookup_time", "arguments": "{\"city\":\"Tokyo\"}" }
                        }
                    ]
                },
                "finish_reason": "tool_calls"
            }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            &body,
        )
        .unwrap();
        let parts = out["candidates"][0]["content"]["parts"]
            .as_array()
            .expect("gemini parts");
        let function_parts = parts
            .iter()
            .filter(|part| part.get("functionCall").is_some())
            .collect::<Vec<_>>();
        assert_eq!(
            function_parts[0]["thoughtSignature"],
            "skip_thought_signature_validator"
        );
        assert!(function_parts[1].get("thoughtSignature").is_none());
    }

    #[test]
    fn translate_response_gemini_to_openai_maps_finish_and_reasoning_usage_details() {
        let body = json!({
            "response": {
                "responseId": "gem_resp_1",
                "modelVersion": "gemini-2.5",
                "candidates": [{
                    "content": {
                        "role": "model",
                        "parts": [{ "text": "Hi" }]
                    },
                    "finishReason": "MAX_TOKENS"
                }],
                "usageMetadata": {
                    "promptTokenCount": 11,
                    "candidatesTokenCount": 5,
                    "thoughtsTokenCount": 2,
                    "totalTokenCount": 18,
                    "cachedContentTokenCount": 3
                }
            }
        });
        let out = translate_response(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .unwrap();
        assert_eq!(out["id"], "gem_resp_1");
        assert_eq!(out["model"], "gemini-2.5");
        assert_eq!(out["choices"][0]["finish_reason"], "length");
        assert_eq!(out["usage"]["prompt_tokens"], 11);
        assert_eq!(out["usage"]["completion_tokens"], 7);
        assert_eq!(out["usage"]["prompt_tokens_details"]["cached_tokens"], 3);
        assert_eq!(
            out["usage"]["completion_tokens_details"]["reasoning_tokens"],
            2
        );
    }

    #[test]
    fn translate_response_gemini_prompt_feedback_without_candidates_is_not_an_error() {
        let body = json!({
            "promptFeedback": {
                "blockReason": "SAFETY"
            },
            "usageMetadata": {
                "promptTokenCount": 3,
                "totalTokenCount": 3
            },
            "modelVersion": "gemini-2.5"
        });
        let out = translate_response(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .expect("translated response");

        assert_eq!(out["object"], "chat.completion");
        assert_eq!(out["choices"][0]["finish_reason"], "content_filter");
        assert_eq!(out["usage"]["prompt_tokens"], 3);
    }

    #[test]
    fn translate_response_gemini_to_openai_rejects_multiple_candidates() {
        let body = json!({
            "response": {
                "responseId": "gem_resp_multi",
                "modelVersion": "gemini-2.5",
                "candidates": [
                    {
                        "content": { "role": "model", "parts": [{ "text": "Hi" }] },
                        "finishReason": "STOP"
                    },
                    {
                        "content": { "role": "model", "parts": [{ "text": "Hello" }] },
                        "finishReason": "STOP"
                    }
                ]
            }
        });

        let err = translate_response(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .expect_err("multiple Gemini candidates should fail closed");

        assert!(err.contains("candidates"), "err = {err}");
        assert!(err.contains("single"), "err = {err}");
    }

    #[test]
    fn translate_response_gemini_non_success_finish_reasons_do_not_collapse_to_success() {
        let cases = [
            ("MALFORMED_FUNCTION_CALL", "tool_error"),
            ("UNEXPECTED_TOOL_CALL", "tool_error"),
            ("TOO_MANY_TOOL_CALLS", "tool_error"),
            ("MISSING_THOUGHT_SIGNATURE", "tool_error"),
            ("IMAGE_OTHER", "error"),
            ("NO_IMAGE", "error"),
            ("LANGUAGE", "error"),
        ];

        for (reason, expected) in cases {
            let body = json!({
                "response": {
                    "responseId": format!("gem_{reason}"),
                    "modelVersion": "gemini-2.5",
                    "candidates": [{
                        "content": {
                            "role": "model",
                            "parts": [{
                                "functionCall": {
                                    "id": "call_1",
                                    "name": "lookup_weather",
                                    "args": { "city": "Tokyo" }
                                }
                            }]
                        },
                        "finishReason": reason
                    }]
                }
            });
            let out = translate_response(
                UpstreamFormat::Google,
                UpstreamFormat::OpenAiCompletion,
                &body,
            )
            .unwrap();
            assert_eq!(
                out["choices"][0]["finish_reason"], expected,
                "reason = {reason}, body = {out:?}"
            );
        }
    }

    #[test]
    fn translate_response_openai_to_responses_maps_error_finish_to_failed() {
        let body = json!({
            "id": "chatcmpl_1",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "" },
                "finish_reason": "error"
            }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            &body,
        )
        .unwrap();
        assert_eq!(out["status"], "failed");
        assert_eq!(out["error"]["code"], "error");
    }

    #[test]
    fn translate_response_openai_to_responses_maps_tool_error_finish_to_failed() {
        let body = json!({
            "id": "chatcmpl_1",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "" },
                "finish_reason": "tool_error"
            }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            &body,
        )
        .unwrap();
        assert_eq!(out["status"], "failed");
        assert_eq!(out["error"]["code"], "tool_error");
    }

    #[test]
    fn translate_response_openai_to_gemini_maps_portable_finish_reasons() {
        let body = json!({
            "id": "chatcmpl_1",
            "object": "chat.completion",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "Hi" },
                "finish_reason": "content_filter"
            }]
        });
        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            &body,
        )
        .unwrap();
        assert_eq!(out["responseId"], "chatcmpl_1");
        assert_eq!(out["modelVersion"], "gpt-4o");
        assert_eq!(out["candidates"][0]["finishReason"], "SAFETY");
    }

    #[test]
    fn translate_response_openai_refusal_to_gemini_preserves_text_part_and_safety_finish() {
        let body = json!({
            "id": "chatcmpl_refusal",
            "object": "chat.completion",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "refusal": "I can't help with that."
                },
                "finish_reason": "content_filter"
            }]
        });

        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            &body,
        )
        .unwrap();

        let parts = out["candidates"][0]["content"]["parts"]
            .as_array()
            .expect("gemini parts");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["text"], "I can't help with that.");
        assert_eq!(out["candidates"][0]["finishReason"], "SAFETY");
    }

    #[test]
    fn translate_response_openai_text_and_refusal_to_gemini_preserves_both() {
        let body = json!({
            "id": "chatcmpl_text_refusal",
            "object": "chat.completion",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Visible answer.",
                    "refusal": "But I can't help with the unsafe part."
                },
                "finish_reason": "content_filter"
            }]
        });

        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            &body,
        )
        .unwrap();

        let parts = out["candidates"][0]["content"]["parts"]
            .as_array()
            .expect("gemini parts");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["text"], "Visible answer.");
        assert_eq!(parts[1]["text"], "But I can't help with the unsafe part.");
        assert_eq!(out["candidates"][0]["finishReason"], "SAFETY");
    }

    #[test]
    fn translate_response_openai_to_gemini_rejects_multiple_choices() {
        let body = json!({
            "id": "chatcmpl_multi",
            "object": "chat.completion",
            "model": "gpt-4o",
            "choices": [
                {
                    "index": 0,
                    "message": { "role": "assistant", "content": "Hi" },
                    "finish_reason": "stop"
                },
                {
                    "index": 1,
                    "message": { "role": "assistant", "content": "Hello" },
                    "finish_reason": "stop"
                }
            ]
        });

        let err = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            &body,
        )
        .expect_err("multiple OpenAI choices should fail closed");

        assert!(err.contains("choices"), "err = {err}");
        assert!(err.contains("single"), "err = {err}");
    }

    #[test]
    fn translate_response_gemini_to_openai_rejects_multimodal_assistant_output_parts() {
        let body = json!({
            "response": {
                "responseId": "gem_resp_multimodal",
                "modelVersion": "gemini-2.5",
                "candidates": [{
                    "content": {
                        "role": "model",
                        "parts": [
                            { "text": "Look at this." },
                            { "inlineData": { "mimeType": "image/png", "data": "iVBORw0KGgo" } },
                            { "inlineData": { "mimeType": "audio/wav", "data": "AAAA" } },
                            { "inlineData": { "mimeType": "application/pdf", "data": "JVBERi0x" } },
                            { "fileData": { "mimeType": "application/pdf", "fileUri": "gs://bucket/doc.pdf", "displayName": "doc.pdf" } }
                        ]
                    },
                    "finishReason": "STOP"
                }]
            }
        });

        let out = translate_response(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .expect_err("Gemini assistant multimodal output should fail closed");

        assert!(out.contains("Gemini"), "err = {out}");
        assert!(out.contains("assistant"), "err = {out}");
    }

    #[test]
    fn translate_response_gemini_to_openai_rejects_unrepresentable_output_part() {
        let body = json!({
            "response": {
                "responseId": "gem_resp_code",
                "modelVersion": "gemini-2.5",
                "candidates": [{
                    "content": {
                        "role": "model",
                        "parts": [{
                            "executableCode": {
                                "code": "print('hi')",
                                "language": "PYTHON"
                            }
                        }]
                    },
                    "finishReason": "STOP"
                }]
            }
        });

        let err = translate_response(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .expect_err("unrepresentable Gemini output parts should fail closed");

        assert!(err.contains("executableCode"), "err = {err}");
        assert!(err.contains("OpenAI"), "err = {err}");
    }

    #[test]
    fn translate_response_gemini_to_openai_allows_text_and_function_call_output() {
        let body = json!({
            "response": {
                "responseId": "gem_resp_text_tool",
                "modelVersion": "gemini-2.5",
                "candidates": [{
                    "content": {
                        "role": "model",
                        "parts": [
                            { "text": "Need a tool." },
                            {
                                "functionCall": {
                                    "id": "call_1",
                                    "name": "lookup_weather",
                                    "args": { "city": "Tokyo" }
                                }
                            }
                        ]
                    },
                    "finishReason": "STOP"
                }]
            }
        });

        let out = translate_response(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .unwrap();

        assert_eq!(out["choices"][0]["message"]["content"], "Need a tool.");
        assert_eq!(
            out["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            "lookup_weather"
        );
    }

    #[test]
    fn translate_response_claude_thinking_signature_provenance_rejects_for_non_anthropic_clients() {
        let body = json!({
            "id": "msg_sig",
            "content": [{
                "type": "thinking",
                "thinking": "internal reasoning",
                "signature": "sig_123"
            }],
            "stop_reason": "end_turn",
            "model": "claude-3"
        });

        for client_format in [
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Google,
        ] {
            let err = translate_response(UpstreamFormat::Anthropic, client_format, &body)
                .expect_err("Anthropic thinking signature provenance should fail closed");
            assert!(err.contains("thinking"), "err = {err}");
            assert!(err.contains("signature"), "err = {err}");
        }
    }

    #[test]
    fn translate_response_claude_omitted_thinking_rejects_for_non_anthropic_clients() {
        let body = json!({
            "id": "msg_omitted",
            "content": [{
                "type": "thinking",
                "thinking": { "display": "omitted" },
                "signature": "sig_123"
            }],
            "stop_reason": "end_turn",
            "model": "claude-3"
        });

        for client_format in [
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Google,
        ] {
            let err = translate_response(UpstreamFormat::Anthropic, client_format, &body)
                .expect_err("Anthropic omitted thinking should fail closed");
            assert!(err.contains("thinking"), "err = {err}");
            assert!(err.contains("Anthropic"), "err = {err}");
        }
    }

    #[test]
    fn translate_response_claude_plain_thinking_without_provenance_still_translates() {
        let body = json!({
            "id": "msg_plain_thinking",
            "content": [
                { "type": "thinking", "thinking": "think" },
                { "type": "text", "text": "Hi" }
            ],
            "stop_reason": "end_turn",
            "model": "claude-3"
        });

        let out = translate_response(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .unwrap();

        assert_eq!(out["choices"][0]["message"]["reasoning_content"], "think");
        assert_eq!(out["choices"][0]["message"]["content"], "Hi");
    }
}
