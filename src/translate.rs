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

#[derive(Debug, Clone, PartialEq, Eq)]
enum NormalizedToolPolicy {
    Auto,
    None,
    Required,
    ForcedFunction(String),
}

#[derive(Debug, Clone, PartialEq)]
struct NormalizedJsonSchemaOutputShape {
    name: String,
    schema: Value,
    description: Option<String>,
    strict: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
enum NormalizedOutputShape {
    Text,
    JsonObject,
    JsonSchema(NormalizedJsonSchemaOutputShape),
}

#[derive(Debug, Clone, Default, PartialEq)]
struct NormalizedDecodingControls {
    stop: Option<Value>,
    seed: Option<Value>,
    presence_penalty: Option<Value>,
    frequency_penalty: Option<Value>,
    top_k: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
struct NormalizedLogprobsControls {
    enabled: bool,
    top_logprobs: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
struct NormalizedResponseLogprobCandidate {
    raw: Value,
    token: String,
    logprob: f64,
}

#[derive(Debug, Clone, PartialEq)]
struct NormalizedResponseTokenLogprob {
    raw: Value,
    token: String,
    logprob: f64,
    top_logprobs: Vec<NormalizedResponseLogprobCandidate>,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct NormalizedRequestControls {
    tool_policy: Option<NormalizedToolPolicy>,
    restricted_tool_names: Option<Vec<String>>,
    output_shape: Option<NormalizedOutputShape>,
    decoding: NormalizedDecodingControls,
    logprobs: Option<NormalizedLogprobsControls>,
    metadata: Option<Value>,
    user: Option<Value>,
    service_tier: Option<Value>,
    stream_include_obfuscation: Option<Value>,
    verbosity: Option<Value>,
    reasoning_effort: Option<Value>,
    prompt_cache_key: Option<Value>,
    prompt_cache_retention: Option<Value>,
    safety_identifier: Option<Value>,
    parallel_tool_calls: Option<Value>,
    store: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
struct NormalizedOpenAiFamilyFunctionTool {
    name: String,
    description: Option<Value>,
    parameters: Option<Value>,
    strict: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
struct NormalizedOpenAiFamilyCustomTool {
    name: String,
    description: Option<Value>,
    format: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
struct NormalizedOpenAiFamilyNamespaceTool {
    name: String,
}

#[derive(Debug, Clone, PartialEq)]
enum NormalizedOpenAiFamilyToolDef {
    Function(NormalizedOpenAiFamilyFunctionTool),
    Custom(NormalizedOpenAiFamilyCustomTool),
    Namespace(NormalizedOpenAiFamilyNamespaceTool),
}

#[derive(Debug, Clone, PartialEq)]
enum NormalizedOpenAiFamilyToolCall {
    Function {
        id: Option<Value>,
        name: String,
        arguments: String,
        namespace: Option<String>,
        proxied_tool_kind: Option<Value>,
    },
    Custom {
        id: Option<Value>,
        name: String,
        input: String,
        namespace: Option<String>,
        proxied_tool_kind: Option<Value>,
    },
}

#[derive(Debug, Clone, PartialEq)]
struct NormalizedOpenAiAudioContract {
    response_modalities: Vec<String>,
    voice_name: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct SharedControlProfile {
    metadata: bool,
    user: bool,
    service_tier: bool,
    stream_include_obfuscation: bool,
    verbosity: bool,
    reasoning_effort: bool,
    prompt_cache_key: bool,
    prompt_cache_retention: bool,
    safety_identifier: bool,
    top_logprobs: bool,
    parallel_tool_calls: bool,
    logit_bias: bool,
}

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

fn anthropic_request_tool_definition_not_portable_message(
    detail: &str,
    target_label: &str,
) -> String {
    format!(
        "Anthropic tool definitions with {detail} cannot be faithfully translated to {target_label}"
    )
}

fn anthropic_tool_result_order_not_portable_message(target_label: &str) -> String {
    format!(
        "Anthropic user turns that mix `tool_result` blocks with surrounding content cannot be faithfully translated to {target_label} without reordering blocks"
    )
}

fn gemini_function_response_parts_not_portable_message(target_label: &str) -> String {
    format!("Gemini functionResponse.parts cannot be faithfully translated to {target_label}")
}

fn openai_assistant_audio_not_portable_message(target_label: &str) -> String {
    format!("OpenAI assistant audio output cannot be faithfully translated to {target_label}")
}

fn openai_assistant_audio_field_not_portable_message(field: &str, target_label: &str) -> String {
    format!(
        "OpenAI assistant audio field `{field}` cannot be faithfully translated to {target_label}"
    )
}

fn openai_request_audio_not_portable_message(target_label: &str) -> String {
    format!("OpenAI Chat audio output intent cannot be faithfully translated to {target_label}")
}

fn openai_assistant_audio_history_not_portable_message(target_label: &str) -> String {
    format!("OpenAI assistant history field `messages[].audio` cannot be faithfully translated to {target_label}")
}

fn responses_multiple_output_audio_items_not_portable_message(target_label: &str) -> String {
    format!(
        "OpenAI Responses output has multiple `output_audio` items and cannot be faithfully translated to {target_label}"
    )
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
            Some("custom") | Some("custom_tool_call") | Some("custom_tool_call_output") => {
                SemanticToolKind::OpenAiCustom
            }
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

fn openai_tool_result_content_to_responses_output(content: Option<&Value>) -> Result<Value, String> {
    match content {
        None => Ok(Value::String(String::new())),
        Some(Value::Array(parts)) => {
            let mut output = Vec::with_capacity(parts.len());
            for part in parts {
                if part.get("type").and_then(Value::as_str) != Some("text") {
                    return Err(format!(
                        "OpenAI tool message content arrays can only contain text parts when translating to OpenAI Responses tool output; found `{}`.",
                        part.get("type")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                    ));
                }
                if part
                    .get("annotations")
                    .and_then(Value::as_array)
                    .is_some_and(|annotations| !annotations.is_empty())
                {
                    return Err(
                        "OpenAI tool message text-part annotations cannot be faithfully translated to OpenAI Responses tool output."
                            .to_string(),
                    );
                }
                output.push(serde_json::json!({
                    "type": "input_text",
                    "text": part.get("text").cloned().unwrap_or(Value::String(String::new()))
                }));
            }
            Ok(Value::Array(output))
        }
        Some(other) => Ok(other.clone()),
    }
}

fn responses_tool_output_to_openai_tool_content(
    output: Option<&Value>,
    target_format: UpstreamFormat,
) -> Result<Value, String> {
    match output {
        None => Ok(Value::String(String::new())),
        Some(Value::Array(items)) if target_format == UpstreamFormat::Google => {
            Ok(Value::Array(items.clone()))
        }
        Some(Value::Array(items)) => {
            let mut content = Vec::with_capacity(items.len());
            for item in items {
                match item.get("type").and_then(Value::as_str) {
                    Some("input_text") | Some("output_text") => content.push(serde_json::json!({
                        "type": "text",
                        "text": item.get("text").cloned().unwrap_or(Value::String(String::new()))
                    })),
                    Some(other) => {
                        return Err(format!(
                            "OpenAI Responses tool output arrays containing `{other}` cannot be faithfully translated to {}; only text arrays are portable.",
                            translation_target_label(target_format)
                        ))
                    }
                    None => {
                        return Err(format!(
                            "OpenAI Responses tool output arrays containing untyped values cannot be faithfully translated to {}.",
                            translation_target_label(target_format)
                        ))
                    }
                }
            }
            Ok(Value::Array(content))
        }
        Some(other) => Ok(other.clone()),
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
        text: part
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
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

fn openai_custom_tool_payload(value: &Value) -> Option<&serde_json::Map<String, Value>> {
    if value.get("type").and_then(Value::as_str) != Some("custom") {
        return None;
    }
    value
        .get("custom")
        .and_then(Value::as_object)
        .or_else(|| value.get("function").and_then(Value::as_object))
        .or_else(|| value.as_object().filter(|obj| obj.get("name").is_some()))
}

fn openai_custom_tool_name(value: &Value) -> Option<&str> {
    openai_custom_tool_payload(value)?
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
}

fn openai_custom_tool_input_raw(value: &Value) -> Option<&str> {
    let payload = openai_custom_tool_payload(value)?;
    payload
        .get("input")
        .or_else(|| payload.get("arguments"))
        .and_then(Value::as_str)
}

fn openai_tool_arguments_raw(tool_call: &Value) -> Option<&str> {
    tool_call
        .get("function")
        .and_then(|function| function.get("arguments"))
        .or_else(|| tool_call.get("arguments"))
        .and_then(Value::as_str)
}

fn openai_function_tool_payload(value: &Value) -> Option<&serde_json::Map<String, Value>> {
    if value.get("type").and_then(Value::as_str) != Some("function") {
        return None;
    }
    value
        .get("function")
        .and_then(Value::as_object)
        .or_else(|| value.as_object().filter(|obj| obj.get("name").is_some()))
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
        format!("tool arguments for {target_label} must be valid JSON; received `{raw}`: {error}")
    })?;
    if !value.is_object() {
        return Err(format!(
            "tool arguments for {target_label} must be a JSON object; received `{raw}`"
        ));
    }
    Ok(value)
}

fn normalized_openai_tool_definition(
    tool: &Value,
) -> Result<Option<NormalizedOpenAiFamilyToolDef>, String> {
    match tool.get("type").and_then(Value::as_str) {
        Some("function") => {
            let payload = openai_function_tool_payload(tool)
                .ok_or("OpenAI function tools require a `function` payload.".to_string())?;
            let name = payload
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .ok_or("OpenAI function tools require a non-empty function name.".to_string())?;
            Ok(Some(NormalizedOpenAiFamilyToolDef::Function(
                NormalizedOpenAiFamilyFunctionTool {
                    name: name.to_string(),
                    description: payload.get("description").cloned(),
                    parameters: payload.get("parameters").cloned(),
                    strict: payload.get("strict").cloned(),
                },
            )))
        }
        Some("custom") => {
            let payload = openai_custom_tool_payload(tool)
                .ok_or("OpenAI custom tools require a `custom` payload.".to_string())?;
            let name = payload
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .ok_or("OpenAI custom tools require a non-empty custom name.".to_string())?;
            Ok(Some(NormalizedOpenAiFamilyToolDef::Custom(
                NormalizedOpenAiFamilyCustomTool {
                    name: name.to_string(),
                    description: payload.get("description").cloned(),
                    format: payload.get("format").cloned(),
                },
            )))
        }
        _ => Ok(None),
    }
}

fn normalized_responses_tool_definition(
    tool: &Value,
) -> Result<Option<NormalizedOpenAiFamilyToolDef>, String> {
    match tool.get("type").and_then(Value::as_str) {
        Some("function") => {
            let name = tool
                .get("name")
                .or_else(|| tool.get("function").and_then(|function| function.get("name")))
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .ok_or("OpenAI Responses function tools require a non-empty name.".to_string())?;
            Ok(Some(NormalizedOpenAiFamilyToolDef::Function(
                NormalizedOpenAiFamilyFunctionTool {
                    name: name.to_string(),
                    description: tool.get("description").cloned(),
                    parameters: tool.get("parameters").cloned(),
                    strict: tool.get("strict").cloned(),
                },
            )))
        }
        Some("custom") => {
            let name = tool
                .get("name")
                .or_else(|| tool.get("custom").and_then(|custom| custom.get("name")))
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .ok_or("OpenAI Responses custom tools require a non-empty name.".to_string())?;
            Ok(Some(NormalizedOpenAiFamilyToolDef::Custom(
                NormalizedOpenAiFamilyCustomTool {
                    name: name.to_string(),
                    description: tool.get("description").cloned(),
                    format: tool.get("format").cloned(),
                },
            )))
        }
        Some("namespace") => {
            let name = tool
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .ok_or("OpenAI Responses namespace tools require a non-empty name.".to_string())?;
            Ok(Some(NormalizedOpenAiFamilyToolDef::Namespace(
                NormalizedOpenAiFamilyNamespaceTool {
                    name: name.to_string(),
                },
            )))
        }
        _ => Ok(None),
    }
}

fn normalized_openai_tool_definitions_from_request(
    body: &Value,
) -> Result<Vec<NormalizedOpenAiFamilyToolDef>, String> {
    body.get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools.iter().try_fold(Vec::new(), |mut normalized, tool| {
                if let Some(tool) = normalized_openai_tool_definition(tool)? {
                    normalized.push(tool);
                }
                Ok(normalized)
            })
        })
        .unwrap_or_else(|| Ok(Vec::new()))
}

fn normalized_responses_tool_definitions_from_request(
    body: &Value,
) -> Result<Vec<NormalizedOpenAiFamilyToolDef>, String> {
    body.get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools.iter().try_fold(Vec::new(), |mut normalized, tool| {
                if let Some(tool) = normalized_responses_tool_definition(tool)? {
                    normalized.push(tool);
                }
                Ok(normalized)
            })
        })
        .unwrap_or_else(|| Ok(Vec::new()))
}

fn normalized_tool_definition_to_openai(
    tool: &NormalizedOpenAiFamilyToolDef,
) -> Result<Value, String> {
    match tool {
        NormalizedOpenAiFamilyToolDef::Function(function) => {
            let mut payload = serde_json::Map::new();
            payload.insert("name".to_string(), Value::String(function.name.clone()));
            if let Some(description) = function.description.clone() {
                payload.insert("description".to_string(), description);
            }
            if let Some(parameters) = function.parameters.clone() {
                payload.insert("parameters".to_string(), parameters);
            }
            if let Some(strict) = function.strict.clone() {
                payload.insert("strict".to_string(), strict);
            }
            Ok(serde_json::json!({
                "type": "function",
                "function": Value::Object(payload)
            }))
        }
        NormalizedOpenAiFamilyToolDef::Custom(custom) => {
            let mut payload = serde_json::Map::new();
            payload.insert("name".to_string(), Value::String(custom.name.clone()));
            if let Some(description) = custom.description.clone() {
                payload.insert("description".to_string(), description);
            }
            if let Some(format) = custom.format.clone() {
                payload.insert("format".to_string(), format);
            }
            Ok(serde_json::json!({
                "type": "custom",
                "custom": Value::Object(payload)
            }))
        }
        NormalizedOpenAiFamilyToolDef::Namespace(namespace) => Err(format!(
            "OpenAI Responses namespace tool `{}` cannot be faithfully translated to OpenAI Chat Completions",
            namespace.name
        )),
    }
}

fn normalized_tool_definition_to_responses(tool: &NormalizedOpenAiFamilyToolDef) -> Value {
    match tool {
        NormalizedOpenAiFamilyToolDef::Function(function) => {
            let mut payload = serde_json::Map::new();
            payload.insert("type".to_string(), Value::String("function".to_string()));
            payload.insert("name".to_string(), Value::String(function.name.clone()));
            if let Some(description) = function.description.clone() {
                payload.insert("description".to_string(), description);
            }
            if let Some(parameters) = function.parameters.clone() {
                payload.insert("parameters".to_string(), parameters);
            }
            if let Some(strict) = function.strict.clone() {
                payload.insert("strict".to_string(), strict);
            }
            Value::Object(payload)
        }
        NormalizedOpenAiFamilyToolDef::Custom(custom) => {
            let mut payload = serde_json::Map::new();
            payload.insert("type".to_string(), Value::String("custom".to_string()));
            payload.insert("name".to_string(), Value::String(custom.name.clone()));
            if let Some(description) = custom.description.clone() {
                payload.insert("description".to_string(), description);
            }
            if let Some(format) = custom.format.clone() {
                payload.insert("format".to_string(), format);
            }
            Value::Object(payload)
        }
        NormalizedOpenAiFamilyToolDef::Namespace(namespace) => serde_json::json!({
            "type": "namespace",
            "name": namespace.name
        }),
    }
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

fn normalized_openai_tool_call(
    tool_call: &Value,
) -> Result<Option<NormalizedOpenAiFamilyToolCall>, String> {
    let Some(tool_type) = tool_call.get("type").and_then(Value::as_str) else {
        return Ok(None);
    };
    match tool_type {
        "function" => {
            let Some(function) = tool_call.get("function").and_then(Value::as_object) else {
                return Err(
                    "OpenAI function tool calls require a `function` payload.".to_string(),
                );
            };
            let name = function
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .ok_or("OpenAI function tool calls require a non-empty function name.".to_string())?;
            Ok(Some(NormalizedOpenAiFamilyToolCall::Function {
                id: tool_call.get("id").cloned(),
                name: name.to_string(),
                arguments: openai_tool_arguments_raw(tool_call).unwrap_or("{}").to_string(),
                namespace: None,
                proxied_tool_kind: tool_call.get("proxied_tool_kind").cloned(),
            }))
        }
        "custom" => {
            let name = openai_custom_tool_name(tool_call).ok_or(
                "OpenAI custom tool calls require a `custom.name` field.".to_string(),
            )?;
            Ok(Some(NormalizedOpenAiFamilyToolCall::Custom {
                id: tool_call.get("id").cloned(),
                name: name.to_string(),
                input: openai_custom_tool_input_raw(tool_call).unwrap_or("").to_string(),
                namespace: None,
                proxied_tool_kind: tool_call.get("proxied_tool_kind").cloned(),
            }))
        }
        _ => Ok(None),
    }
}

fn normalized_responses_tool_call(
    item: &Value,
) -> Result<Option<NormalizedOpenAiFamilyToolCall>, String> {
    match item.get("type").and_then(Value::as_str) {
        Some("function_call") | Some("custom_tool_call") => {}
        _ => return Ok(None),
    }

    let name = item
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .ok_or("OpenAI Responses tool calls require a non-empty name.".to_string())?;
    let namespace = item
        .get("namespace")
        .and_then(Value::as_str)
        .map(str::to_string);
    let proxied_tool_kind = item.get("proxied_tool_kind").cloned();
    Ok(Some(match semantic_tool_kind_from_value(item) {
        SemanticToolKind::OpenAiCustom => NormalizedOpenAiFamilyToolCall::Custom {
            id: item.get("call_id").cloned(),
            name: name.to_string(),
            input: responses_tool_call_input_raw(item).unwrap_or("").to_string(),
            namespace,
            proxied_tool_kind,
        },
        _ => NormalizedOpenAiFamilyToolCall::Function {
            id: item.get("call_id").cloned(),
            name: name.to_string(),
            arguments: responses_tool_call_input_raw(item).unwrap_or("{}").to_string(),
            namespace,
            proxied_tool_kind,
        },
    }))
}

fn normalized_tool_call_to_openai(call: &NormalizedOpenAiFamilyToolCall) -> Value {
    match call {
        NormalizedOpenAiFamilyToolCall::Function {
            id,
            name,
            arguments,
            proxied_tool_kind,
            ..
        } => {
            let mut tool_call = serde_json::json!({
                "id": id,
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": arguments
                }
            });
            if let Some(proxied_tool_kind) = proxied_tool_kind.clone() {
                tool_call["proxied_tool_kind"] = proxied_tool_kind;
            }
            tool_call
        }
        NormalizedOpenAiFamilyToolCall::Custom {
            id,
            name,
            input,
            proxied_tool_kind,
            ..
        } => {
            let mut tool_call = serde_json::json!({
                "id": id,
                "type": "custom",
                "custom": {
                    "name": name,
                    "input": input
                }
            });
            if let Some(proxied_tool_kind) = proxied_tool_kind.clone() {
                tool_call["proxied_tool_kind"] = proxied_tool_kind;
            }
            tool_call
        }
    }
}

fn responses_tool_call_item_to_openai_tool_call(item: &Value) -> Option<Value> {
    normalized_responses_tool_call(item)
        .ok()
        .flatten()
        .map(|tool_call| normalized_tool_call_to_openai(&tool_call))
}

fn responses_tool_call_item_to_openai_tool_call_strict(
    item: &Value,
    target_label: &str,
) -> Result<Option<Value>, String> {
    let Some(tool_call) = normalized_responses_tool_call(item)? else {
        return Ok(None);
    };
    match &tool_call {
        NormalizedOpenAiFamilyToolCall::Function {
            name,
            namespace: Some(_),
            ..
        }
        | NormalizedOpenAiFamilyToolCall::Custom {
            name,
            namespace: Some(_),
            ..
        } => Err(format!(
            "OpenAI Responses namespaced tool call `{name}` cannot be faithfully translated to {target_label}"
        )),
        _ => Ok(Some(normalized_tool_call_to_openai(&tool_call))),
    }
}

fn openai_tool_call_to_responses_item(tool_call: &Value) -> Value {
    normalized_openai_tool_call(tool_call)
        .ok()
        .flatten()
        .map(|call| match call {
            NormalizedOpenAiFamilyToolCall::Function {
                id,
                name,
                arguments,
                proxied_tool_kind,
                ..
            } => {
                let mut item = serde_json::json!({
                    "type": "function_call",
                    "call_id": id,
                    "name": name,
                    "arguments": arguments
                });
                if let Some(proxied_tool_kind) = proxied_tool_kind {
                    item["proxied_tool_kind"] = proxied_tool_kind;
                }
                item
            }
            NormalizedOpenAiFamilyToolCall::Custom {
                id,
                name,
                input,
                proxied_tool_kind,
                ..
            } => {
                let mut item = serde_json::json!({
                    "type": "custom_tool_call",
                    "call_id": id,
                    "name": name,
                    "input": input
                });
                if let Some(proxied_tool_kind) = proxied_tool_kind {
                    item["proxied_tool_kind"] = proxied_tool_kind;
                }
                item
            }
        })
        .unwrap_or_else(|| serde_json::json!({
            "type": "function_call",
            "call_id": tool_call.get("id"),
            "name": tool_call.get("function").and_then(|f| f.get("name")),
            "arguments": openai_tool_arguments_raw(tool_call).unwrap_or("{}")
        }))
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

fn gemini_function_response_has_nonportable_parts(function_response: &Value) -> bool {
    function_response
        .get("parts")
        .or_else(|| {
            gemini_nested_field(function_response, "response", "response")
                .and_then(|response| response.get("parts"))
        })
        .or_else(|| {
            gemini_nested_field(function_response, "response", "response")
                .and_then(|response| response.get("result"))
                .and_then(|result| result.get("parts"))
        })
        .map(|parts| !parts.is_null())
        .unwrap_or(false)
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

fn responses_portable_output_item_type(item_type: &str) -> bool {
    matches!(
        item_type,
        "message" | "function_call" | "custom_tool_call" | "reasoning" | "output_audio"
    )
}

fn responses_hosted_output_item_type(item_type: &str) -> bool {
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

fn responses_nonportable_output_item_message(item: &Value, target_label: &str) -> Option<String> {
    let item_type = item.get("type").and_then(Value::as_str)?;
    match item_type {
        "reasoning" if item.get("encrypted_content").is_some() => Some(format!(
            "OpenAI Responses reasoning output item field `encrypted_content` cannot be faithfully translated to {target_label}"
        )),
        "function_call_output" | "custom_tool_call_output" => Some(format!(
            "OpenAI Responses output item `{item_type}` cannot be faithfully translated to {target_label}"
        )),
        "compaction" => Some(format!(
            "OpenAI Responses output item `compaction` cannot be faithfully translated to {target_label}"
        )),
        _ if responses_portable_output_item_type(item_type) => None,
        _ if responses_hosted_output_item_type(item_type) => Some(format!(
            "OpenAI Responses output item `{item_type}` cannot be faithfully translated to {target_label}"
        )),
        _ => Some(format!(
            "OpenAI Responses output item `{item_type}` is outside the portable cross-protocol subset and cannot be faithfully translated to {target_label}"
        )),
    }
}

fn responses_output_text_logprobs(item: &Value) -> Result<Option<Vec<Value>>, String> {
    let Some(parts) = item.get("content").and_then(Value::as_array) else {
        return Ok(None);
    };

    let mut saw_logprobs = false;
    let mut content_logprobs = Vec::new();
    for part in parts {
        if part.get("type").and_then(Value::as_str) != Some("output_text") {
            continue;
        }
        match part.get("logprobs") {
            Some(Value::Array(logprobs)) => {
                saw_logprobs = true;
                content_logprobs.extend(logprobs.iter().cloned());
            }
            Some(Value::Null) | None => {}
            Some(_) => {
                return Err(
                    "OpenAI Responses message.output_text.logprobs must be an array for response translation."
                        .to_string(),
                )
            }
        }
    }

    Ok(saw_logprobs.then_some(content_logprobs))
}

fn normalized_response_logprob_candidate_from_value(
    value: &Value,
    field_name: &str,
    target_label: &str,
) -> Result<NormalizedResponseLogprobCandidate, String> {
    let Some(obj) = value.as_object() else {
        return Err(format!(
            "OpenAI-family response field `{field_name}` must contain objects when translating to {target_label}"
        ));
    };
    let token = obj
        .get("token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            format!(
                "OpenAI-family response field `{field_name}` must contain non-empty string `token` values when translating to {target_label}"
            )
        })?
        .to_string();
    let logprob = obj
        .get("logprob")
        .and_then(Value::as_f64)
        .filter(|logprob| logprob.is_finite())
        .ok_or_else(|| {
            format!(
                "OpenAI-family response field `{field_name}` must contain finite numeric `logprob` values when translating to {target_label}"
            )
        })?;
    Ok(NormalizedResponseLogprobCandidate {
        raw: value.clone(),
        token,
        logprob,
    })
}

fn normalized_response_token_logprob_from_value(
    value: &Value,
    target_label: &str,
) -> Result<NormalizedResponseTokenLogprob, String> {
    let candidate = normalized_response_logprob_candidate_from_value(
        value,
        "choice.logprobs.content",
        target_label,
    )?;
    let top_logprobs = match value.get("top_logprobs") {
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                normalized_response_logprob_candidate_from_value(
                    item,
                    "choice.logprobs.content[].top_logprobs",
                    target_label,
                )
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(Value::Null) | None => Vec::new(),
        Some(_) => {
            return Err(format!(
                "OpenAI Chat response field `choice.logprobs.content[].top_logprobs` must be an array when translating to {target_label}"
            ))
        }
    };
    Ok(NormalizedResponseTokenLogprob {
        raw: value.clone(),
        token: candidate.token,
        logprob: candidate.logprob,
        top_logprobs,
    })
}

fn normalized_response_logprobs_from_openai_choice(
    choice: &Value,
    target_label: &str,
) -> Result<Option<Vec<NormalizedResponseTokenLogprob>>, String> {
    let Some(logprobs) = choice.get("logprobs").filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let Some(logprobs) = logprobs.as_object() else {
        return Err(
            format!(
                "OpenAI Chat response field `choice.logprobs` must be an object when translating to {target_label}"
            ),
        );
    };
    if logprobs
        .get("refusal")
        .and_then(Value::as_array)
        .is_some_and(|refusal| !refusal.is_empty())
    {
        return Err(format!(
            "OpenAI Chat response field `choice.logprobs.refusal` cannot be faithfully translated to {target_label}"
        ));
    }
    match logprobs.get("content") {
        Some(Value::Array(content)) => content
            .iter()
            .map(|item| normalized_response_token_logprob_from_value(item, target_label))
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(format!(
            "OpenAI Chat response field `choice.logprobs.content` must be an array when translating to {target_label}"
        )),
    }
}

fn normalized_response_logprob_candidate_from_gemini_value(
    value: &Value,
    field_name: &str,
    target_label: &str,
) -> Result<NormalizedResponseLogprobCandidate, String> {
    let Some(obj) = value.as_object() else {
        return Err(format!(
            "Gemini response field `{field_name}` must contain objects when translating to {target_label}"
        ));
    };
    let token = obj
        .get("token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            format!(
                "Gemini response field `{field_name}` must contain non-empty string `token` values when translating to {target_label}"
            )
        })?
        .to_string();
    let logprob = obj
        .get("logProbability")
        .and_then(Value::as_f64)
        .filter(|logprob| logprob.is_finite())
        .ok_or_else(|| {
            format!(
                "Gemini response field `{field_name}` must contain finite numeric `logProbability` values when translating to {target_label}"
            )
        })?;
    Ok(NormalizedResponseLogprobCandidate {
        raw: serde_json::json!({
            "token": token,
            "logprob": logprob
        }),
        token,
        logprob,
    })
}

fn normalized_response_logprobs_from_gemini_candidate(
    candidate: &Value,
    target_label: &str,
) -> Result<Option<Vec<NormalizedResponseTokenLogprob>>, String> {
    let avg_logprobs = match candidate.get("avgLogprobs").filter(|value| !value.is_null()) {
        Some(value) => Some(
            value
                .as_f64()
                .filter(|logprob| logprob.is_finite())
                .ok_or_else(|| {
                    format!(
                        "Gemini response field `avgLogprobs` must be a finite number when translating to {target_label}"
                    )
                })?,
        ),
        None => None,
    };
    let Some(logprobs_result) = candidate.get("logprobsResult").filter(|value| !value.is_null())
    else {
        if avg_logprobs.is_some() {
            return Err(format!(
                "Gemini response field `avgLogprobs` cannot be faithfully translated to {target_label} without token-level `logprobsResult`"
            ));
        }
        return Ok(None);
    };
    let Some(logprobs_result) = logprobs_result.as_object() else {
        return Err(format!(
            "Gemini response field `logprobsResult` must be an object when translating to {target_label}"
        ));
    };
    let chosen_candidates = logprobs_result
        .get("chosenCandidates")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            format!(
                "Gemini response field `logprobsResult.chosenCandidates` must be an array when translating to {target_label}"
            )
        })?;
    let top_candidates = match logprobs_result.get("topCandidates") {
        Some(Value::Array(items)) => Some(items),
        Some(Value::Null) | None => None,
        Some(_) => {
            return Err(format!(
                "Gemini response field `logprobsResult.topCandidates` must be an array when translating to {target_label}"
            ))
        }
    };
    if let Some(top_candidates) = top_candidates {
        if top_candidates.len() != chosen_candidates.len() {
            return Err(format!(
                "Gemini response fields `logprobsResult.chosenCandidates` and `logprobsResult.topCandidates` must have the same length when translating to {target_label}"
            ));
        }
    }

    chosen_candidates
        .iter()
        .enumerate()
        .map(|(idx, chosen_candidate)| {
            let chosen = normalized_response_logprob_candidate_from_gemini_value(
                chosen_candidate,
                "logprobsResult.chosenCandidates",
                target_label,
            )?;
            let top_logprobs = if let Some(top_candidates) = top_candidates {
                let top_candidates_for_step = top_candidates
                    .get(idx)
                    .and_then(|step| step.get("candidates"))
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        format!(
                            "Gemini response field `logprobsResult.topCandidates[].candidates` must be an array when translating to {target_label}"
                        )
                    })?;
                top_candidates_for_step
                    .iter()
                    .map(|top_candidate| {
                        normalized_response_logprob_candidate_from_gemini_value(
                            top_candidate,
                            "logprobsResult.topCandidates[].candidates",
                            target_label,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                Vec::new()
            };
            Ok(NormalizedResponseTokenLogprob {
                raw: serde_json::json!({
                    "token": chosen.token,
                    "logprob": chosen.logprob,
                    "top_logprobs": top_logprobs
                        .iter()
                        .map(|candidate| candidate.raw.clone())
                        .collect::<Vec<_>>()
                }),
                token: chosen.token,
                logprob: chosen.logprob,
                top_logprobs,
            })
        })
        .collect::<Result<Vec<_>, String>>()
        .map(Some)
}

fn normalized_response_logprobs_to_openai_values(
    content_logprobs: &[NormalizedResponseTokenLogprob],
) -> Vec<Value> {
    content_logprobs
        .iter()
        .map(|item| item.raw.clone())
        .collect::<Vec<_>>()
}

fn attach_openai_choice_logprobs_to_responses_content(
    content: &mut [Value],
    content_logprobs: &[NormalizedResponseTokenLogprob],
) -> Result<(), String> {
    let output_text_indexes = content
        .iter()
        .enumerate()
        .filter_map(|(idx, part)| {
            (part.get("type").and_then(Value::as_str) == Some("output_text")).then_some(idx)
        })
        .collect::<Vec<_>>();
    let [output_text_index] = output_text_indexes.as_slice() else {
        return Err(
            "OpenAI Chat response logprobs can only be translated to Responses when assistant output maps to a single `output_text` item."
                .to_string(),
        );
    };
    content[*output_text_index]["logprobs"] =
        Value::Array(normalized_response_logprobs_to_openai_values(content_logprobs));
    Ok(())
}

fn normalized_response_logprob_candidate_to_gemini(
    candidate: &NormalizedResponseLogprobCandidate,
) -> Value {
    serde_json::json!({
        "token": candidate.token,
        "logProbability": candidate.logprob
    })
}

fn normalized_response_logprobs_to_gemini_fields(
    content_logprobs: &[NormalizedResponseTokenLogprob],
) -> (Option<Value>, Value) {
    let log_probability_sum = content_logprobs.iter().map(|item| item.logprob).sum::<f64>();
    let avg_logprobs = (!content_logprobs.is_empty()).then(|| {
        serde_json::json!(log_probability_sum / content_logprobs.len() as f64)
    });
    let logprobs_result = serde_json::json!({
        "chosenCandidates": content_logprobs
            .iter()
            .map(|item| serde_json::json!({
                "token": item.token,
                "logProbability": item.logprob
            }))
            .collect::<Vec<_>>(),
        "topCandidates": content_logprobs
            .iter()
            .map(|item| serde_json::json!({
                "candidates": item
                    .top_logprobs
                    .iter()
                    .map(normalized_response_logprob_candidate_to_gemini)
                    .collect::<Vec<_>>()
            }))
            .collect::<Vec<_>>(),
        "logProbabilitySum": log_probability_sum
    });
    (avg_logprobs, logprobs_result)
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

fn openai_response_has_assistant_audio(body: &Value) -> bool {
    body.get("choices")
        .and_then(Value::as_array)
        .map(|choices| {
            choices.iter().any(|choice| {
                choice
                    .get("message")
                    .and_then(|message| message.get("audio"))
                    .map(|audio| !audio.is_null())
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn clone_usage_details_object(details: Option<&Value>) -> Option<Value> {
    let details = details?.as_object()?;
    (!details.is_empty()).then(|| Value::Object(details.clone()))
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

fn copy_remaining_usage_fields(source: &Value, target: &mut Value, consumed_fields: &[&str]) {
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
        target_map
            .entry(key.clone())
            .or_insert_with(|| value.clone());
    }
}

fn responses_stateful_request_controls_for_translate(body: &Value) -> Vec<&'static str> {
    let mut controls = Vec::new();
    for field in [
        "previous_response_id",
        "conversation",
        "background",
        "prompt",
    ] {
        if body.get(field).is_some() {
            controls.push(field);
        }
    }
    controls
}

fn cross_protocol_store_warning_message(
    client_format: UpstreamFormat,
    upstream_format: UpstreamFormat,
) -> String {
    format!(
        "{} request field `store` has provider-specific persistence semantics and will be dropped when translating to {}",
        translation_target_label(client_format),
        translation_target_label(upstream_format)
    )
}

fn gemini_top_k_warning_message(upstream_format: UpstreamFormat) -> String {
    format!(
        "Gemini generationConfig.topK has no direct equivalent in {} and will be dropped",
        translation_target_label(upstream_format)
    )
}

fn openai_parallel_tool_calls_to_gemini_warning_message(client_format: UpstreamFormat) -> String {
    format!(
        "{} field `parallel_tool_calls=false` has no direct Gemini equivalent and will be dropped",
        translation_target_label(client_format)
    )
}

fn shared_control_profile_for_target(target_format: UpstreamFormat) -> SharedControlProfile {
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
        UpstreamFormat::Google => SharedControlProfile {
            top_logprobs: true,
            ..SharedControlProfile::default()
        },
    }
}

fn request_stream_include_obfuscation(body: &Value) -> Option<Value> {
    body.get("stream_options")
        .and_then(Value::as_object)
        .and_then(|stream_options| stream_options.get("include_obfuscation"))
        .cloned()
}

fn openai_normalized_logprobs_controls(body: &Value) -> Option<NormalizedLogprobsControls> {
    let enabled = body.get("logprobs").and_then(Value::as_bool) == Some(true);
    let top_logprobs = body.get("top_logprobs").cloned();
    (enabled || top_logprobs.is_some()).then_some(NormalizedLogprobsControls {
        enabled,
        top_logprobs,
    })
}

fn responses_normalized_logprobs_controls(body: &Value) -> Option<NormalizedLogprobsControls> {
    let enabled = responses_include_requests_output_text_logprobs(body);
    let top_logprobs = body.get("top_logprobs").cloned();
    (enabled || top_logprobs.is_some()).then_some(NormalizedLogprobsControls {
        enabled,
        top_logprobs,
    })
}

fn gemini_normalized_logprobs_controls(body: &Value) -> Option<NormalizedLogprobsControls> {
    let enabled = gemini_generation_config_field(body, "responseLogprobs", "response_logprobs")
        .and_then(Value::as_bool)
        == Some(true);
    let top_logprobs = gemini_generation_config_field(body, "logprobs", "logprobs").cloned();
    (enabled || top_logprobs.is_some()).then_some(NormalizedLogprobsControls {
        enabled,
        top_logprobs,
    })
}

fn normalized_openai_audio_contract(
    body: &Value,
) -> Result<Option<NormalizedOpenAiAudioContract>, String> {
    let modalities = body
        .get("modalities")
        .and_then(Value::as_array)
        .map(|items| {
            items.iter()
                .filter_map(Value::as_str)
                .map(|item| item.trim().to_ascii_lowercase())
                .filter(|item| !item.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let requests_audio = modalities.iter().any(|item| item == "audio") || body.get("audio").is_some();
    if !requests_audio {
        return Ok(None);
    }

    let audio = body
        .get("audio")
        .and_then(Value::as_object)
        .ok_or("OpenAI Chat audio output requests require a top-level `audio` object.".to_string())?;
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
            "OpenAI Chat audio field `audio.format` value `{format}` cannot be faithfully translated to Gemini because Gemini request speechConfig has no documented output encoding field."
        ));
    }
    let voice_name = match audio.get("voice") {
        Some(Value::String(voice)) if !voice.trim().is_empty() => Some(voice.clone()),
        Some(Value::Object(voice)) => {
            let id = voice.get("id").and_then(Value::as_str).unwrap_or("");
            return Err(format!(
                "OpenAI Chat audio voice `{id}` cannot be faithfully translated because Gemini only documents prebuilt voice names in speechConfig."
            ));
        }
        Some(_) => {
            return Err(
                "OpenAI Chat audio voice must be a non-empty string to translate to Gemini."
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

fn openai_assistant_history_audio_present(body: &Value) -> bool {
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

fn responses_include_items(body: &Value) -> Vec<&str> {
    body.get("include")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

fn responses_include_requests_output_text_logprobs(body: &Value) -> bool {
    responses_include_items(body).contains(&"message.output_text.logprobs")
}

fn responses_include_has_nonportable_items(
    body: &Value,
    target_format: UpstreamFormat,
) -> bool {
    let include_items = responses_include_items(body);
    if include_items.is_empty() {
        return body.get("include").is_some();
    }

    include_items.iter().any(|item| {
        !matches!(
            (target_format, *item),
            (
                UpstreamFormat::OpenAiCompletion
                    | UpstreamFormat::OpenAiResponses
                    | UpstreamFormat::Google,
                "message.output_text.logprobs"
            )
        )
    })
}

fn responses_text_verbosity(body: &Value) -> Option<Value> {
    body.get("text")
        .and_then(Value::as_object)
        .and_then(|text| text.get("verbosity"))
        .cloned()
}

fn responses_reasoning_effort(body: &Value) -> Option<Value> {
    body.get("reasoning")
        .and_then(Value::as_object)
        .and_then(|reasoning| reasoning.get("effort"))
        .cloned()
}

fn object_has_only_keys(
    object: &serde_json::Map<String, Value>,
    allowed_keys: &[&str],
) -> bool {
    object.keys().all(|key| allowed_keys.contains(&key.as_str()))
}

fn responses_text_has_nonportable_fields(body: &Value, profile: SharedControlProfile) -> bool {
    let Some(text) = body.get("text").and_then(Value::as_object) else {
        return false;
    };
    let mut allowed_keys = vec!["format"];
    if profile.verbosity {
        allowed_keys.push("verbosity");
    }
    !object_has_only_keys(text, &allowed_keys)
}

fn responses_reasoning_has_nonportable_fields(
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

fn openai_to_responses_dropped_control_names(body: &Value) -> Vec<&'static str> {
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

fn openai_to_anthropic_dropped_control_names(body: &Value) -> Vec<&'static str> {
    let mut controls = Vec::new();
    for field in ["seed", "presence_penalty", "frequency_penalty"] {
        if body.get(field).is_some() {
            controls.push(field);
        }
    }
    controls
}

fn openai_warning_only_request_controls_for_translate(
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

fn gemini_warning_only_request_controls_for_translate(
    body: &Value,
    target_format: UpstreamFormat,
) -> Vec<String> {
    let profile = shared_control_profile_for_target(target_format);
    let mut controls = Vec::new();
    if let Some(logprobs) = gemini_normalized_logprobs_controls(body) {
        if logprobs.enabled && !profile.top_logprobs {
            controls.push("responseLogprobs".to_string());
        }
        if logprobs.top_logprobs.is_some() && !profile.top_logprobs {
            controls.push("logprobs".to_string());
        }
    }
    controls
}

fn responses_warning_only_request_controls_for_translate(
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
    if body.get("context_management").is_some() {
        controls.push("context_management".to_string());
    }
    controls
}

fn responses_tool_choice_allowed_tools_array(
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

fn openai_named_tool_choice_name<'a>(
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

fn openai_tool_choice_contains_custom(value: &Value) -> bool {
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
            tools.map(|tools| {
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

fn responses_nonportable_tool_choice_message(
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
            UpstreamFormat::OpenAiCompletion => None,
            _ => Some(format!(
                "OpenAI Responses tool_choice.type `custom` cannot be faithfully translated to {target_label}"
            )),
        },
        "allowed_tools" => responses_tool_choice_allowed_tools_array(tool_choice).and_then(
            |tools| {
                tools.iter().find_map(|tool| match tool.get("type").and_then(Value::as_str) {
                    Some("function") => None,
                    Some("custom") if target_format == UpstreamFormat::OpenAiCompletion => None,
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

fn responses_nonportable_tool_definition_message(body: &Value, target_label: &str) -> Option<String> {
    let tools = body.get("tools").and_then(Value::as_array)?;
    tools.iter().find_map(|tool| {
        match normalized_responses_tool_definition(tool) {
            Ok(Some(NormalizedOpenAiFamilyToolDef::Namespace(namespace))) => Some(format!(
                "OpenAI Responses namespace tool `{}` cannot be faithfully translated to {target_label}",
                namespace.name
            )),
            Err(message) => Some(message),
            _ => None,
        }
    })
}

fn responses_has_warning_only_nonportable_tool_definitions(body: &Value) -> bool {
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

fn responses_hosted_input_item_type(item_type: &str) -> bool {
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

fn responses_portable_input_item_type(item_type: &str) -> bool {
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

fn responses_nonportable_input_item_message(body: &Value, target_label: &str) -> Option<String> {
    let items = body.get("input").and_then(Value::as_array)?;
    items.iter().find_map(|item| {
        let item_type = item
            .get("type")
            .and_then(Value::as_str)
            .or_else(|| item.get("role").and_then(Value::as_str).map(|_| "message"))?;
        if item_type == "reasoning" && item.get("encrypted_content").is_some() {
            return Some(format!(
                "OpenAI Responses reasoning item field `encrypted_content` cannot be faithfully translated to {target_label}"
            ));
        }
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

fn cross_protocol_requested_choice_count(
    client_format: UpstreamFormat,
    body: &Value,
) -> Option<(&'static str, u64)> {
    match client_format {
        UpstreamFormat::OpenAiCompletion => body.get("n").and_then(Value::as_u64).map(|n| ("n", n)),
        UpstreamFormat::Google => {
            let generation_config = body
                .get("generationConfig")
                .or_else(|| body.get("generation_config"));
            generation_config
                .and_then(|config| {
                    config
                        .get("candidateCount")
                        .or_else(|| config.get("candidate_count"))
                })
                .or_else(|| {
                    body.get("candidateCount")
                        .or_else(|| body.get("candidate_count"))
                })
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
                    message
                        .get("tool_calls")
                        .and_then(Value::as_array)
                        .and_then(|tool_calls| {
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
                            (semantic_tool_kind_from_value(item) != SemanticToolKind::OpenAiCustom)
                                .then(|| {
                                    responses_tool_call_to_structured_value(item, target_label)
                                        .err()
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
        if let Some(message) = responses_nonportable_tool_choice_message(
            body,
            upstream_format,
        ) {
            assessment.reject(message);
        }
        if let Some(message) = responses_nonportable_input_item_message(
            body,
            translation_target_label(upstream_format),
        ) {
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
    }

    if upstream_format == UpstreamFormat::Google
        && body.get("parallel_tool_calls").and_then(Value::as_bool) == Some(false)
    {
        assessment.warning(openai_parallel_tool_calls_to_gemini_warning_message(
            client_format,
        ));
    }

    if body.get("store").is_some() {
        assessment.warning(cross_protocol_store_warning_message(
            client_format,
            upstream_format,
        ));
    }

    if client_format == UpstreamFormat::OpenAiCompletion
        && upstream_format == UpstreamFormat::OpenAiResponses
    {
        if let Some(message) = normalized_openai_audio_contract(body)
            .err()
            .or_else(|| {
                normalized_openai_audio_contract(body)
                    .ok()
                    .flatten()
                    .map(|_| openai_request_audio_not_portable_message("OpenAI Responses"))
            })
        {
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
        if let Some(message) = normalized_openai_audio_contract(body)
            .err()
            .or_else(|| {
                normalized_openai_audio_contract(body)
                    .ok()
                    .flatten()
                    .map(|_| openai_request_audio_not_portable_message("Anthropic"))
            })
        {
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

    if client_format == UpstreamFormat::OpenAiCompletion && upstream_format == UpstreamFormat::Google
    {
        if let Some(message) = normalized_openai_audio_contract(body).err() {
            assessment.reject(message);
        }
        if openai_assistant_history_audio_present(body) {
            assessment
                .reject(openai_assistant_audio_history_not_portable_message("Gemini"));
        }
        let controls = openai_warning_only_request_controls_for_translate(body, upstream_format);
        if !controls.is_empty() {
            let quoted = controls
                .iter()
                .map(|field| format!("`{field}`"))
                .collect::<Vec<_>>()
                .join(", ");
            assessment.warning(format!(
                "OpenAI Chat Completions controls {quoted} are not portable to Gemini and will be dropped"
            ));
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
        if anthropic_request_has_nonportable_thinking_provenance(body) {
            assessment.reject(anthropic_thinking_provenance_not_portable_message());
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

    if client_format == UpstreamFormat::Google && upstream_format != UpstreamFormat::Google {
        if let Some(message) =
            gemini_request_nonportable_message(body, translation_target_label(upstream_format))
        {
            assessment.reject(message);
        }
        if gemini_generation_config_field(body, "topK", "top_k").is_some() {
            assessment.warning(gemini_top_k_warning_message(upstream_format));
        }
        let controls = gemini_warning_only_request_controls_for_translate(body, upstream_format);
        if !controls.is_empty() {
            let quoted = controls
                .iter()
                .map(|field| format!("`{field}`"))
                .collect::<Vec<_>>()
                .join(", ");
            assessment.warning(format!(
                "Gemini controls {quoted} are not portable to {} and will be dropped",
                translation_target_label(upstream_format)
            ));
        }
    }

    if upstream_format == UpstreamFormat::Anthropic
        && request_contains_openai_reasoning_without_provenance(client_format, body)
    {
        assessment.reject(OPENAI_REASONING_TO_ANTHROPIC_REJECT_MESSAGE);
    }

    if matches!(
        upstream_format,
        UpstreamFormat::Anthropic | UpstreamFormat::Google
    ) && request_has_custom_tools(client_format, body)
    {
        assessment.reject(custom_tools_not_portable_message(upstream_format));
    }

    if matches!(
        upstream_format,
        UpstreamFormat::Anthropic | UpstreamFormat::Google
    ) {
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
    }))?
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
    body.get("content")
        .and_then(Value::as_array)
        .map(|content| anthropic_blocks_have_nonportable_thinking_provenance(content))
        .unwrap_or(false)
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
    let response_logprobs = match candidate {
        Some(candidate) => {
            normalized_response_logprobs_from_gemini_candidate(candidate, "OpenAI Chat Completions")?
        }
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
            parts
                .iter()
                .any(|part| part.get("type").and_then(Value::as_str) != Some("text"))
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
        body.get("choices")
            .and_then(Value::as_array)
            .map(Vec::as_slice),
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
        body.get("choices")
            .and_then(Value::as_array)
            .map(Vec::as_slice),
        "OpenAI response",
        "Gemini",
        "choices",
    )?;
    let message = choice.get("message").ok_or("missing message")?;
    let response_logprobs =
        normalized_response_logprobs_from_openai_choice(choice, "Gemini")?;
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

fn responses_response_to_openai(body: &Value) -> Result<Value, String> {
    let output = match body.get("output").and_then(Value::as_array) {
        Some(o) => o,
        None => return Ok(body.clone()),
    };
    if let Some(message) = output.iter().find_map(|item| {
        responses_nonportable_output_item_message(item, "OpenAI Chat Completions")
    }) {
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
        if let Some(tool_call) = responses_tool_call_item_to_openai_tool_call_strict(
            item,
            "OpenAI Chat Completions",
        )? {
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

fn openai_response_to_responses(body: &Value) -> Result<Value, String> {
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
    if let Some(reasoning) = openai_message_reasoning_text(message) {
        if !reasoning.is_empty() {
            output.push(serde_json::json!({
                "type": "reasoning",
                "summary": [{ "type": "summary_text", "text": reasoning }]
            }));
        }
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
            output.push(openai_tool_call_to_responses_item(t));
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
        client_to_openai_completion(client_format, upstream_format, body)?;
    }
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
            claude_to_openai(body)?;
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

fn responses_to_messages(body: &mut Value, target_format: UpstreamFormat) -> Result<(), String> {
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
                let Some(tc) = responses_tool_call_item_to_openai_tool_call_strict(
                    &item,
                    translation_target_label(target_format),
                )? else {
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
        if let Some(mapped_tool_choice) = responses_tool_choice_to_openai_tool_choice(&tool_choice)
        {
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

    // Convert tools from Responses API format to Chat Completions format
    // Responses: { "name": "...", "description": "...", "parameters": {...} }
    // Chat: { "type": "function", "function": { "name": "...", "description": "...", "parameters": {...} } }
    // OpenAI-family custom tools stay as top-level { type: "custom", name, ... }.
    if tools.is_some() {
        let converted_tools = normalized_responses_tool_definitions_from_request(body)?
            .into_iter()
            .map(|tool| normalized_tool_definition_to_openai(&tool))
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

fn messages_to_responses(body: &mut Value) -> Result<(), String> {
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
        "custom" => {
            let name = openai_named_tool_choice_name(choice, "custom")?;
            Some(serde_json::json!({
                "type": "custom",
                "custom": { "name": name }
            }))
        }
        "allowed_tools" => {
            let mode = obj.get("mode")?;
            let tools = obj.get("tools")?.as_array()?;
            let converted_tools = tools
                .iter()
                .map(|tool| {
                    match tool.get("type").and_then(Value::as_str) {
                        Some("function") if tool.get("name").is_some() => serde_json::json!({
                            "type": "function",
                            "function": { "name": tool.get("name").cloned().unwrap_or(Value::Null) }
                        }),
                        Some("custom") if tool.get("name").is_some() => serde_json::json!({
                            "type": "custom",
                            "custom": { "name": tool.get("name").cloned().unwrap_or(Value::Null) }
                        }),
                        _ => tool.clone(),
                    }
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
                                tool.get("custom")
                                    .and_then(|custom| custom.get("name"))
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
        if let Some(content) = claude_system_to_openai_content(system)? {
            result["messages"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({ "role": "system", "content": content }));
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

fn gemini_request_field<'a>(value: &'a Value, camel: &str, snake: &str) -> Option<&'a Value> {
    gemini_nested_field(value, camel, snake)
}

fn gemini_request_object_field<'a>(
    value: &'a Value,
    camel: &str,
    snake: &str,
) -> Option<&'a serde_json::Map<String, Value>> {
    gemini_request_field(value, camel, snake).and_then(Value::as_object)
}

fn gemini_request_array_field<'a>(
    value: &'a Value,
    camel: &str,
    snake: &str,
) -> Option<&'a Vec<Value>> {
    gemini_request_field(value, camel, snake).and_then(Value::as_array)
}

fn gemini_request_object_field_from_object<'a>(
    value: &'a serde_json::Map<String, Value>,
    camel: &str,
    snake: &str,
) -> Option<&'a serde_json::Map<String, Value>> {
    value
        .get(camel)
        .or_else(|| value.get(snake))
        .and_then(Value::as_object)
}

fn gemini_request_system_instruction(body: &Value) -> Option<&Value> {
    gemini_request_field(body, "systemInstruction", "system_instruction")
}

fn gemini_request_generation_config(body: &Value) -> Option<&Value> {
    gemini_request_field(body, "generationConfig", "generation_config")
}

fn gemini_request_tools(body: &Value) -> Option<&Vec<Value>> {
    gemini_request_array_field(body, "tools", "tools")
}

fn gemini_request_tool_config(body: &Value) -> Option<&serde_json::Map<String, Value>> {
    gemini_request_object_field(body, "toolConfig", "tool_config")
}

fn gemini_request_function_calling_config_from_object(
    tool_config: &serde_json::Map<String, Value>,
) -> Option<&serde_json::Map<String, Value>> {
    gemini_request_object_field_from_object(
        tool_config,
        "functionCallingConfig",
        "function_calling_config",
    )
}

fn gemini_generation_config_field<'a>(
    body: &'a Value,
    camel: &str,
    snake: &str,
) -> Option<&'a Value> {
    gemini_request_generation_config(body)
        .and_then(|generation_config| gemini_request_field(generation_config, camel, snake))
        .filter(|value| !value.is_null())
}

fn gemini_function_declaration_output_schema_field(
    declaration: &Value,
) -> Option<(&'static str, &Value)> {
    declaration
        .get("response")
        .filter(|value| !value.is_null())
        .map(|value| ("response", value))
        .or_else(|| {
            gemini_request_field(declaration, "responseJsonSchema", "response_json_schema")
                .filter(|value| !value.is_null())
                .map(|value| ("responseJsonSchema", value))
        })
}

fn gemini_function_output_schema_not_portable_message(
    declaration: &Value,
    field_name: &str,
    target_label: &str,
) -> String {
    format!(
        "Gemini FunctionDeclaration `{}` field `{field_name}` cannot be faithfully translated to {target_label}",
        gemini_function_declaration_name(declaration)
    )
}

fn gemini_request_nonportable_output_shape_message(
    body: &Value,
    target_label: &str,
) -> Option<String> {
    let response_schema = gemini_generation_config_field(body, "responseSchema", "response_schema");
    let response_json_schema =
        gemini_generation_config_field(body, "responseJsonSchema", "response_json_schema");
    let response_mime_type =
        gemini_generation_config_field(body, "responseMimeType", "response_mime_type")
            .and_then(Value::as_str);

    if response_schema.is_some() {
        return Some(format!(
            "Gemini generationConfig.responseSchema cannot be faithfully translated to {target_label}"
        ));
    }

    if response_json_schema.is_some() && response_mime_type != Some("application/json") {
        return Some(format!(
            "Gemini generationConfig.responseJsonSchema requires `responseMimeType: application/json` when translating to {target_label}"
        ));
    }

    match response_mime_type {
        Some("text/plain") | Some("application/json") | None => None,
        Some("text/x.enum") => Some(format!(
            "Gemini generationConfig.responseMimeType `text/x.enum` cannot be faithfully translated to {target_label}"
        )),
        Some(other) => Some(format!(
            "Gemini generationConfig.responseMimeType `{other}` cannot be faithfully translated to {target_label}"
        )),
    }
}

fn openai_response_format_json_schema(
    container: &serde_json::Map<String, Value>,
) -> Result<NormalizedJsonSchemaOutputShape, String> {
    let json_schema = container
        .get("json_schema")
        .and_then(Value::as_object)
        .ok_or("OpenAI response_format.type = json_schema requires a `json_schema` object")?;
    let schema = json_schema.get("schema").cloned().ok_or(
        "OpenAI response_format.json_schema.schema is required for structured output translation",
    )?;
    let name = json_schema
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .unwrap_or("structured_output")
        .to_string();
    let description = json_schema
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_string);
    let strict = json_schema.get("strict").and_then(Value::as_bool);
    Ok(NormalizedJsonSchemaOutputShape {
        name,
        schema,
        description,
        strict,
    })
}

fn responses_text_format_json_schema(
    container: &serde_json::Map<String, Value>,
) -> Result<NormalizedJsonSchemaOutputShape, String> {
    let schema = container
        .get("schema")
        .cloned()
        .ok_or("Responses text.format.type = json_schema requires a `schema` object")?;
    let name = container
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .unwrap_or("structured_output")
        .to_string();
    let description = container
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_string);
    let strict = container.get("strict").and_then(Value::as_bool);
    Ok(NormalizedJsonSchemaOutputShape {
        name,
        schema,
        description,
        strict,
    })
}

fn openai_normalized_output_shape(body: &Value) -> Result<Option<NormalizedOutputShape>, String> {
    let Some(response_format) = body.get("response_format").filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let Some(response_format) = response_format.as_object() else {
        return Err(
            "OpenAI response_format must be an object when translating structured output controls"
                .to_string(),
        );
    };
    match response_format.get("type").and_then(Value::as_str) {
        Some("text") => Ok(Some(NormalizedOutputShape::Text)),
        Some("json_object") => Ok(Some(NormalizedOutputShape::JsonObject)),
        Some("json_schema") => Ok(Some(NormalizedOutputShape::JsonSchema(
            openai_response_format_json_schema(response_format)?,
        ))),
        Some(other) => Err(format!(
            "OpenAI response_format.type `{other}` cannot be faithfully translated"
        )),
        None => Err(
            "OpenAI response_format.type is required when translating structured output controls"
                .to_string(),
        ),
    }
}

fn responses_normalized_output_shape(
    body: &Value,
) -> Result<Option<NormalizedOutputShape>, String> {
    let Some(text) = body
        .get("text")
        .and_then(Value::as_object)
        .and_then(|text| text.get("format"))
        .filter(|value| !value.is_null())
    else {
        return Ok(None);
    };
    let Some(text_format) = text.as_object() else {
        return Err(
            "Responses text.format must be an object when translating structured output controls"
                .to_string(),
        );
    };
    match text_format.get("type").and_then(Value::as_str) {
        Some("text") => Ok(Some(NormalizedOutputShape::Text)),
        Some("json_object") => Ok(Some(NormalizedOutputShape::JsonObject)),
        Some("json_schema") => Ok(Some(NormalizedOutputShape::JsonSchema(
            responses_text_format_json_schema(text_format)?,
        ))),
        Some(other) => Err(format!(
            "Responses text.format.type `{other}` cannot be faithfully translated"
        )),
        None => Err(
            "Responses text.format.type is required when translating structured output controls"
                .to_string(),
        ),
    }
}

fn gemini_normalized_output_shape(body: &Value) -> Result<Option<NormalizedOutputShape>, String> {
    let response_json_schema =
        gemini_generation_config_field(body, "responseJsonSchema", "response_json_schema");
    let response_schema = gemini_generation_config_field(body, "responseSchema", "response_schema");
    let response_mime_type =
        gemini_generation_config_field(body, "responseMimeType", "response_mime_type")
            .and_then(Value::as_str);

    if response_schema.is_some() {
        return Err(
            "Gemini generationConfig.responseSchema cannot be faithfully translated to non-Gemini targets"
                .to_string(),
        );
    }

    if response_json_schema.is_some() && response_mime_type != Some("application/json") {
        return Err(
            "Gemini generationConfig.responseJsonSchema requires `responseMimeType: application/json` for cross-protocol translation"
                .to_string(),
        );
    }

    match response_mime_type {
        Some("text/plain") => Ok(Some(NormalizedOutputShape::Text)),
        Some("application/json") => {
            if let Some(schema) = response_json_schema {
                Ok(Some(NormalizedOutputShape::JsonSchema(
                    NormalizedJsonSchemaOutputShape {
                        name: "gemini_response".to_string(),
                        schema: schema.clone(),
                        description: None,
                        strict: None,
                    },
                )))
            } else {
                Ok(Some(NormalizedOutputShape::JsonObject))
            }
        }
        Some("text/x.enum") => Err(
            "Gemini generationConfig.responseMimeType `text/x.enum` cannot be faithfully translated to non-Gemini targets"
                .to_string(),
        ),
        Some(other) => Err(format!(
            "Gemini generationConfig.responseMimeType `{other}` cannot be faithfully translated to non-Gemini targets"
        )),
        None => {
            if response_json_schema.is_some() {
                Err(
                    "Gemini generationConfig.responseJsonSchema requires `responseMimeType: application/json` for cross-protocol translation"
                        .to_string(),
                )
            } else {
                Ok(None)
            }
        }
    }
}

fn openai_normalized_decoding_controls(body: &Value) -> NormalizedDecodingControls {
    NormalizedDecodingControls {
        stop: body.get("stop").cloned(),
        seed: body.get("seed").cloned(),
        presence_penalty: body.get("presence_penalty").cloned(),
        frequency_penalty: body.get("frequency_penalty").cloned(),
        top_k: None,
    }
}

fn gemini_normalized_decoding_controls(body: &Value) -> NormalizedDecodingControls {
    NormalizedDecodingControls {
        stop: gemini_generation_config_field(body, "stopSequences", "stop_sequences").cloned(),
        seed: gemini_generation_config_field(body, "seed", "seed").cloned(),
        presence_penalty: gemini_generation_config_field(
            body,
            "presencePenalty",
            "presence_penalty",
        )
        .cloned(),
        frequency_penalty: gemini_generation_config_field(
            body,
            "frequencyPenalty",
            "frequency_penalty",
        )
        .cloned(),
        top_k: gemini_generation_config_field(body, "topK", "top_k").cloned(),
    }
}

fn openai_function_tool_name(value: &Value) -> Option<&str> {
    if value.get("type").and_then(Value::as_str) != Some("function") {
        return None;
    }
    value
        .get("function")
        .and_then(|function| function.get("name"))
        .or_else(|| value.get("name"))
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
}

fn openai_declared_function_tools(body: &Value) -> Vec<Value> {
    body.get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter(|tool| openai_function_tool_name(tool).is_some())
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

fn openai_select_function_tools_by_name(
    tools: &[Value],
    selected_names: &[String],
    selector_label: &str,
) -> Result<Vec<Value>, String> {
    selected_names
        .iter()
        .map(|selected_name| {
            tools.iter()
                .find(|tool| openai_function_tool_name(tool) == Some(selected_name.as_str()))
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "OpenAI {selector_label} selected function `{selected_name}`, but no matching declared function tool exists."
                    )
                })
        })
        .collect()
}

fn openai_tool_choice_allowed_tools_object(
    choice: &serde_json::Map<String, Value>,
) -> Option<&serde_json::Map<String, Value>> {
    choice
        .get("allowed_tools")
        .and_then(Value::as_object)
        .or_else(|| {
            if choice.get("mode").is_some() && choice.get("tools").is_some() {
                Some(choice)
            } else {
                None
            }
        })
}

fn openai_legacy_allowed_tool_names(body: &Value) -> Option<Vec<String>> {
    let allowed_tool_names = body.get("allowed_tool_names")?.as_array()?;
    if allowed_tool_names.is_empty() {
        return None;
    }
    Some(
        allowed_tool_names
            .iter()
            .filter_map(|name| name.as_str().map(str::to_string))
            .filter(|name| !name.is_empty())
            .collect(),
    )
    .filter(|names: &Vec<String>| !names.is_empty())
}

fn openai_normalized_tool_policy(
    body: &Value,
) -> Result<(Option<NormalizedToolPolicy>, Option<Vec<String>>), String> {
    let declared_tools = openai_declared_function_tools(body);
    let legacy_allowed_tool_names = openai_legacy_allowed_tool_names(body);
    let Some(tool_choice) = body.get("tool_choice").filter(|value| !value.is_null()) else {
        return Ok((None, None));
    };

    if let Some(choice) = tool_choice.as_str() {
        let tool_policy = match choice {
            "auto" => Some(NormalizedToolPolicy::Auto),
            "none" => Some(NormalizedToolPolicy::None),
            "required" => Some(NormalizedToolPolicy::Required),
            _ => None,
        };
        let restricted_tool_names = if choice == "required" {
            legacy_allowed_tool_names
        } else {
            None
        };
        return Ok((tool_policy, restricted_tool_names));
    }

    let Some(tool_choice) = tool_choice.as_object() else {
        return Ok((None, None));
    };
    let Some(choice_type) = tool_choice.get("type").and_then(Value::as_str) else {
        return Ok((None, None));
    };

    match choice_type {
        "function" => {
            let name = tool_choice
                .get("name")
                .or_else(|| {
                    tool_choice
                        .get("function")
                        .and_then(|function| function.get("name"))
                })
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .ok_or(
                    "OpenAI tool_choice.type = function requires a non-empty function name."
                        .to_string(),
                )?;
            Ok((
                Some(NormalizedToolPolicy::ForcedFunction(name.to_string())),
                None,
            ))
        }
        "allowed_tools" => {
            let allowed_tools = openai_tool_choice_allowed_tools_object(tool_choice).ok_or(
                "OpenAI tool_choice.type = allowed_tools requires an `allowed_tools` payload."
                    .to_string(),
            )?;
            let mode = allowed_tools.get("mode").and_then(Value::as_str).ok_or(
                "OpenAI tool_choice.allowed_tools.mode is required for allowed_tools translation."
                    .to_string(),
            )?;
            let selected_tools = allowed_tools.get("tools").and_then(Value::as_array).ok_or(
                "OpenAI tool_choice.allowed_tools.tools must be an array of selected tools."
                    .to_string(),
            )?;
            if selected_tools.is_empty() {
                return Err(
                    "OpenAI tool_choice.allowed_tools.tools cannot be empty for cross-protocol translation."
                        .to_string(),
                );
            }

            let mut selected_names = Vec::with_capacity(selected_tools.len());
            for selected_tool in selected_tools {
                match selected_tool.get("type").and_then(Value::as_str) {
                    Some("function") => {}
                    Some("custom") => return Ok((None, None)),
                    _ => {
                        return Err(
                            "OpenAI tool_choice.allowed_tools is only portable for function/custom tools."
                                .to_string(),
                        )
                    }
                }
                let Some(name) = openai_function_tool_name(selected_tool) else {
                    return Err(
                        "OpenAI tool_choice.allowed_tools function selections require non-empty function names."
                            .to_string(),
                    );
                };
                selected_names.push(name.to_string());
            }

            openai_select_function_tools_by_name(
                &declared_tools,
                &selected_names,
                "tool_choice.allowed_tools",
            )?;

            let tool_policy = match mode {
                "auto" => NormalizedToolPolicy::Auto,
                "required" => NormalizedToolPolicy::Required,
                other => {
                    return Err(format!(
                        "OpenAI tool_choice.allowed_tools.mode `{other}` is not portable."
                    ))
                }
            };
            Ok((Some(tool_policy), Some(selected_names)))
        }
        _ => Ok((None, None)),
    }
}

fn openai_normalized_request_controls(body: &Value) -> Result<NormalizedRequestControls, String> {
    let (tool_policy, restricted_tool_names) = openai_normalized_tool_policy(body)?;
    Ok(NormalizedRequestControls {
        tool_policy,
        restricted_tool_names,
        output_shape: openai_normalized_output_shape(body)?,
        decoding: openai_normalized_decoding_controls(body),
        logprobs: openai_normalized_logprobs_controls(body),
        metadata: body.get("metadata").cloned(),
        user: body.get("user").cloned(),
        service_tier: body.get("service_tier").cloned(),
        stream_include_obfuscation: request_stream_include_obfuscation(body),
        verbosity: body.get("verbosity").cloned(),
        reasoning_effort: body.get("reasoning_effort").cloned(),
        prompt_cache_key: body.get("prompt_cache_key").cloned(),
        prompt_cache_retention: body.get("prompt_cache_retention").cloned(),
        safety_identifier: body.get("safety_identifier").cloned(),
        parallel_tool_calls: body.get("parallel_tool_calls").cloned(),
        store: body.get("store").cloned(),
    })
}

fn responses_normalized_request_controls(
    body: &Value,
) -> Result<NormalizedRequestControls, String> {
    Ok(NormalizedRequestControls {
        output_shape: responses_normalized_output_shape(body)?,
        logprobs: responses_normalized_logprobs_controls(body),
        metadata: body.get("metadata").cloned(),
        user: body.get("user").cloned(),
        service_tier: body.get("service_tier").cloned(),
        stream_include_obfuscation: request_stream_include_obfuscation(body),
        verbosity: responses_text_verbosity(body),
        reasoning_effort: responses_reasoning_effort(body),
        prompt_cache_key: body.get("prompt_cache_key").cloned(),
        prompt_cache_retention: body.get("prompt_cache_retention").cloned(),
        safety_identifier: body.get("safety_identifier").cloned(),
        parallel_tool_calls: body.get("parallel_tool_calls").cloned(),
        store: body.get("store").cloned(),
        ..NormalizedRequestControls::default()
    })
}

fn normalized_output_shape_to_openai_response_format(shape: &NormalizedOutputShape) -> Value {
    match shape {
        NormalizedOutputShape::Text => serde_json::json!({ "type": "text" }),
        NormalizedOutputShape::JsonObject => serde_json::json!({ "type": "json_object" }),
        NormalizedOutputShape::JsonSchema(schema) => {
            let mut json_schema = serde_json::Map::new();
            json_schema.insert("name".to_string(), Value::String(schema.name.clone()));
            json_schema.insert("schema".to_string(), schema.schema.clone());
            if let Some(description) = &schema.description {
                json_schema.insert(
                    "description".to_string(),
                    Value::String(description.clone()),
                );
            }
            if let Some(strict) = schema.strict {
                json_schema.insert("strict".to_string(), Value::Bool(strict));
            }
            let mut response_format = serde_json::Map::new();
            response_format.insert("type".to_string(), Value::String("json_schema".to_string()));
            response_format.insert("json_schema".to_string(), Value::Object(json_schema));
            Value::Object(response_format)
        }
    }
}

fn normalized_output_shape_to_responses_text_format(shape: &NormalizedOutputShape) -> Value {
    match shape {
        NormalizedOutputShape::Text => serde_json::json!({ "type": "text" }),
        NormalizedOutputShape::JsonObject => serde_json::json!({ "type": "json_object" }),
        NormalizedOutputShape::JsonSchema(schema) => {
            let mut text_format = serde_json::Map::new();
            text_format.insert("type".to_string(), Value::String("json_schema".to_string()));
            text_format.insert("name".to_string(), Value::String(schema.name.clone()));
            text_format.insert("schema".to_string(), schema.schema.clone());
            if let Some(description) = &schema.description {
                text_format.insert(
                    "description".to_string(),
                    Value::String(description.clone()),
                );
            }
            if let Some(strict) = schema.strict {
                text_format.insert("strict".to_string(), Value::Bool(strict));
            }
            Value::Object(text_format)
        }
    }
}

fn normalized_output_shape_to_gemini_generation_config(shape: &NormalizedOutputShape) -> Value {
    match shape {
        NormalizedOutputShape::Text => {
            serde_json::json!({ "responseMimeType": "text/plain" })
        }
        NormalizedOutputShape::JsonObject => {
            serde_json::json!({ "responseMimeType": "application/json" })
        }
        NormalizedOutputShape::JsonSchema(schema) => serde_json::json!({
            "responseMimeType": "application/json",
            "responseJsonSchema": schema.schema.clone()
        }),
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

fn openai_stop_to_gemini_stop_sequences(stop: &Value) -> Value {
    if stop.is_array() {
        stop.clone()
    } else {
        Value::Array(vec![stop.clone()])
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

fn gemini_tool_function_declarations(tool: &Value) -> Option<&Vec<Value>> {
    gemini_request_array_field(tool, "functionDeclarations", "function_declarations")
}

enum GeminiFunctionSchemaSource<'a> {
    Parameters(&'a Value),
    ParametersJsonSchema(&'a Value),
}

impl<'a> GeminiFunctionSchemaSource<'a> {
    fn value(&self) -> &'a Value {
        match self {
            Self::Parameters(schema) | Self::ParametersJsonSchema(schema) => schema,
        }
    }
}

fn gemini_function_declaration_name(declaration: &Value) -> &str {
    declaration
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .unwrap_or("<unnamed>")
}

fn gemini_function_declaration_schema_source<'a>(
    declaration: &'a Value,
) -> Result<Option<GeminiFunctionSchemaSource<'a>>, String> {
    let parameters = declaration
        .get("parameters")
        .filter(|value| !value.is_null());
    let parameters_json_schema = gemini_request_field(
        declaration,
        "parametersJsonSchema",
        "parameters_json_schema",
    )
    .filter(|value| !value.is_null());

    match (parameters, parameters_json_schema) {
        (Some(_), Some(_)) => Err(format!(
            "Gemini FunctionDeclaration `{}` cannot specify both `parameters` and `parametersJsonSchema`; they are mutually exclusive.",
            gemini_function_declaration_name(declaration)
        )),
        (Some(schema), None) => Ok(Some(GeminiFunctionSchemaSource::Parameters(schema))),
        (None, Some(schema)) => Ok(Some(GeminiFunctionSchemaSource::ParametersJsonSchema(
            schema,
        ))),
        (None, None) => Ok(None),
    }
}

fn gemini_openai_function_tool_from_declaration(declaration: &Value) -> Result<Value, String> {
    if let Some((field_name, _)) = gemini_function_declaration_output_schema_field(declaration) {
        return Err(gemini_function_output_schema_not_portable_message(
            declaration,
            field_name,
            "non-Gemini targets",
        ));
    }

    let mut function = serde_json::Map::new();
    function.insert(
        "name".to_string(),
        declaration.get("name").cloned().unwrap_or(Value::Null),
    );
    function.insert(
        "description".to_string(),
        declaration
            .get("description")
            .cloned()
            .unwrap_or_else(|| serde_json::json!("")),
    );
    if let Some(schema_source) = gemini_function_declaration_schema_source(declaration)? {
        function.insert("parameters".to_string(), schema_source.value().clone());
    }

    Ok(serde_json::json!({
        "type": "function",
        "function": Value::Object(function)
    }))
}

fn gemini_openai_function_tools_from_request(body: &Value) -> Result<Vec<Value>, String> {
    let mut tools = Vec::new();
    if let Some(request_tools) = gemini_request_tools(body) {
        for tool in request_tools {
            if let Some(declarations) = gemini_tool_function_declarations(tool) {
                for declaration in declarations {
                    tools.push(gemini_openai_function_tool_from_declaration(declaration)?);
                }
            }
        }
    }
    Ok(tools)
}

fn gemini_openai_function_tool_name(tool: &Value) -> Option<&str> {
    tool.get("function")
        .and_then(|function| function.get("name"))
        .and_then(Value::as_str)
}

fn gemini_select_openai_tools_by_name(
    tools: &[Value],
    allowed_names: &[String],
) -> Result<Vec<Value>, String> {
    allowed_names
        .iter()
        .map(|allowed_name| {
            tools.iter()
                .find(|tool| gemini_openai_function_tool_name(tool) == Some(allowed_name.as_str()))
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "Gemini functionCallingConfig.allowedFunctionNames entry `{allowed_name}` has no matching function declaration."
                    )
                })
        })
        .collect()
}

fn gemini_validated_allowed_function_names(
    function_calling_config: &serde_json::Map<String, Value>,
    openai_tools: &[Value],
) -> Result<Option<Vec<String>>, String> {
    let Some(allowed_function_names) = function_calling_config
        .get("allowedFunctionNames")
        .or_else(|| function_calling_config.get("allowed_function_names"))
        .filter(|value| !value.is_null())
    else {
        return Ok(None);
    };

    let Some(allowed_function_names) = allowed_function_names.as_array() else {
        return Err(
            "Gemini functionCallingConfig.allowedFunctionNames must be an array of non-empty strings."
                .to_string(),
        );
    };

    if allowed_function_names.is_empty() {
        return Err(
            "Gemini functionCallingConfig.allowedFunctionNames cannot be empty.".to_string(),
        );
    }

    let mut validated_names = Vec::with_capacity(allowed_function_names.len());
    for name in allowed_function_names {
        let Some(name) = name.as_str() else {
            return Err(
                "Gemini functionCallingConfig.allowedFunctionNames must contain only non-empty strings."
                    .to_string(),
            );
        };
        if name.trim().is_empty() {
            return Err(
                "Gemini functionCallingConfig.allowedFunctionNames must contain only non-empty strings."
                    .to_string(),
            );
        }
        validated_names.push(name.to_string());
    }

    gemini_select_openai_tools_by_name(openai_tools, &validated_names)?;
    Ok(Some(validated_names))
}

fn gemini_normalized_request_controls(
    body: &Value,
    openai_tools: &[Value],
) -> Result<NormalizedRequestControls, String> {
    let mut controls = NormalizedRequestControls {
        output_shape: gemini_normalized_output_shape(body)?,
        decoding: gemini_normalized_decoding_controls(body),
        logprobs: gemini_normalized_logprobs_controls(body),
        store: body.get("store").cloned(),
        ..NormalizedRequestControls::default()
    };

    let Some(tool_config) = gemini_request_tool_config(body) else {
        return Ok(controls);
    };
    let Some(function_calling_config) =
        gemini_request_function_calling_config_from_object(tool_config)
    else {
        return Ok(controls);
    };

    let mode = function_calling_config.get("mode").and_then(Value::as_str);
    let allowed_names =
        gemini_validated_allowed_function_names(function_calling_config, openai_tools)?;

    if allowed_names.is_some() && mode != Some("ANY") {
        return Err(
            "Gemini functionCallingConfig.allowedFunctionNames is only portable with mode ANY."
                .to_string(),
        );
    }

    controls.tool_policy = match mode {
        Some("NONE") => Some(NormalizedToolPolicy::None),
        Some("AUTO") => Some(NormalizedToolPolicy::Auto),
        Some("ANY") => {
            if let Some(allowed_names) = allowed_names.as_ref() {
                if allowed_names.len() == 1 {
                    Some(NormalizedToolPolicy::ForcedFunction(
                        allowed_names[0].clone(),
                    ))
                } else {
                    Some(NormalizedToolPolicy::Required)
                }
            } else {
                Some(NormalizedToolPolicy::Required)
            }
        }
        Some("VALIDATED") => {
            return Err(
                "Gemini functionCallingConfig.mode = VALIDATED cannot be faithfully translated to non-Gemini targets"
                    .to_string(),
            )
        }
        Some(other) => {
            return Err(format!(
                "Gemini functionCallingConfig.mode = {other} cannot be faithfully translated to non-Gemini targets"
            ))
        }
        None => None,
    };
    controls.restricted_tool_names = allowed_names;
    Ok(controls)
}

fn normalized_tool_policy_to_openai_tool_choice(tool_policy: &NormalizedToolPolicy) -> Value {
    match tool_policy {
        NormalizedToolPolicy::Auto => serde_json::json!("auto"),
        NormalizedToolPolicy::None => serde_json::json!("none"),
        NormalizedToolPolicy::Required => serde_json::json!("required"),
        NormalizedToolPolicy::ForcedFunction(name) => serde_json::json!({
            "type": "function",
            "function": { "name": name }
        }),
    }
}

fn gemini_to_openai(body: &mut Value) -> Result<(), String> {
    let mut result = serde_json::json!({
        "model": body.get("model").cloned().unwrap_or(serde_json::Value::Null),
        "messages": [],
        "stream": body.get("stream").cloned().unwrap_or(serde_json::json!(false))
    });
    if let Some(gc) = gemini_request_generation_config(body) {
        if let Some(n) = gemini_request_field(gc, "maxOutputTokens", "max_output_tokens") {
            result["max_tokens"] = n.clone();
        }
        if let Some(t) = gemini_request_field(gc, "temperature", "temperature") {
            result["temperature"] = t.clone();
        }
        if let Some(p) = gemini_request_field(gc, "topP", "top_p") {
            result["top_p"] = p.clone();
        }
    }
    if let Some(si) = gemini_request_system_instruction(body) {
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
    let out = gemini_openai_function_tools_from_request(body)?;
    if !out.is_empty() {
        result["tools"] = Value::Array(out);
    }
    let openai_tools = result["tools"].as_array().cloned().unwrap_or_default();
    let controls = gemini_normalized_request_controls(body, &openai_tools)?;
    if let Some(tool_policy) = controls.tool_policy.as_ref() {
        result["tool_choice"] = normalized_tool_policy_to_openai_tool_choice(tool_policy);
    }
    if let Some(allowed_names) = controls.restricted_tool_names.as_ref() {
        result["tools"] = Value::Array(gemini_select_openai_tools_by_name(
            &openai_tools,
            allowed_names,
        )?);
    }
    if let Some(output_shape) = controls.output_shape.as_ref() {
        result["response_format"] = normalized_output_shape_to_openai_response_format(output_shape);
    }
    if let Some(logprobs) = controls.logprobs.as_ref() {
        if logprobs.enabled || logprobs.top_logprobs.is_some() {
            result["logprobs"] = Value::Bool(true);
        }
        if let Some(top_logprobs) = logprobs.top_logprobs.as_ref() {
            result["top_logprobs"] = top_logprobs.clone();
        }
    }
    let decoding = controls.decoding;
    if let Some(stop) = decoding.stop {
        result["stop"] = stop;
    }
    if let Some(seed) = decoding.seed {
        result["seed"] = seed;
    }
    if let Some(presence_penalty) = decoding.presence_penalty {
        result["presence_penalty"] = presence_penalty;
    }
    if let Some(frequency_penalty) = decoding.frequency_penalty {
        result["frequency_penalty"] = frequency_penalty;
    }
    if result
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| tools.is_empty())
    {
        if let Some(obj) = result.as_object_mut() {
            obj.remove("tools");
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
        && compact
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '=' | '\r' | '\n'))
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
            if gemini_function_response_has_nonportable_parts(fr) {
                return Err(gemini_function_response_parts_not_portable_message(
                    "OpenAI Chat Completions",
                ));
            }
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

fn normalized_tool_policy_to_gemini_function_calling_config(
    tool_policy: &NormalizedToolPolicy,
) -> Value {
    match tool_policy {
        NormalizedToolPolicy::Auto => serde_json::json!({ "mode": "AUTO" }),
        NormalizedToolPolicy::None => serde_json::json!({ "mode": "NONE" }),
        NormalizedToolPolicy::Required => serde_json::json!({ "mode": "ANY" }),
        NormalizedToolPolicy::ForcedFunction(name) => serde_json::json!({
            "mode": "ANY",
            "allowedFunctionNames": [name]
        }),
    }
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

fn gemini_display_name_extension_for_mime_type(mime_type: &str) -> String {
    match mime_type
        .split(';')
        .next()
        .unwrap_or(mime_type)
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "image/png" => "png".to_string(),
        "image/jpeg" => "jpg".to_string(),
        "image/webp" => "webp".to_string(),
        "image/gif" => "gif".to_string(),
        "audio/wav" | "audio/x-wav" => "wav".to_string(),
        "audio/mpeg" => "mp3".to_string(),
        "audio/mp4" => "m4a".to_string(),
        "audio/ogg" => "ogg".to_string(),
        "application/pdf" => "pdf".to_string(),
        "application/json" => "json".to_string(),
        "text/plain" => "txt".to_string(),
        other => other.rsplit('/').next().unwrap_or("bin").to_string(),
    }
}

fn gemini_tool_result_display_name(
    call_id: Option<&str>,
    media_index: usize,
    mime_type: &str,
) -> String {
    let stem = call_id
        .filter(|value| !value.is_empty())
        .unwrap_or("tool_result");
    let extension = gemini_display_name_extension_for_mime_type(mime_type);
    format!("{stem}_part_{media_index}.{extension}")
}

fn gemini_function_response_parts_supported_for_model(model: &str) -> bool {
    model.trim().to_ascii_lowercase().contains("gemini-3")
}

fn gemini_function_response_parts_supported_mime_type(mime_type: &str) -> bool {
    matches!(
        mime_type
            .split(';')
            .next()
            .unwrap_or(mime_type)
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "image/png" | "image/jpeg" | "image/webp" | "application/pdf" | "text/plain"
    )
}

fn ensure_gemini_function_response_part_supported(
    target_model: &str,
    mime_type: &str,
) -> Result<(), String> {
    if !gemini_function_response_parts_supported_for_model(target_model) {
        return Err(format!(
            "Gemini functionResponse.parts with inline media are only documented for Gemini 3 series models; target model `{target_model}` cannot be faithfully translated."
        ));
    }
    if !gemini_function_response_parts_supported_mime_type(mime_type) {
        return Err(format!(
            "Gemini functionResponse.parts MIME `{mime_type}` is not documented as supported; only image/png, image/jpeg, image/webp, application/pdf, and text/plain are allowed."
        ));
    }
    Ok(())
}

fn gemini_tool_result_inline_part(display_name: &str, mime_type: &str, data: &str) -> Value {
    serde_json::json!({
        "inlineData": {
            "displayName": display_name,
            "mimeType": mime_type,
            "data": data
        }
    })
}

fn gemini_tool_result_media_response_item(
    kind: &str,
    display_name: &str,
    mime_type: &str,
    filename: Option<&str>,
) -> Value {
    let mut item = serde_json::json!({
        "type": kind,
        "mimeType": mime_type,
    });
    item[kind] = serde_json::json!({ "$ref": display_name });
    if let Some(filename) = filename {
        item["filename"] = Value::String(filename.to_string());
    }
    item
}

fn gemini_tool_result_parse_image_data_uri(
    url: &str,
    part_type: &str,
) -> Result<(String, String), String> {
    let Some((mime_type, data)) = base64_data_uri_parts(url) else {
        return Err(format!(
            "Gemini tool result `{part_type}` parts require inline base64 data URIs; remote references cannot be faithfully translated to Gemini tool result parts."
        ));
    };
    Ok((mime_type.to_string(), data.to_string()))
}

fn gemini_tool_result_block_kind(block: &Value) -> Option<&str> {
    block.get("type").and_then(Value::as_str)
}

fn gemini_tool_result_array_media_status(blocks: &[Value]) -> Result<bool, String> {
    let mut saw_media = false;
    let mut saw_typed = false;
    let mut saw_untyped = false;
    for block in blocks {
        let Some(kind) = gemini_tool_result_block_kind(block) else {
            saw_untyped = true;
            continue;
        };
        saw_typed = true;
        match kind {
            "text" | "input_text" | "output_text" | "json" => {}
            "image_url" | "input_image" | "input_audio" | "file" | "image" => {
                saw_media = true;
            }
            other => {
                return Err(format!(
                    "OpenAI/Responses/Anthropic tool result block `{other}` cannot be faithfully translated to Gemini."
                ));
            }
        }
    }
    if saw_typed && saw_untyped {
        return Err(
            "OpenAI/Responses/Anthropic tool result arrays cannot mix typed blocks with untyped JSON values when translating to Gemini."
                .to_string(),
        );
    }
    Ok(saw_media)
}

fn gemini_tool_result_block_to_response_and_part(
    block: &Value,
    call_id: Option<&str>,
    media_index: usize,
    target_model: &str,
) -> Result<(Value, Option<Value>), String> {
    let kind = gemini_tool_result_block_kind(block)
        .ok_or("tool result typed blocks require a `type` field to translate to Gemini")?;
    match kind {
        "text" => Ok((
            serde_json::json!({
                "type": "text",
                "text": block.get("text").cloned().unwrap_or(Value::String(String::new()))
            }),
            None,
        )),
        "input_text" | "output_text" => Ok((
            serde_json::json!({
                "type": "text",
                "text": block.get("text").cloned().unwrap_or(Value::String(String::new()))
            }),
            None,
        )),
        "json" => Ok((
            serde_json::json!({
                "type": "json",
                "json": block.get("json").cloned().unwrap_or(Value::Null)
            }),
            None,
        )),
        "image_url" | "input_image" => {
            let url = block
                .get("image_url")
                .and_then(|image| image.get("url"))
                .and_then(Value::as_str)
                .or_else(|| block.get("image_url").and_then(Value::as_str))
                .unwrap_or("");
            let (mime_type, data) = gemini_tool_result_parse_image_data_uri(url, kind)?;
            ensure_gemini_function_response_part_supported(target_model, &mime_type)?;
            let display_name = gemini_tool_result_display_name(call_id, media_index, &mime_type);
            Ok((
                gemini_tool_result_media_response_item(
                    "image",
                    &display_name,
                    &mime_type,
                    None,
                ),
                Some(gemini_tool_result_inline_part(
                    &display_name,
                    &mime_type,
                    &data,
                )),
            ))
        }
        "input_audio" => {
            let input_audio = block.get("input_audio").unwrap_or(&Value::Null);
            let data = input_audio
                .get("data")
                .and_then(Value::as_str)
                .unwrap_or("");
            if data.is_empty() {
                return Err(
                    "Gemini tool result `input_audio` parts require inline base64 data to translate to Gemini."
                        .to_string(),
                );
            }
            let mime_type = openai_audio_mime_type(
                input_audio
                    .get("format")
                    .and_then(Value::as_str)
                    .unwrap_or("wav"),
            );
            ensure_gemini_function_response_part_supported(target_model, &mime_type)?;
            let display_name = gemini_tool_result_display_name(call_id, media_index, &mime_type);
            Ok((
                gemini_tool_result_media_response_item(
                    "audio",
                    &display_name,
                    &mime_type,
                    None,
                ),
                Some(gemini_tool_result_inline_part(
                    &display_name,
                    &mime_type,
                    data,
                )),
            ))
        }
        "file" => {
            let file = block
                .get("file")
                .and_then(Value::as_object)
                .ok_or("Gemini tool result `file` blocks require a file object.")?;
            if let Some(file_id) = file.get("file_id").and_then(Value::as_str) {
                return Err(format!(
                    "Gemini tool result file_id `{file_id}` cannot be faithfully translated to Gemini without inline file bytes."
                ));
            }
            let filename = file.get("filename").and_then(Value::as_str);
            let Some(file_data) = file.get("file_data").and_then(Value::as_str) else {
                return Err(
                    "Gemini tool result `file` blocks require inline file_data to translate to Gemini."
                        .to_string(),
                );
            };
            let (mime_type, data) = match openai_file_data_reference(file_data, filename)? {
                OpenAiFileDataReference::InlineData { mime_type, data } => {
                    (mime_type, data.to_string())
                }
                OpenAiFileDataReference::FileUri(file_uri) => {
                    return Err(format!(
                        "Gemini tool result file reference `{file_uri}` cannot be faithfully translated without inline file bytes."
                    ));
                }
            };
            ensure_gemini_function_response_part_supported(target_model, &mime_type)?;
            let display_name = gemini_tool_result_display_name(call_id, media_index, &mime_type);
            Ok((
                gemini_tool_result_media_response_item(
                    "file",
                    &display_name,
                    &mime_type,
                    filename,
                ),
                Some(gemini_tool_result_inline_part(
                    &display_name,
                    &mime_type,
                    &data,
                )),
            ))
        }
        "image" => {
            let source = block.get("source").unwrap_or(&Value::Null);
            if source.get("type").and_then(Value::as_str) != Some("base64") {
                return Err(
                    "Gemini tool result `image` blocks require inline base64 image data to translate to Gemini."
                        .to_string(),
                );
            }
            let mime_type = source
                .get("media_type")
                .and_then(Value::as_str)
                .unwrap_or("image/png")
                .to_string();
            let data = source
                .get("data")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            ensure_gemini_function_response_part_supported(target_model, &mime_type)?;
            let display_name = gemini_tool_result_display_name(call_id, media_index, &mime_type);
            Ok((
                gemini_tool_result_media_response_item(
                    "image",
                    &display_name,
                    &mime_type,
                    None,
                ),
                Some(gemini_tool_result_inline_part(
                    &display_name,
                    &mime_type,
                    &data,
                )),
            ))
        }
        other => Err(format!(
            "OpenAI/Responses/Anthropic tool result block `{other}` cannot be faithfully translated to Gemini."
        )),
    }
}

fn tool_message_to_gemini_function_response(
    msg: &Value,
    function_name: Value,
    target_model: &str,
) -> Result<Value, String> {
    if semantic_tool_kind_from_value(msg) == SemanticToolKind::OpenAiCustom {
        return Err(custom_tools_not_portable_message(UpstreamFormat::Google));
    }
    let call_id = msg.get("tool_call_id").cloned();
    let call_id_str = msg.get("tool_call_id").and_then(Value::as_str);
    let content = msg
        .get("content")
        .cloned()
        .unwrap_or(Value::String(String::new()));

    let mut function_response = serde_json::json!({
        "id": call_id,
        "name": function_name,
        "response": { "result": content.clone() }
    });

    let blocks = if let Some(blocks) = content.as_array() {
        Some(blocks.to_vec())
    } else if matches!(
        gemini_tool_result_block_kind(&content),
        Some(
            "text"
                | "input_text"
                | "output_text"
                | "json"
                | "image_url"
                | "input_image"
                | "input_audio"
                | "file"
                | "image"
        )
    ) {
        Some(vec![content.clone()])
    } else {
        None
    };
    let Some(blocks) = blocks else {
        return Ok(function_response);
    };
    let saw_media = gemini_tool_result_array_media_status(&blocks)?;
    if !saw_media {
        return Ok(function_response);
    }

    let mut result_items = Vec::with_capacity(blocks.len());
    let mut parts = Vec::new();
    let mut media_index = 1usize;
    for block in &blocks {
        let (result_item, part) = gemini_tool_result_block_to_response_and_part(
            block,
            call_id_str,
            media_index,
            target_model,
        )?;
        result_items.push(result_item);
        if let Some(part) = part {
            parts.push(part);
            media_index += 1;
        }
    }
    function_response["response"]["result"] = Value::Array(result_items);
    function_response["parts"] = Value::Array(parts);
    Ok(function_response)
}

fn openai_to_gemini(body: &mut Value, target_model: &str) -> Result<(), String> {
    let controls = openai_normalized_request_controls(body)?;
    let mut result = serde_json::json!({
        "model": if target_model.is_empty() {
            body.get("model").cloned().unwrap_or(serde_json::Value::Null)
        } else {
            Value::String(target_model.to_string())
        },
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
    if let Some(output_shape) = controls.output_shape.as_ref() {
        let generation_config = normalized_output_shape_to_gemini_generation_config(output_shape);
        if let Some(generation_config) = generation_config.as_object() {
            for (key, value) in generation_config {
                result["generationConfig"][key] = value.clone();
            }
        }
    }
    if let Some(audio_contract) = normalized_openai_audio_contract(body)? {
        result["generationConfig"]["responseModalities"] = Value::Array(
            audio_contract
                .response_modalities
                .iter()
                .map(|modality| Value::String(modality.to_ascii_uppercase()))
                .collect(),
        );
        if let Some(voice_name) = audio_contract.voice_name {
            result["generationConfig"]["speechConfig"] = serde_json::json!({
                "voiceConfig": {
                    "prebuiltVoiceConfig": {
                        "voiceName": voice_name
                    }
                }
            });
        }
    }
    if let Some(logprobs) = controls.logprobs.as_ref() {
        if logprobs.enabled || logprobs.top_logprobs.is_some() {
            result["generationConfig"]["responseLogprobs"] = Value::Bool(true);
        }
        if let Some(top_logprobs) = logprobs.top_logprobs.as_ref() {
            result["generationConfig"]["logprobs"] = top_logprobs.clone();
        }
    }
    if let Some(stop) = controls.decoding.stop.as_ref() {
        result["generationConfig"]["stopSequences"] = openai_stop_to_gemini_stop_sequences(stop);
    }
    if let Some(seed) = controls.decoding.seed.as_ref() {
        result["generationConfig"]["seed"] = seed.clone();
    }
    if let Some(presence_penalty) = controls.decoding.presence_penalty.as_ref() {
        result["generationConfig"]["presencePenalty"] = presence_penalty.clone();
    }
    if let Some(frequency_penalty) = controls.decoding.frequency_penalty.as_ref() {
        result["generationConfig"]["frequencyPenalty"] = frequency_penalty.clone();
    }
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .ok_or("missing messages")?;
    let mut tool_name_by_call_id = std::collections::HashMap::new();
    let mut tool_sort_key_by_call_id = std::collections::HashMap::new();
    let mut next_tool_sort_key = 0usize;
    let mut contents: Vec<Value> = Vec::new();
    let mut system_parts: Vec<Value> = Vec::new();
    let mut pending_tool_parts: Vec<(usize, Value)> = Vec::new();
    for msg in messages {
        let role = msg.get("role").and_then(Value::as_str).unwrap_or("user");
        if role != "tool" {
            flush_pending_gemini_function_responses(&mut contents, &mut pending_tool_parts);
        }
        if role == "system" || role == "developer" {
            system_parts.extend(openai_content_to_gemini_parts(msg.get("content"))?);
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
            let call_id_str = msg.get("tool_call_id").and_then(Value::as_str);
            let function_name = msg
                .get("tool_call_id")
                .and_then(Value::as_str)
                .and_then(|id| tool_name_by_call_id.get(id).cloned())
                .or_else(|| msg.get("name").cloned())
                .unwrap_or_else(|| msg.get("tool_call_id").cloned().unwrap_or(Value::Null));
            let sort_key = call_id_str
                .and_then(|id| tool_sort_key_by_call_id.get(id).copied())
                .unwrap_or(next_tool_sort_key + pending_tool_parts.len());
            let function_response =
                tool_message_to_gemini_function_response(msg, function_name.clone(), target_model)?;
            pending_tool_parts.push((
                sort_key,
                serde_json::json!({
                    "functionResponse": function_response
                }),
            ));
        }
    }
    flush_pending_gemini_function_responses(&mut contents, &mut pending_tool_parts);
    if !system_parts.is_empty() {
        result["systemInstruction"] = serde_json::json!({
            "role": "user",
            "parts": system_parts
        });
    }
    result["contents"] = Value::Array(contents);
    let portable_tools = openai_portable_function_tools(
        body,
        controls.restricted_tool_names.as_deref(),
        "tool_choice.allowed_tools",
    )?;
    if !portable_tools.is_empty() {
        let mut decls = vec![];
        for t in &portable_tools {
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
    if let Some(tool_policy) = controls.tool_policy.as_ref() {
        result["toolConfig"]["functionCallingConfig"] =
            normalized_tool_policy_to_gemini_function_calling_config(tool_policy);
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
                        "custom": {
                            "name": "code_exec",
                            "input": "print('hi')"
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
        assert_eq!(messages[1]["tool_calls"][0]["custom"]["name"], "code_exec");
        assert_eq!(messages[1]["tool_calls"][0]["custom"]["input"], "print('hi')");
        assert_eq!(messages[2]["role"], "tool");
        assert_eq!(messages[2]["tool_call_id"], "call_custom");
        assert_eq!(messages[2]["content"], "exit 0");
    }

    #[test]
    fn translate_request_responses_standalone_custom_tool_output_to_non_openai_rejects() {
        for upstream_format in [UpstreamFormat::Anthropic, UpstreamFormat::Google] {
            let mut body = json!({
                "model": "gpt-4o",
                "input": [{
                    "type": "custom_tool_call_output",
                    "call_id": "call_custom",
                    "output": "exit 0"
                }]
            });

            let err = translate_request(
                UpstreamFormat::OpenAiResponses,
                upstream_format,
                "target-model",
                &mut body,
                false,
            )
            .expect_err("standalone custom tool outputs should fail closed");

            assert!(err.contains("custom tools"), "err = {err}");
        }
    }

    #[test]
    fn translate_request_responses_tool_output_text_arrays_to_openai_text_parts() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": [
                {
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "lookup_weather",
                    "arguments": "{\"city\":\"Tokyo\"}"
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_1",
                    "output": [
                        { "type": "input_text", "text": "Sunny" },
                        { "type": "input_text", "text": " 24C" }
                    ]
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
        let content = messages[1]["content"].as_array().expect("tool content");
        assert_eq!(messages[1]["role"], "tool");
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "Sunny");
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[1]["text"], " 24C");
    }

    #[test]
    fn translate_request_responses_tool_output_media_arrays_to_openai_rejects() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": [
                {
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "inspect_media",
                    "arguments": "{}"
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_1",
                    "output": [
                        {
                            "type": "input_image",
                            "image_url": "https://example.com/cat.png"
                        }
                    ]
                }
            ]
        });

        let err = translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            false,
        )
        .expect_err("Responses tool output media arrays should fail closed on Chat targets");

        assert!(err.contains("tool output"), "err = {err}");
        assert!(err.contains("input_image"), "err = {err}");
    }

    #[test]
    fn translate_request_responses_to_openai_preserves_custom_tools_and_custom_tool_choice() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": "run this",
            "tools": [{
                "type": "custom",
                "name": "code_exec",
                "description": "Executes code",
                "format": { "type": "text" }
            }],
            "tool_choice": {
                "type": "custom",
                "name": "code_exec"
            }
        });

        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        let tools = body["tools"].as_array().expect("chat tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "custom");
        assert_eq!(tools[0]["custom"]["name"], "code_exec");
        assert_eq!(tools[0]["custom"]["description"], "Executes code");
        assert_eq!(tools[0]["custom"]["format"]["type"], "text");
        assert_eq!(body["tool_choice"]["type"], "custom");
        assert_eq!(body["tool_choice"]["custom"]["name"], "code_exec");
    }

    #[test]
    fn translate_request_responses_to_openai_preserves_custom_allowed_tools_shape() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": "run this",
            "tools": [
                {
                    "type": "custom",
                    "name": "code_exec",
                    "description": "Executes code"
                },
                {
                    "type": "function",
                    "name": "lookup_weather",
                    "parameters": { "type": "object" }
                }
            ],
            "tool_choice": {
                "type": "allowed_tools",
                "mode": "required",
                "tools": [
                    { "type": "custom", "name": "code_exec" },
                    { "type": "function", "name": "lookup_weather" }
                ]
            }
        });

        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        let tools = body["tools"].as_array().expect("chat tools");
        assert_eq!(tools[0]["type"], "custom");
        assert_eq!(tools[0]["custom"]["name"], "code_exec");
        assert_eq!(tools[1]["type"], "function");
        assert_eq!(body["tool_choice"]["type"], "allowed_tools");
        let allowed_tools = body["tool_choice"]["allowed_tools"]["tools"]
            .as_array()
            .expect("allowed tools");
        assert_eq!(allowed_tools[0]["type"], "custom");
        assert_eq!(allowed_tools[0]["custom"]["name"], "code_exec");
        assert_eq!(allowed_tools[1]["type"], "function");
        assert_eq!(allowed_tools[1]["function"]["name"], "lookup_weather");
    }

    #[test]
    fn translate_request_responses_to_non_responses_rejects_namespace_tool_groups() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": "run this",
            "tools": [{
                "type": "namespace",
                "name": "crm",
                "description": "CRM tools",
                "tools": [{
                    "type": "custom",
                    "name": "lookup_account"
                }]
            }]
        });

        let err = translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            false,
        )
        .expect_err("Responses namespace tools should fail closed");

        assert!(err.contains("namespace"), "err = {err}");
    }

    #[test]
    fn translate_request_responses_to_non_responses_rejects_namespaced_tool_calls() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": [{
                "type": "custom_tool_call",
                "call_id": "call_custom",
                "name": "lookup_account",
                "namespace": "crm",
                "input": "account_id=123"
            }]
        });

        let err = translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            false,
        )
        .expect_err("Responses namespaced tool calls should fail closed");

        assert!(err.contains("namespace"), "err = {err}");
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
        assert_eq!(
            content[2]["file"]["file_data"],
            "data:application/pdf;base64,JVBERi0x"
        );
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
    fn translate_request_openai_to_gemini_rejects_plain_base64_file_data_without_mime_or_provenance(
    ) {
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
    fn translate_request_responses_to_openai_maps_shared_controls_and_drops_responses_only_fields(
    ) {
        let mut body = json!({
            "model": "gpt-4o",
            "input": "Hello",
            "stream": true,
            "max_output_tokens": 123,
            "metadata": { "trace_id": "abc" },
            "user": "user-123",
            "temperature": 0.2,
            "top_p": 0.8,
            "top_logprobs": 5,
            "service_tier": "priority",
            "stream_options": { "include_obfuscation": false },
            "include": ["reasoning.encrypted_content"],
            "text": {
                "format": { "type": "text" },
                "verbosity": "high"
            },
            "reasoning": { "effort": "medium" },
            "max_tool_calls": 2,
            "prompt_cache_key": "cache-key",
            "prompt_cache_retention": "24h",
            "safety_identifier": "safe-user",
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
        assert_eq!(body["metadata"]["trace_id"], "abc");
        assert_eq!(body["user"], "user-123");
        assert_eq!(body["temperature"], 0.2);
        assert_eq!(body["top_p"], 0.8);
        assert_eq!(body["logprobs"], true);
        assert_eq!(body["top_logprobs"], 5);
        assert_eq!(body["service_tier"], "priority");
        assert_eq!(body["stream_options"]["include_obfuscation"], false);
        assert_eq!(body["verbosity"], "high");
        assert_eq!(body["reasoning_effort"], "medium");
        assert_eq!(body["prompt_cache_key"], "cache-key");
        assert_eq!(body["prompt_cache_retention"], "24h");
        assert_eq!(body["safety_identifier"], "safe-user");
        assert_eq!(body["response_format"]["type"], "text");
        assert!(body.get("include").is_none());
        assert!(body.get("text").is_none());
        assert!(body.get("reasoning").is_none());
        assert!(body.get("max_tool_calls").is_none());
        assert!(body.get("truncation").is_none());
    }

    #[test]
    fn translate_request_responses_to_openai_drops_stop_request_extension() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": "Hello",
            "stop": ["END"]
        });

        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        assert!(body.get("stop").is_none(), "body = {body:?}");
    }

    #[test]
    fn translate_request_responses_to_openai_drops_undocumented_sampling_controls() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": "Hello",
            "seed": 42,
            "presence_penalty": 0.7,
            "frequency_penalty": 0.3
        });

        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        for field in ["seed", "presence_penalty", "frequency_penalty"] {
            assert!(body.get(field).is_none(), "field = {field}, body = {body:?}");
        }
    }

    #[test]
    fn translate_request_responses_to_non_responses_rejects_hosted_tool_choice_selectors() {
        for upstream_format in [UpstreamFormat::OpenAiCompletion, UpstreamFormat::Anthropic] {
            let mut body = json!({
                "model": "gpt-4o",
                "input": "Hello",
                "tool_choice": { "type": "file_search" }
            });

            let err = translate_request(
                UpstreamFormat::OpenAiResponses,
                upstream_format,
                "target-model",
                &mut body,
                false,
            )
            .expect_err("Responses hosted tool_choice selectors should fail closed");

            assert!(err.contains("tool_choice"), "err = {err}");
            assert!(err.contains("file_search"), "err = {err}");
        }
    }

    #[test]
    fn translate_request_responses_to_non_responses_rejects_hosted_tool_items() {
        let item_cases = [
            (
                "file_search_call",
                json!({ "type": "file_search_call", "id": "fsc_1", "queries": ["weather"] }),
            ),
            (
                "computer_call_output",
                json!({
                    "type": "computer_call_output",
                    "call_id": "comp_1",
                    "output": { "type": "image", "image_url": "https://example.com/screenshot.png" }
                }),
            ),
        ];

        for (label, item) in item_cases {
            for upstream_format in [UpstreamFormat::OpenAiCompletion, UpstreamFormat::Anthropic] {
                let mut body = json!({
                    "model": "gpt-4o",
                    "input": [item.clone()]
                });

                let err = translate_request(
                    UpstreamFormat::OpenAiResponses,
                    upstream_format,
                    "target-model",
                    &mut body,
                    false,
                )
                .expect_err("Responses hosted input items should fail closed");

                assert!(err.contains(label), "label = {label}, err = {err}");
            }
        }
    }

    #[test]
    fn translate_request_responses_to_non_responses_rejects_item_reference_items() {
        for upstream_format in [UpstreamFormat::OpenAiCompletion, UpstreamFormat::Anthropic] {
            let mut body = json!({
                "model": "gpt-4o",
                "input": [{ "type": "item_reference", "id": "msg_123" }]
            });

            let err = translate_request(
                UpstreamFormat::OpenAiResponses,
                upstream_format,
                "target-model",
                &mut body,
                false,
            )
            .expect_err("Responses item_reference should fail closed cross-protocol");

            assert!(err.contains("item_reference"), "err = {err}");
        }
    }

    #[test]
    fn translate_request_responses_to_non_responses_rejects_reasoning_encrypted_content_items() {
        for upstream_format in [UpstreamFormat::OpenAiCompletion, UpstreamFormat::Anthropic] {
            let mut body = json!({
                "model": "gpt-4o",
                "input": [{
                    "type": "reasoning",
                    "summary": [{ "type": "summary_text", "text": "thinking" }],
                    "encrypted_content": "enc_123"
                }]
            });

            let err = translate_request(
                UpstreamFormat::OpenAiResponses,
                upstream_format,
                "target-model",
                &mut body,
                false,
            )
            .expect_err("Responses reasoning encrypted_content should fail closed cross-protocol");

            assert!(err.contains("encrypted_content"), "err = {err}");
        }
    }

    #[test]
    fn translate_request_responses_to_openai_preserves_function_tool_strict() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": "Hello",
            "tools": [{
                "type": "function",
                "name": "lookup_weather",
                "description": "Weather lookup",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "city": { "type": "string" }
                    }
                },
                "strict": true
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

        let tools = body["tools"].as_array().expect("chat tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["function"]["name"], "lookup_weather");
        assert_eq!(tools[0]["function"]["strict"], true);
    }

    #[test]
    fn translate_request_responses_to_openai_rejects_stateful_responses_controls() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": "Hello",
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
            "Responses request controls `previous_response_id` require a native OpenAI Responses upstream and cannot be translated to openai-completion; the proxy does not reconstruct provider state"
        );
        assert_eq!(body["previous_response_id"], "resp_123");
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
    fn translate_request_openai_to_claude_rejects_reasoning_without_provenance_before_mutating_blocks_or_cache_control(
    ) {
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
    fn translate_request_openai_allowed_tools_to_claude_filters_function_subset() {
        let mut body = json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "tools": [
                {
                    "type": "function",
                    "function": {
                        "name": "lookup_weather",
                        "parameters": { "type": "object", "properties": {} }
                    }
                },
                {
                    "type": "function",
                    "function": {
                        "name": "lookup_time",
                        "parameters": { "type": "object", "properties": {} }
                    }
                },
                {
                    "type": "function",
                    "function": {
                        "name": "lookup_news",
                        "parameters": { "type": "object", "properties": {} }
                    }
                }
            ],
            "tool_choice": {
                "type": "allowed_tools",
                "allowed_tools": {
                    "mode": "required",
                    "tools": [
                        { "type": "function", "function": { "name": "lookup_weather" } },
                        { "type": "function", "function": { "name": "lookup_time" } }
                    ]
                }
            }
        });

        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Anthropic,
            "claude-3",
            &mut body,
            false,
        )
        .unwrap();

        assert_eq!(body["tool_choice"]["type"], "any");
        let tools = body["tools"].as_array().expect("claude tools");
        assert_eq!(tools.len(), 2, "body = {body:?}");
        assert_eq!(tools[0]["name"], "lookup_weather");
        assert_eq!(tools[1]["name"], "lookup_time");
    }

    #[test]
    fn translate_request_responses_allowed_tools_to_claude_filters_function_subset() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": "Hi",
            "tools": [
                {
                    "type": "function",
                    "name": "lookup_weather",
                    "parameters": { "type": "object", "properties": {} }
                },
                {
                    "type": "function",
                    "name": "lookup_time",
                    "parameters": { "type": "object", "properties": {} }
                },
                {
                    "type": "function",
                    "name": "lookup_news",
                    "parameters": { "type": "object", "properties": {} }
                }
            ],
            "tool_choice": {
                "type": "allowed_tools",
                "mode": "auto",
                "tools": [
                    { "type": "function", "name": "lookup_weather" },
                    { "type": "function", "name": "lookup_time" }
                ]
            }
        });

        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Anthropic,
            "claude-3",
            &mut body,
            false,
        )
        .unwrap();

        assert_eq!(body["tool_choice"]["type"], "auto");
        let tools = body["tools"].as_array().expect("claude tools");
        assert_eq!(tools.len(), 2, "body = {body:?}");
        assert_eq!(tools[0]["name"], "lookup_weather");
        assert_eq!(tools[1]["name"], "lookup_time");
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
            "gemini-3-pro",
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
            "gemini-3-pro",
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
            "tools": [
                {
                    "type": "function",
                    "function": {
                        "name": "lookup_weather",
                        "parameters": { "type": "object", "properties": {} }
                    }
                },
                {
                    "type": "function",
                    "function": {
                        "name": "lookup_time",
                        "parameters": { "type": "object", "properties": {} }
                    }
                }
            ],
            "tool_choice": "required",
            "allowed_tool_names": ["lookup_weather", "lookup_time"]
        });
        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-3-pro",
            &mut body,
            false,
        )
        .unwrap();

        assert_eq!(body["toolConfig"]["functionCallingConfig"]["mode"], "ANY");
        assert!(
            body["toolConfig"]["functionCallingConfig"]
                .get("allowedFunctionNames")
                .is_none(),
            "body = {body:?}"
        );
        let declarations = body["tools"][0]["functionDeclarations"]
            .as_array()
            .expect("function declarations");
        assert_eq!(declarations.len(), 2, "body = {body:?}");
        assert_eq!(declarations[0]["name"], "lookup_weather");
        assert_eq!(declarations[1]["name"], "lookup_time");
    }

    #[test]
    fn translate_request_openai_allowed_tools_to_gemini_filters_function_subset() {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "Hi" }],
            "tools": [
                {
                    "type": "function",
                    "function": {
                        "name": "lookup_weather",
                        "parameters": { "type": "object", "properties": {} }
                    }
                },
                {
                    "type": "function",
                    "function": {
                        "name": "lookup_time",
                        "parameters": { "type": "object", "properties": {} }
                    }
                },
                {
                    "type": "function",
                    "function": {
                        "name": "lookup_news",
                        "parameters": { "type": "object", "properties": {} }
                    }
                }
            ],
            "tool_choice": {
                "type": "allowed_tools",
                "allowed_tools": {
                    "mode": "required",
                    "tools": [
                        { "type": "function", "function": { "name": "lookup_weather" } },
                        { "type": "function", "function": { "name": "lookup_time" } }
                    ]
                }
            }
        });

        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-3-pro",
            &mut body,
            false,
        )
        .unwrap();

        assert_eq!(body["toolConfig"]["functionCallingConfig"]["mode"], "ANY");
        assert!(
            body["toolConfig"]["functionCallingConfig"]
                .get("allowedFunctionNames")
                .is_none(),
            "body = {body:?}"
        );
        let declarations = body["tools"][0]["functionDeclarations"]
            .as_array()
            .expect("function declarations");
        assert_eq!(declarations.len(), 2, "body = {body:?}");
        assert_eq!(declarations[0]["name"], "lookup_weather");
        assert_eq!(declarations[1]["name"], "lookup_time");
    }

    #[test]
    fn translate_request_responses_allowed_tools_to_gemini_filters_function_subset() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": "Hi",
            "tools": [
                {
                    "type": "function",
                    "name": "lookup_weather",
                    "parameters": { "type": "object", "properties": {} }
                },
                {
                    "type": "function",
                    "name": "lookup_time",
                    "parameters": { "type": "object", "properties": {} }
                },
                {
                    "type": "function",
                    "name": "lookup_news",
                    "parameters": { "type": "object", "properties": {} }
                }
            ],
            "tool_choice": {
                "type": "allowed_tools",
                "mode": "auto",
                "tools": [
                    { "type": "function", "name": "lookup_weather" },
                    { "type": "function", "name": "lookup_time" }
                ]
            }
        });

        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Google,
            "gemini-3-pro",
            &mut body,
            false,
        )
        .unwrap();

        assert_eq!(body["toolConfig"]["functionCallingConfig"]["mode"], "AUTO");
        let declarations = body["tools"][0]["functionDeclarations"]
            .as_array()
            .expect("function declarations");
        assert_eq!(declarations.len(), 2, "body = {body:?}");
        assert_eq!(declarations[0]["name"], "lookup_weather");
        assert_eq!(declarations[1]["name"], "lookup_time");
    }

    #[test]
    fn translate_request_allowed_tools_to_non_gemini_targets_rejects_unresolved_selector() {
        for upstream_format in [UpstreamFormat::Google, UpstreamFormat::Anthropic] {
            let mut body = json!({
                "model": "gpt-4o",
                "messages": [{ "role": "user", "content": "Hi" }],
                "tools": [{
                    "type": "function",
                    "function": {
                        "name": "lookup_weather",
                        "parameters": { "type": "object", "properties": {} }
                    }
                }],
                "tool_choice": {
                    "type": "allowed_tools",
                    "allowed_tools": {
                        "mode": "required",
                        "tools": [
                            { "type": "function", "function": { "name": "lookup_time" } }
                        ]
                    }
                }
            });

            let err = translate_request(
                UpstreamFormat::OpenAiCompletion,
                upstream_format,
                "target-model",
                &mut body,
                false,
            )
            .expect_err("unresolved allowed_tools selector should fail closed");

            assert!(err.contains("allowed_tools"), "err = {err}");
            assert!(err.contains("lookup_time"), "err = {err}");
        }
    }

    #[test]
    fn translate_request_allowed_tools_to_non_gemini_targets_rejects_non_function_selection() {
        for upstream_format in [UpstreamFormat::Google, UpstreamFormat::Anthropic] {
            let mut body = json!({
                "model": "gpt-4o",
                "messages": [{ "role": "user", "content": "Hi" }],
                "tool_choice": {
                    "type": "allowed_tools",
                    "allowed_tools": {
                        "mode": "required",
                        "tools": [
                            { "type": "custom", "name": "code_exec" }
                        ]
                    }
                }
            });

            let err = translate_request(
                UpstreamFormat::OpenAiCompletion,
                upstream_format,
                "target-model",
                &mut body,
                false,
            )
            .expect_err("non-function allowed_tools selection should fail closed");

            assert!(
                err.contains("allowed_tools")
                    || err.contains("custom tools")
                    || err.contains("custom tool"),
                "err = {err}"
            );
        }
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
    fn translate_request_openai_to_gemini_maps_logprobs_controls() {
        let mut body = json!({
            "model": "gemini-1.5",
            "messages": [{ "role": "user", "content": "Hi" }],
            "logprobs": true,
            "top_logprobs": 5
        });

        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-1.5",
            &mut body,
            false,
        )
        .unwrap();

        assert_eq!(body["generationConfig"]["responseLogprobs"], true);
        assert_eq!(body["generationConfig"]["logprobs"], 5);
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
            "gemini-3-pro",
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
    fn translate_request_openai_to_responses_maps_shared_controls_and_normalizes_legacy_allowlist()
    {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "Hi" }],
            "metadata": { "trace_id": "abc" },
            "user": "user-123",
            "service_tier": "priority",
            "stream_options": { "include_obfuscation": true },
            "verbosity": "low",
            "reasoning_effort": "high",
            "tool_choice": "required",
            "allowed_tool_names": ["lookup_weather", "lookup_time"],
            "tools": [
                {
                    "type": "function",
                    "function": {
                        "name": "lookup_weather",
                        "parameters": { "type": "object", "properties": {} }
                    }
                },
                {
                    "type": "function",
                    "function": {
                        "name": "lookup_time",
                        "parameters": { "type": "object", "properties": {} }
                    }
                },
                {
                    "type": "function",
                    "function": {
                        "name": "lookup_news",
                        "parameters": { "type": "object", "properties": {} }
                    }
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

        assert_eq!(body["metadata"]["trace_id"], "abc");
        assert_eq!(body["user"], "user-123");
        assert_eq!(body["service_tier"], "priority");
        assert_eq!(body["stream_options"]["include_obfuscation"], true);
        assert_eq!(body["text"]["verbosity"], "low");
        assert_eq!(body["reasoning"]["effort"], "high");
        assert!(body.get("allowed_tool_names").is_none(), "body = {body:?}");
        assert_eq!(body["tool_choice"]["type"], "allowed_tools");
        assert_eq!(body["tool_choice"]["mode"], "required");
        let tools = body["tool_choice"]["tools"].as_array().expect("allowed tools");
        assert_eq!(tools.len(), 2, "body = {body:?}");
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["name"], "lookup_weather");
        assert_eq!(tools[1]["name"], "lookup_time");
    }

    #[test]
    fn translate_request_openai_custom_tool_to_responses_preserves_custom_type() {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "Hi" }],
            "tools": [{
                "type": "custom",
                "custom": {
                    "name": "code_exec",
                    "description": "Executes code with provider-managed semantics",
                    "format": { "type": "text" }
                }
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
        assert_eq!(
            tools[0]["description"],
            "Executes code with provider-managed semantics"
        );
        assert_eq!(tools[0]["format"]["type"], "text");
    }

    #[test]
    fn translate_request_openai_custom_tool_choice_to_responses_preserves_custom_type() {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "Hi" }],
            "tools": [{
                "type": "custom",
                "custom": {
                    "name": "code_exec",
                    "description": "Executes code with provider-managed semantics",
                    "format": { "type": "text" }
                }
            }],
            "tool_choice": {
                "type": "custom",
                "custom": {
                    "name": "code_exec"
                }
            }
        });

        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        assert_eq!(body["tool_choice"]["type"], "custom");
        assert_eq!(body["tool_choice"]["name"], "code_exec");
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
                    "custom": {
                        "name": "code_exec",
                        "input": "print('hi')"
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
    fn translate_request_openai_tool_text_part_arrays_to_responses_output_arrays() {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [
                {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "lookup_weather",
                            "arguments": "{\"city\":\"Tokyo\"}"
                        }
                    }]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_1",
                    "content": [
                        { "type": "text", "text": "Sunny" },
                        { "type": "text", "text": " 24C" }
                    ]
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
        let output = input[1]["output"].as_array().expect("tool output");
        assert_eq!(input[1]["type"], "function_call_output");
        assert_eq!(output.len(), 2);
        assert_eq!(output[0]["type"], "input_text");
        assert_eq!(output[0]["text"], "Sunny");
        assert_eq!(output[1]["type"], "input_text");
        assert_eq!(output[1]["text"], " 24C");
    }

    #[test]
    fn translate_request_openai_custom_allowed_tools_to_responses_preserves_custom_shape() {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "Hi" }],
            "tools": [
                {
                    "type": "custom",
                    "custom": {
                        "name": "code_exec",
                        "description": "Executes code"
                    }
                },
                {
                    "type": "function",
                    "function": {
                        "name": "lookup_weather",
                        "parameters": { "type": "object" }
                    }
                }
            ],
            "tool_choice": {
                "type": "allowed_tools",
                "allowed_tools": {
                    "mode": "required",
                    "tools": [
                        {
                            "type": "custom",
                            "custom": { "name": "code_exec" }
                        },
                        {
                            "type": "function",
                            "function": { "name": "lookup_weather" }
                        }
                    ]
                }
            }
        });

        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        assert_eq!(body["tool_choice"]["type"], "allowed_tools");
        let tools = body["tool_choice"]["tools"].as_array().expect("allowed tools");
        assert_eq!(tools[0]["type"], "custom");
        assert_eq!(tools[0]["name"], "code_exec");
        assert_eq!(tools[1]["type"], "function");
        assert_eq!(tools[1]["name"], "lookup_weather");
    }

    #[test]
    fn translate_request_openai_to_responses_maps_logprobs_to_include_and_drops_chat_only_controls() {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "Hi" }],
            "logprobs": true,
            "top_logprobs": 5,
            "prediction": {
                "type": "content",
                "content": "predicted"
            },
            "web_search_options": {
                "search_context_size": "high"
            },
            "n": 1
        });

        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        let include = body["include"].as_array().expect("responses include");
        assert!(
            include
                .iter()
                .any(|item| item == "message.output_text.logprobs"),
            "body = {body:?}"
        );
        assert_eq!(body["top_logprobs"], 5);
        assert!(body.get("logprobs").is_none(), "body = {body:?}");
        assert!(body.get("prediction").is_none(), "body = {body:?}");
        assert!(body.get("web_search_options").is_none(), "body = {body:?}");
        assert!(body.get("n").is_none(), "body = {body:?}");
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
                    "custom": {
                        "name": "code_exec",
                        "input": "print('hi')"
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
                "custom": {
                    "name": "code_exec",
                    "description": "Executes code with provider-managed semantics"
                }
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
    fn translate_request_chat_to_gemini_rejects_audio_format_without_documented_equivalent() {
        let mut body = json!({
            "model": "gemini-2.5-flash",
            "messages": [{ "role": "user", "content": "Read this aloud" }],
            "modalities": ["text", "audio"],
            "audio": {
                "format": "wav",
                "voice": "alloy"
            }
        });

        let err = translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-2.5-flash",
            &mut body,
            false,
        )
        .expect_err("Chat audio.format should fail closed when Gemini has no documented equivalent");

        assert!(err.contains("audio"), "err = {err}");
        assert!(err.contains("format"), "err = {err}");
    }

    #[test]
    fn translate_request_chat_to_non_gemini_rejects_audio_output_intent() {
        for upstream in [UpstreamFormat::OpenAiResponses, UpstreamFormat::Anthropic] {
            let mut body = json!({
                "model": "gpt-4o-audio-preview",
                "messages": [{ "role": "user", "content": "Read this aloud" }],
                "modalities": ["audio"],
                "audio": {
                    "format": "wav",
                    "voice": "alloy"
                }
            });

            let err = translate_request(
                UpstreamFormat::OpenAiCompletion,
                upstream,
                "gpt-4o-audio-preview",
                &mut body,
                false,
            )
            .expect_err("Chat audio output intent should fail closed for non-Gemini targets");

            assert!(err.contains("audio"), "err = {err}");
        }
    }

    #[test]
    fn translate_request_chat_assistant_audio_history_rejects_on_non_chat_targets() {
        for upstream in [
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Anthropic,
            UpstreamFormat::Google,
        ] {
            let mut body = json!({
                "model": "gpt-4o-audio-preview",
                "messages": [
                    {
                        "role": "assistant",
                        "content": "Earlier spoken reply",
                        "audio": { "id": "audio_123" }
                    },
                    {
                        "role": "user",
                        "content": "Continue"
                    }
                ]
            });

            let err = translate_request(
                UpstreamFormat::OpenAiCompletion,
                upstream,
                "gpt-4o-audio-preview",
                &mut body,
                false,
            )
            .expect_err("assistant audio history should fail closed for non-Chat targets");

            assert!(err.contains("audio"), "err = {err}");
        }
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
    fn translate_request_gemini_to_openai_maps_snake_case_request_fields_and_allowed_tools() {
        let mut body = json!({
            "model": "gemini-1.5",
            "system_instruction": {
                "parts": [{ "text": "You are helpful." }]
            },
            "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
            "generation_config": {
                "max_output_tokens": 222,
                "temperature": 0.3,
                "top_p": 0.9
            },
            "tools": [{
                "function_declarations": [
                    {
                        "name": "lookup_weather",
                        "description": "Weather lookup",
                        "parameters": { "type": "object", "properties": {} }
                    },
                    {
                        "name": "lookup_time",
                        "description": "Time lookup",
                        "parameters": { "type": "object", "properties": {} }
                    },
                    {
                        "name": "lookup_news",
                        "description": "News lookup",
                        "parameters": { "type": "object", "properties": {} }
                    }
                ]
            }],
            "tool_config": {
                "function_calling_config": {
                    "mode": "ANY",
                    "allowed_function_names": ["lookup_weather", "lookup_time"]
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

        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "You are helpful.");
        assert_eq!(body["max_tokens"], 222);
        assert_eq!(body["temperature"], 0.3);
        assert_eq!(body["top_p"], 0.9);
        assert_eq!(body["tools"][0]["function"]["name"], "lookup_weather");
        assert_eq!(body["tools"][1]["function"]["name"], "lookup_time");
        assert_eq!(body["tool_choice"], "required");
        assert_eq!(body["tools"].as_array().unwrap().len(), 2);
        assert!(body.get("allowed_tool_names").is_none(), "body = {body:?}");
    }

    #[test]
    fn translate_request_gemini_to_responses_maps_snake_case_request_fields_and_allowed_tools() {
        let mut body = json!({
            "model": "gemini-1.5",
            "system_instruction": {
                "parts": [{ "text": "You are helpful." }]
            },
            "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
            "generation_config": {
                "max_output_tokens": 222,
                "temperature": 0.3,
                "top_p": 0.9
            },
            "tools": [{
                "function_declarations": [
                    {
                        "name": "lookup_weather",
                        "description": "Weather lookup",
                        "parameters": { "type": "object", "properties": {} }
                    },
                    {
                        "name": "lookup_time",
                        "description": "Time lookup",
                        "parameters": { "type": "object", "properties": {} }
                    },
                    {
                        "name": "lookup_news",
                        "description": "News lookup",
                        "parameters": { "type": "object", "properties": {} }
                    }
                ]
            }],
            "tool_config": {
                "function_calling_config": {
                    "mode": "ANY",
                    "allowed_function_names": ["lookup_weather", "lookup_time"]
                }
            }
        });
        translate_request(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiResponses,
            "gemini-1.5",
            &mut body,
            false,
        )
        .unwrap();

        assert_eq!(body["input"][0]["role"], "system");
        assert_eq!(body["input"][0]["content"][0]["text"], "You are helpful.");
        assert_eq!(body["max_output_tokens"], 222);
        assert_eq!(body["temperature"], 0.3);
        assert_eq!(body["top_p"], 0.9);
        assert_eq!(body["tools"][0]["name"], "lookup_weather");
        assert_eq!(body["tools"][1]["name"], "lookup_time");
        assert_eq!(body["tool_choice"], "required");
        assert_eq!(body["tools"].as_array().unwrap().len(), 2);
        assert!(body.get("allowed_tool_names").is_none(), "body = {body:?}");
    }

    #[test]
    fn translate_request_gemini_to_openai_maps_single_allowed_function_to_forced_function_choice() {
        let mut body = json!({
            "model": "gemini-1.5",
            "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
            "tools": [{
                "functionDeclarations": [
                    {
                        "name": "lookup_weather",
                        "description": "Weather lookup",
                        "parameters": { "type": "object", "properties": {} }
                    },
                    {
                        "name": "lookup_time",
                        "description": "Time lookup",
                        "parameters": { "type": "object", "properties": {} }
                    }
                ]
            }],
            "toolConfig": {
                "functionCallingConfig": {
                    "mode": "ANY",
                    "allowedFunctionNames": ["lookup_weather"]
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

        assert_eq!(body["tool_choice"]["type"], "function");
        assert_eq!(body["tool_choice"]["function"]["name"], "lookup_weather");
        let tools = body["tools"].as_array().expect("chat tools");
        assert_eq!(tools.len(), 1, "body = {body:?}");
        assert_eq!(tools[0]["function"]["name"], "lookup_weather");
    }

    #[test]
    fn translate_request_gemini_to_responses_maps_single_allowed_function_to_forced_function_choice(
    ) {
        let mut body = json!({
            "model": "gemini-1.5",
            "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
            "tools": [{
                "functionDeclarations": [
                    {
                        "name": "lookup_weather",
                        "description": "Weather lookup",
                        "parameters": { "type": "object", "properties": {} }
                    },
                    {
                        "name": "lookup_time",
                        "description": "Time lookup",
                        "parameters": { "type": "object", "properties": {} }
                    }
                ]
            }],
            "toolConfig": {
                "functionCallingConfig": {
                    "mode": "ANY",
                    "allowedFunctionNames": ["lookup_weather"]
                }
            }
        });

        translate_request(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiResponses,
            "gemini-1.5",
            &mut body,
            false,
        )
        .unwrap();

        assert_eq!(body["tool_choice"]["type"], "function");
        assert_eq!(body["tool_choice"]["name"], "lookup_weather");
        let tools = body["tools"].as_array().expect("responses tools");
        assert_eq!(tools.len(), 1, "body = {body:?}");
        assert_eq!(tools[0]["name"], "lookup_weather");
    }

    #[test]
    fn translate_request_gemini_to_openai_maps_json_object_output_shape_control() {
        let mut body = json!({
            "model": "gemini-1.5",
            "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
            "generationConfig": {
                "responseMimeType": "application/json"
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

        assert_eq!(body["response_format"]["type"], "json_object");
    }

    #[test]
    fn translate_request_gemini_to_responses_maps_json_object_output_shape_control() {
        let mut body = json!({
            "model": "gemini-1.5",
            "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
            "generation_config": {
                "response_mime_type": "application/json"
            }
        });

        translate_request(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiResponses,
            "gemini-1.5",
            &mut body,
            false,
        )
        .unwrap();

        assert_eq!(body["text"]["format"]["type"], "json_object");
    }

    #[test]
    fn translate_request_gemini_to_anthropic_maps_snake_case_request_fields_without_losing_allowlist(
    ) {
        let mut body = json!({
            "model": "gemini-1.5",
            "system_instruction": {
                "parts": [{ "text": "You are helpful." }]
            },
            "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
            "generation_config": {
                "max_output_tokens": 222,
                "temperature": 0.3,
                "top_p": 0.9
            },
            "tools": [{
                "function_declarations": [
                    {
                        "name": "lookup_weather",
                        "description": "Weather lookup",
                        "parameters": { "type": "object", "properties": {} }
                    },
                    {
                        "name": "lookup_time",
                        "description": "Time lookup",
                        "parameters": { "type": "object", "properties": {} }
                    },
                    {
                        "name": "lookup_news",
                        "description": "News lookup",
                        "parameters": { "type": "object", "properties": {} }
                    }
                ]
            }],
            "tool_config": {
                "function_calling_config": {
                    "mode": "ANY",
                    "allowed_function_names": ["lookup_weather", "lookup_time"]
                }
            }
        });
        translate_request(
            UpstreamFormat::Google,
            UpstreamFormat::Anthropic,
            "gemini-1.5",
            &mut body,
            false,
        )
        .unwrap();

        assert_eq!(body["system"][0]["text"], "You are helpful.");
        assert_eq!(body["max_tokens"], 222);
        assert_eq!(body["temperature"], 0.3);
        assert_eq!(body["top_p"], 0.9);
        let tools = body["tools"].as_array().expect("anthropic tools");
        assert_eq!(tools.len(), 2, "body = {body:?}");
        assert_eq!(tools[0]["name"], "lookup_weather");
        assert_eq!(tools[1]["name"], "lookup_time");
        assert_eq!(body["tool_choice"]["type"], "any");
    }

    #[test]
    fn translate_request_gemini_to_non_gemini_rejects_built_in_and_server_side_tools() {
        let tool_cases = [
            ("googleSearch", json!({ "googleSearch": {} })),
            ("codeExecution", json!({ "codeExecution": {} })),
            ("computerUse", json!({ "computerUse": {} })),
            (
                "mcpServers",
                json!({ "mcpServers": [{ "server": "https://mcp.example" }] }),
            ),
        ];

        for (label, tool) in tool_cases {
            for upstream_format in [
                UpstreamFormat::OpenAiCompletion,
                UpstreamFormat::OpenAiResponses,
                UpstreamFormat::Anthropic,
            ] {
                let mut body = json!({
                    "model": "gemini-1.5",
                    "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
                    "tools": [tool.clone()]
                });

                let err = translate_request(
                    UpstreamFormat::Google,
                    upstream_format,
                    "gemini-1.5",
                    &mut body,
                    false,
                )
                .expect_err("Gemini built-in/server-side tools should fail closed");

                assert!(err.contains(label), "label = {label}, err = {err}");
            }
        }
    }

    #[test]
    fn translate_request_gemini_to_non_gemini_rejects_nonportable_tool_config_controls() {
        let config_cases = [
            (
                "includeServerSideToolInvocations",
                json!({
                    "functionCallingConfig": { "mode": "ANY" },
                    "includeServerSideToolInvocations": true
                }),
            ),
            (
                "retrievalConfig",
                json!({
                    "functionCallingConfig": { "mode": "ANY" },
                    "retrievalConfig": { "languageCode": "en" }
                }),
            ),
            (
                "VALIDATED",
                json!({
                    "functionCallingConfig": {
                        "mode": "VALIDATED",
                        "allowedFunctionNames": ["lookup_weather"]
                    }
                }),
            ),
        ];

        for (label, tool_config) in config_cases {
            for upstream_format in [
                UpstreamFormat::OpenAiCompletion,
                UpstreamFormat::OpenAiResponses,
                UpstreamFormat::Anthropic,
            ] {
                let mut body = json!({
                    "model": "gemini-1.5",
                    "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
                    "tools": [{
                        "functionDeclarations": [{
                            "name": "lookup_weather",
                            "description": "Weather lookup",
                            "parameters": { "type": "object", "properties": {} }
                        }]
                    }],
                    "toolConfig": tool_config.clone()
                });

                let err = translate_request(
                    UpstreamFormat::Google,
                    upstream_format,
                    "gemini-1.5",
                    &mut body,
                    false,
                )
                .expect_err("Gemini non-portable tool config should fail closed");

                assert!(err.contains(label), "label = {label}, err = {err}");
            }
        }
    }

    #[test]
    fn translate_request_gemini_to_non_gemini_allows_pure_function_tools_and_portable_modes() {
        let cases = [
            ("AUTO", json!({ "mode": "AUTO" })),
            ("NONE", json!({ "mode": "NONE" })),
            (
                "ANY",
                json!({
                    "mode": "ANY",
                    "allowedFunctionNames": ["lookup_weather"]
                }),
            ),
        ];

        for (label, function_calling_config) in cases {
            for upstream_format in [
                UpstreamFormat::OpenAiCompletion,
                UpstreamFormat::OpenAiResponses,
                UpstreamFormat::Anthropic,
            ] {
                let mut body = json!({
                    "model": "gemini-1.5",
                    "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
                    "tools": [{
                        "functionDeclarations": [{
                            "name": "lookup_weather",
                            "description": "Weather lookup",
                            "parameters": {
                                "type": "object",
                                "properties": {
                                    "city": { "type": "string" }
                                }
                            }
                        }]
                    }],
                    "toolConfig": {
                        "functionCallingConfig": function_calling_config.clone()
                    }
                });

                translate_request(
                    UpstreamFormat::Google,
                    upstream_format,
                    "gemini-1.5",
                    &mut body,
                    false,
                )
                .unwrap_or_else(|err| {
                    panic!("label = {label}, upstream = {upstream_format:?}, err = {err}")
                });
            }
        }
    }

    #[test]
    fn translate_request_gemini_to_non_gemini_preserves_parameters_json_schema() {
        for upstream_format in [
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Anthropic,
        ] {
            let mut body = json!({
                "model": "gemini-1.5",
                "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
                "tools": [{
                    "functionDeclarations": [{
                        "name": "lookup_weather",
                        "description": "Weather lookup",
                        "parametersJsonSchema": {
                            "type": "object",
                            "properties": {
                                "city": { "type": "string" }
                            },
                            "required": ["city"],
                            "additionalProperties": false
                        }
                    }]
                }]
            });

            translate_request(
                UpstreamFormat::Google,
                upstream_format,
                "gemini-1.5",
                &mut body,
                false,
            )
            .unwrap_or_else(|err| {
                panic!("upstream = {upstream_format:?}, err = {err}");
            });

            match upstream_format {
                UpstreamFormat::OpenAiCompletion => {
                    let parameters = &body["tools"][0]["function"]["parameters"];
                    assert_eq!(parameters["properties"]["city"]["type"], "string");
                    assert_eq!(parameters["required"][0], "city");
                    assert_eq!(parameters["additionalProperties"], false);
                }
                UpstreamFormat::OpenAiResponses => {
                    let parameters = &body["tools"][0]["parameters"];
                    assert_eq!(parameters["properties"]["city"]["type"], "string");
                    assert_eq!(parameters["required"][0], "city");
                    assert_eq!(parameters["additionalProperties"], false);
                }
                UpstreamFormat::Anthropic => {
                    let input_schema = &body["tools"][0]["input_schema"];
                    assert_eq!(input_schema["properties"]["city"]["type"], "string");
                    assert_eq!(input_schema["required"][0], "city");
                    assert_eq!(input_schema["additionalProperties"], false);
                }
                UpstreamFormat::Google => unreachable!("non-Gemini loop"),
            }
        }
    }

    #[test]
    fn translate_request_gemini_to_non_gemini_rejects_dual_function_schema_sources() {
        for upstream_format in [
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Anthropic,
        ] {
            let mut body = json!({
                "model": "gemini-1.5",
                "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
                "tools": [{
                    "functionDeclarations": [{
                        "name": "lookup_weather",
                        "description": "Weather lookup",
                        "parameters": {
                            "type": "object",
                            "properties": {
                                "city": { "type": "string" }
                            }
                        },
                        "parametersJsonSchema": {
                            "type": "object",
                            "properties": {
                                "city": { "type": "string" }
                            },
                            "required": ["city"]
                        }
                    }]
                }]
            });

            let err = translate_request(
                UpstreamFormat::Google,
                upstream_format,
                "gemini-1.5",
                &mut body,
                false,
            )
            .expect_err("dual function schema sources should fail closed");

            assert!(err.contains("parameters"), "err = {err}");
            assert!(err.contains("parametersJsonSchema"), "err = {err}");
        }
    }

    #[test]
    fn translate_request_gemini_to_non_gemini_rejects_function_output_schemas() {
        let cases = [
            (
                "response",
                json!({
                    "response": {
                        "type": "object",
                        "properties": {
                            "temperature": { "type": "number" }
                        }
                    }
                }),
            ),
            (
                "responseJsonSchema",
                json!({
                    "responseJsonSchema": {
                        "type": "object",
                        "properties": {
                            "temperature": { "type": "number" }
                        }
                    }
                }),
            ),
        ];

        for (label, extra_fields) in cases {
            for upstream_format in [
                UpstreamFormat::OpenAiCompletion,
                UpstreamFormat::OpenAiResponses,
                UpstreamFormat::Anthropic,
            ] {
                let mut declaration = json!({
                    "name": "lookup_weather",
                    "description": "Weather lookup",
                    "parameters": { "type": "object", "properties": {} }
                });
                declaration
                    .as_object_mut()
                    .expect("declaration object")
                    .extend(
                        extra_fields
                            .as_object()
                            .expect("extra fields object")
                            .clone(),
                    );

                let mut body = json!({
                    "model": "gemini-1.5",
                    "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
                    "tools": [{
                        "functionDeclarations": [declaration]
                    }]
                });

                let err = translate_request(
                    UpstreamFormat::Google,
                    upstream_format,
                    "gemini-1.5",
                    &mut body,
                    false,
                )
                .expect_err("Gemini function output schemas should fail closed");

                assert!(err.contains(label), "label = {label}, err = {err}");
            }
        }
    }

    #[test]
    fn translate_request_gemini_to_non_gemini_rejects_invalid_allowed_function_names() {
        let cases = [
            ("non-string", json!([123])),
            ("empty array", json!([])),
            ("unknown tool", json!(["lookup_unknown"])),
            (
                "mixed valid and unknown",
                json!(["lookup_weather", "lookup_unknown"]),
            ),
            ("mixed valid and empty", json!(["lookup_weather", ""])),
        ];

        for (label, allowed_function_names) in cases {
            for upstream_format in [
                UpstreamFormat::OpenAiCompletion,
                UpstreamFormat::OpenAiResponses,
                UpstreamFormat::Anthropic,
            ] {
                let mut body = json!({
                    "model": "gemini-1.5",
                    "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
                    "tools": [{
                        "function_declarations": [
                            {
                                "name": "lookup_weather",
                                "description": "Weather lookup",
                                "parameters": { "type": "object", "properties": {} }
                            },
                            {
                                "name": "lookup_time",
                                "description": "Time lookup",
                                "parameters": { "type": "object", "properties": {} }
                            }
                        ]
                    }],
                    "tool_config": {
                        "function_calling_config": {
                            "mode": "ANY",
                            "allowed_function_names": allowed_function_names.clone()
                        }
                    }
                });

                let err = translate_request(
                    UpstreamFormat::Google,
                    upstream_format,
                    "gemini-1.5",
                    &mut body,
                    false,
                )
                .expect_err("invalid allowedFunctionNames should fail closed");

                assert!(
                    err.contains("allowedFunctionNames") || err.contains("allowed_function_names"),
                    "label = {label}, err = {err}"
                );
            }
        }
    }

    #[test]
    fn translate_request_gemini_to_openai_maps_json_output_shape_controls() {
        let mut body = json!({
            "model": "gemini-1.5",
            "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
            "generationConfig": {
                "responseMimeType": "application/json",
                "responseJsonSchema": {
                    "type": "object",
                    "properties": {
                        "city": { "type": "string" }
                    },
                    "required": ["city"]
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

        assert_eq!(body["response_format"]["type"], "json_schema");
        assert_eq!(
            body["response_format"]["json_schema"]["schema"]["properties"]["city"]["type"],
            "string"
        );
        assert_eq!(
            body["response_format"]["json_schema"]["schema"]["required"][0],
            "city"
        );
    }

    #[test]
    fn translate_request_gemini_to_responses_maps_json_output_shape_controls() {
        let mut body = json!({
            "model": "gemini-1.5",
            "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
            "generation_config": {
                "response_mime_type": "application/json",
                "response_json_schema": {
                    "type": "object",
                    "properties": {
                        "city": { "type": "string" }
                    },
                    "required": ["city"]
                }
            }
        });

        translate_request(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiResponses,
            "gemini-1.5",
            &mut body,
            false,
        )
        .unwrap();

        assert_eq!(body["text"]["format"]["type"], "json_schema");
        assert_eq!(
            body["text"]["format"]["schema"]["properties"]["city"]["type"],
            "string"
        );
        assert_eq!(body["text"]["format"]["schema"]["required"][0], "city");
    }

    #[test]
    fn translate_request_gemini_to_non_gemini_rejects_nonportable_output_shape_controls() {
        let cases = [
            (
                "responseSchema",
                json!({
                    "responseMimeType": "application/json",
                    "responseSchema": {
                        "type": "object",
                        "properties": {
                            "city": { "type": "string" }
                        }
                    }
                }),
            ),
            (
                "text/x.enum",
                json!({
                    "responseMimeType": "text/x.enum"
                }),
            ),
        ];

        for (label, generation_config) in cases {
            for upstream_format in [
                UpstreamFormat::OpenAiCompletion,
                UpstreamFormat::OpenAiResponses,
            ] {
                let mut body = json!({
                    "model": "gemini-1.5",
                    "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
                    "generationConfig": generation_config.clone()
                });

                let err = translate_request(
                    UpstreamFormat::Google,
                    upstream_format,
                    "gemini-1.5",
                    &mut body,
                    false,
                )
                .expect_err("nonportable Gemini output-shape controls should fail closed");

                assert!(err.contains(label), "label = {label}, err = {err}");
            }
        }
    }

    #[test]
    fn translate_request_openai_to_gemini_maps_response_format_json_schema() {
        let mut body = json!({
            "model": "gemini-2.5-flash",
            "messages": [{ "role": "user", "content": "Hi" }],
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": "weather_response",
                    "schema": {
                        "type": "object",
                        "properties": {
                            "city": { "type": "string" }
                        }
                    }
                }
            }
        });

        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-2.5-flash",
            &mut body,
            false,
        )
        .unwrap();

        assert_eq!(
            body["generationConfig"]["responseMimeType"],
            "application/json"
        );
        assert_eq!(
            body["generationConfig"]["responseJsonSchema"]["properties"]["city"]["type"],
            "string"
        );
    }

    #[test]
    fn translate_request_openai_to_responses_maps_response_format_json_schema() {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "Hi" }],
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": "weather_response",
                    "schema": {
                        "type": "object",
                        "properties": {
                            "city": { "type": "string" }
                        }
                    },
                    "strict": true
                }
            }
        });

        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            "gpt-4o",
            &mut body,
            false,
        )
        .unwrap();

        assert_eq!(body["text"]["format"]["type"], "json_schema");
        assert_eq!(body["text"]["format"]["name"], "weather_response");
        assert_eq!(
            body["text"]["format"]["schema"]["properties"]["city"]["type"],
            "string"
        );
        assert_eq!(body["text"]["format"]["strict"], true);
        assert!(body.get("response_format").is_none(), "body = {body:?}");
    }

    #[test]
    fn translate_request_openai_to_claude_maps_json_schema_output_shape() {
        let mut body = json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": "weather_response",
                    "schema": {
                        "type": "object",
                        "properties": {
                            "city": { "type": "string" }
                        },
                        "required": ["city"]
                    }
                }
            }
        });

        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Anthropic,
            "claude-3",
            &mut body,
            false,
        )
        .unwrap();

        assert_eq!(body["output_config"]["format"]["type"], "json_schema");
        assert_eq!(
            body["output_config"]["format"]["schema"]["properties"]["city"]["type"],
            "string"
        );
        assert_eq!(
            body["output_config"]["format"]["schema"]["required"][0],
            "city"
        );
    }

    #[test]
    fn translate_request_responses_to_claude_maps_json_schema_output_shape() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": "Hi",
            "text": {
                "format": {
                    "type": "json_schema",
                    "name": "weather_response",
                    "schema": {
                        "type": "object",
                        "properties": {
                            "city": { "type": "string" }
                        },
                        "required": ["city"]
                    }
                }
            }
        });

        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Anthropic,
            "claude-3",
            &mut body,
            false,
        )
        .unwrap();

        assert_eq!(body["output_config"]["format"]["type"], "json_schema");
        assert_eq!(
            body["output_config"]["format"]["schema"]["properties"]["city"]["type"],
            "string"
        );
        assert_eq!(
            body["output_config"]["format"]["schema"]["required"][0],
            "city"
        );
    }

    #[test]
    fn translate_request_openai_json_object_output_shape_to_claude_rejects() {
        for (client_format, mut body) in [
            (
                UpstreamFormat::OpenAiCompletion,
                json!({
                    "model": "claude-3",
                    "messages": [{ "role": "user", "content": "Hi" }],
                    "response_format": { "type": "json_object" }
                }),
            ),
            (
                UpstreamFormat::OpenAiResponses,
                json!({
                    "model": "gpt-4o",
                    "input": "Hi",
                    "text": { "format": { "type": "json_object" } }
                }),
            ),
        ] {
            let err = translate_request(
                client_format,
                UpstreamFormat::Anthropic,
                "claude-3",
                &mut body,
                false,
            )
            .expect_err("json_object output shape should fail closed for Anthropic");

            assert!(err.contains("json_object"), "err = {err}");
        }
    }

    #[test]
    fn translate_request_responses_to_gemini_maps_text_json_schema_format() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": "Hi",
            "text": {
                "format": {
                    "type": "json_schema",
                    "name": "weather_response",
                    "schema": {
                        "type": "object",
                        "properties": {
                            "city": { "type": "string" }
                        }
                    }
                }
            }
        });

        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Google,
            "gemini-2.5-flash",
            &mut body,
            false,
        )
        .unwrap();

        assert_eq!(
            body["generationConfig"]["responseMimeType"],
            "application/json"
        );
        assert_eq!(
            body["generationConfig"]["responseJsonSchema"]["properties"]["city"]["type"],
            "string"
        );
    }

    #[test]
    fn translate_request_gemini_to_openai_maps_stop_seed_and_penalties() {
        let mut body = json!({
            "model": "gemini-1.5",
            "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
            "generationConfig": {
                "stopSequences": ["END"],
                "seed": 42,
                "presencePenalty": 0.7,
                "frequencyPenalty": 0.3
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

        assert_eq!(body["stop"][0], "END");
        assert_eq!(body["seed"], 42);
        assert_eq!(body["presence_penalty"], 0.7);
        assert_eq!(body["frequency_penalty"], 0.3);
    }

    #[test]
    fn translate_request_gemini_to_openai_maps_logprobs_controls() {
        let mut body = json!({
            "model": "gemini-1.5",
            "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
            "generationConfig": {
                "responseLogprobs": true,
                "logprobs": 5
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

        assert_eq!(body["logprobs"], true);
        assert_eq!(body["top_logprobs"], 5);
    }

    #[test]
    fn translate_request_gemini_to_responses_maps_logprobs_controls() {
        let mut body = json!({
            "model": "gemini-1.5",
            "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
            "generation_config": {
                "response_logprobs": true,
                "logprobs": 5
            }
        });

        translate_request(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiResponses,
            "gemini-1.5",
            &mut body,
            false,
        )
        .unwrap();

        let include = body["include"].as_array().expect("responses include");
        assert!(
            include
                .iter()
                .any(|item| item == "message.output_text.logprobs"),
            "body = {body:?}"
        );
        assert_eq!(body["top_logprobs"], 5);
    }

    #[test]
    fn translate_request_openai_to_gemini_maps_stop_seed_and_penalties() {
        let mut body = json!({
            "model": "gemini-2.5-flash",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stop": ["END"],
            "seed": 42,
            "presence_penalty": 0.7,
            "frequency_penalty": 0.3
        });

        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-2.5-flash",
            &mut body,
            false,
        )
        .unwrap();

        assert_eq!(body["generationConfig"]["stopSequences"][0], "END");
        assert_eq!(body["generationConfig"]["seed"], 42);
        assert_eq!(body["generationConfig"]["presencePenalty"], 0.7);
        assert_eq!(body["generationConfig"]["frequencyPenalty"], 0.3);
    }

    #[test]
    fn assess_request_translation_gemini_top_k_warns_and_translation_omits_it() {
        let mut body = json!({
            "model": "gemini-1.5",
            "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
            "generationConfig": {
                "topK": 40
            }
        });

        let assessment = assess_request_translation(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiCompletion,
            &body,
        );
        let TranslationDecision::AllowWithWarnings(warnings) = assessment.decision() else {
            panic!("expected warning policy, got {assessment:?}");
        };
        assert!(warnings.iter().any(|warning| warning.contains("topK")));

        translate_request(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiCompletion,
            "gemini-1.5",
            &mut body,
            false,
        )
        .unwrap();

        assert!(body.get("top_k").is_none(), "body = {body:?}");
        assert!(body.get("topK").is_none(), "body = {body:?}");
    }

    #[test]
    fn assess_request_translation_gemini_to_anthropic_warns_on_dropped_logprobs_controls() {
        let body = json!({
            "model": "gemini-1.5",
            "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
            "generationConfig": {
                "responseLogprobs": true,
                "logprobs": 5
            }
        });

        let assessment = assess_request_translation(
            UpstreamFormat::Google,
            UpstreamFormat::Anthropic,
            &body,
        );
        let TranslationDecision::AllowWithWarnings(warnings) = assessment.decision() else {
            panic!("expected warning policy, got {assessment:?}");
        };
        let joined = warnings.join("\n");
        assert!(joined.contains("responseLogprobs"), "warnings = {warnings:?}");
        assert!(joined.contains("logprobs"), "warnings = {warnings:?}");
    }

    #[test]
    fn assess_request_translation_store_warns_and_cross_provider_translation_drops_it() {
        let gemini_body = json!({
            "model": "gemini-1.5",
            "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
            "store": true
        });
        let gemini_assessment = assess_request_translation(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiCompletion,
            &gemini_body,
        );
        let TranslationDecision::AllowWithWarnings(gemini_warnings) = gemini_assessment.decision()
        else {
            panic!("expected Gemini store warning, got {gemini_assessment:?}");
        };
        assert!(gemini_warnings
            .iter()
            .any(|warning| warning.contains("store")));

        let mut openai_body = json!({
            "model": "gemini-2.5-flash",
            "messages": [{ "role": "user", "content": "Hi" }],
            "store": true
        });
        let openai_assessment = assess_request_translation(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            &openai_body,
        );
        let TranslationDecision::AllowWithWarnings(openai_warnings) = openai_assessment.decision()
        else {
            panic!("expected OpenAI store warning, got {openai_assessment:?}");
        };
        assert!(openai_warnings
            .iter()
            .any(|warning| warning.contains("store")));

        let mut responses_body = json!({
            "model": "gpt-4o",
            "input": "Hi",
            "store": true
        });
        let responses_assessment = assess_request_translation(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            &responses_body,
        );
        let TranslationDecision::AllowWithWarnings(responses_warnings) =
            responses_assessment.decision()
        else {
            panic!("expected Responses store warning, got {responses_assessment:?}");
        };
        assert!(responses_warnings
            .iter()
            .any(|warning| warning.contains("store")));

        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-2.5-flash",
            &mut openai_body,
            false,
        )
        .unwrap();

        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut responses_body,
            false,
        )
        .unwrap();

        assert!(openai_body.get("store").is_none(), "body = {openai_body:?}");
        assert!(
            responses_body.get("store").is_none(),
            "body = {responses_body:?}"
        );
    }

    #[test]
    fn assess_request_translation_openai_to_responses_warns_on_dropped_sampling_controls() {
        let body = json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stop": ["END"],
            "seed": 42,
            "presence_penalty": 0.7,
            "frequency_penalty": 0.3
        });

        let assessment = assess_request_translation(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            &body,
        );
        let TranslationDecision::AllowWithWarnings(warnings) = assessment.decision() else {
            panic!("expected warning policy, got {assessment:?}");
        };
        let joined = warnings.join("\n");
        assert!(joined.contains("stop"), "warnings = {warnings:?}");
        assert!(joined.contains("seed"), "warnings = {warnings:?}");
        assert!(
            joined.contains("presence_penalty"),
            "warnings = {warnings:?}"
        );
        assert!(
            joined.contains("frequency_penalty"),
            "warnings = {warnings:?}"
        );
    }

    #[test]
    fn assess_request_translation_openai_to_non_chat_warns_on_logit_bias_drop() {
        for upstream_format in [
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Anthropic,
            UpstreamFormat::Google,
        ] {
            let body = json!({
                "model": "target-model",
                "messages": [{ "role": "user", "content": "Hi" }],
                "logit_bias": { "42": 3 }
            });

            let assessment = assess_request_translation(
                UpstreamFormat::OpenAiCompletion,
                upstream_format,
                &body,
            );
            let TranslationDecision::AllowWithWarnings(warnings) = assessment.decision() else {
                panic!("expected warning policy, got {assessment:?}");
            };
            let joined = warnings.join("\n");
            assert!(joined.contains("logit_bias"), "warnings = {warnings:?}");
        }
    }

    #[test]
    fn translate_request_openai_to_non_chat_drops_logit_bias() {
        for (upstream_format, model) in [
            (UpstreamFormat::OpenAiResponses, "gpt-4o"),
            (UpstreamFormat::Anthropic, "claude-3"),
            (UpstreamFormat::Google, "gemini-2.5-flash"),
        ] {
            let mut body = json!({
                "model": model,
                "messages": [{ "role": "user", "content": "Hi" }],
                "logit_bias": { "42": 3 }
            });

            translate_request(
                UpstreamFormat::OpenAiCompletion,
                upstream_format,
                model,
                &mut body,
                false,
            )
            .unwrap();

            assert!(body.get("logit_bias").is_none(), "body = {body:?}");
        }
    }

    #[test]
    fn assess_request_translation_openai_to_claude_warns_on_dropped_sampling_and_shared_controls_only(
    ) {
        let body = json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stop": ["END"],
            "temperature": 0.3,
            "top_p": 0.9,
            "service_tier": "priority",
            "verbosity": "low",
            "reasoning_effort": "medium",
            "seed": 42,
            "presence_penalty": 0.7,
            "frequency_penalty": 0.3
        });

        let assessment = assess_request_translation(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Anthropic,
            &body,
        );
        let TranslationDecision::AllowWithWarnings(warnings) = assessment.decision() else {
            panic!("expected warning policy, got {assessment:?}");
        };
        let joined = warnings.join("\n");
        assert!(joined.contains("seed"), "warnings = {warnings:?}");
        assert!(
            joined.contains("presence_penalty"),
            "warnings = {warnings:?}"
        );
        assert!(
            joined.contains("frequency_penalty"),
            "warnings = {warnings:?}"
        );
        assert!(joined.contains("service_tier"), "warnings = {warnings:?}");
        assert!(joined.contains("verbosity"), "warnings = {warnings:?}");
        assert!(joined.contains("reasoning_effort"), "warnings = {warnings:?}");
        assert!(!joined.contains("stop"), "warnings = {warnings:?}");
        assert!(!joined.contains("temperature"), "warnings = {warnings:?}");
        assert!(!joined.contains("top_p"), "warnings = {warnings:?}");
    }

    #[test]
    fn assess_request_translation_openai_to_gemini_warns_on_parallel_tool_calls_drop() {
        let body = json!({
            "model": "gemini-2.5-flash",
            "messages": [{ "role": "user", "content": "Hi" }],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "lookup_weather",
                    "parameters": { "type": "object", "properties": {} }
                }
            }],
            "parallel_tool_calls": false
        });

        let assessment = assess_request_translation(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            &body,
        );
        let TranslationDecision::AllowWithWarnings(warnings) = assessment.decision() else {
            panic!("expected warning policy, got {assessment:?}");
        };
        let joined = warnings.join("\n");
        assert!(joined.contains("parallel_tool_calls"), "warnings = {warnings:?}");
    }

    #[test]
    fn assess_request_translation_openai_to_gemini_warns_on_shared_control_drops() {
        let body = json!({
            "model": "gemini-2.5-flash",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stop": ["END"],
            "temperature": 0.3,
            "top_p": 0.9,
            "service_tier": "priority",
            "verbosity": "low",
            "reasoning_effort": "medium"
        });

        let assessment = assess_request_translation(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            &body,
        );
        let TranslationDecision::AllowWithWarnings(warnings) = assessment.decision() else {
            panic!("expected warning policy, got {assessment:?}");
        };
        let joined = warnings.join("\n");
        assert!(joined.contains("service_tier"), "warnings = {warnings:?}");
        assert!(joined.contains("verbosity"), "warnings = {warnings:?}");
        assert!(joined.contains("reasoning_effort"), "warnings = {warnings:?}");
        assert!(!joined.contains("stop"), "warnings = {warnings:?}");
        assert!(!joined.contains("temperature"), "warnings = {warnings:?}");
        assert!(!joined.contains("top_p"), "warnings = {warnings:?}");
    }

    #[test]
    fn assess_request_translation_openai_to_gemini_warns_on_prediction_and_web_search_drop() {
        let body = json!({
            "model": "gemini-2.5-flash",
            "messages": [{ "role": "user", "content": "Hi" }],
            "prediction": {
                "type": "content",
                "content": "Expected completion"
            },
            "web_search_options": {
                "search_context_size": "medium"
            }
        });

        let assessment = assess_request_translation(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            &body,
        );
        let TranslationDecision::AllowWithWarnings(warnings) = assessment.decision() else {
            panic!("expected warning policy, got {assessment:?}");
        };
        let joined = warnings.join("\n");
        assert!(joined.contains("prediction"), "warnings = {warnings:?}");
        assert!(joined.contains("web_search_options"), "warnings = {warnings:?}");
    }

    #[test]
    fn assess_request_translation_openai_to_anthropic_warns_on_prediction_and_web_search_drop() {
        let body = json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "prediction": {
                "type": "content",
                "content": "Expected completion"
            },
            "web_search_options": {
                "search_context_size": "medium"
            }
        });

        let assessment = assess_request_translation(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Anthropic,
            &body,
        );
        let TranslationDecision::AllowWithWarnings(warnings) = assessment.decision() else {
            panic!("expected warning policy, got {assessment:?}");
        };
        let joined = warnings.join("\n");
        assert!(joined.contains("prediction"), "warnings = {warnings:?}");
        assert!(joined.contains("web_search_options"), "warnings = {warnings:?}");
    }

    #[test]
    fn assess_request_translation_responses_to_non_responses_warns_on_context_management_drop() {
        let body = json!({
            "model": "gpt-4o",
            "input": "Hi",
            "context_management": {
                "type": "auto"
            }
        });

        for upstream_format in [
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            UpstreamFormat::Anthropic,
        ] {
            let assessment =
                assess_request_translation(UpstreamFormat::OpenAiResponses, upstream_format, &body);
            let TranslationDecision::AllowWithWarnings(warnings) = assessment.decision() else {
                panic!("expected warning policy, got {assessment:?}");
            };
            let joined = warnings.join("\n");
            assert!(
                joined.contains("context_management"),
                "upstream = {upstream_format:?}, warnings = {warnings:?}"
            );
        }
    }

    #[test]
    fn assess_request_translation_responses_to_openai_warns_on_truly_dropped_controls() {
        let body = json!({
            "model": "gpt-4o",
            "input": "Hi",
            "metadata": { "trace_id": "abc" },
            "user": "user-123",
            "service_tier": "priority",
            "stop": ["END"],
            "seed": 42,
            "presence_penalty": 0.7,
            "frequency_penalty": 0.3,
            "top_logprobs": 5,
            "stream_options": { "include_obfuscation": true },
            "include": ["reasoning.encrypted_content"],
            "reasoning": { "effort": "medium" },
            "text": {
                "format": { "type": "text" },
                "verbosity": "low"
            },
            "max_tool_calls": 2,
            "truncation": "auto",
            "prompt_cache_key": "cache-key",
            "prompt_cache_retention": "24h",
            "safety_identifier": "safe-user"
        });

        let assessment = assess_request_translation(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            &body,
        );
        let TranslationDecision::AllowWithWarnings(warnings) = assessment.decision() else {
            panic!("expected warning policy, got {assessment:?}");
        };
        let joined = warnings.join("\n");
        for field in [
            "stop",
            "seed",
            "presence_penalty",
            "frequency_penalty",
            "include",
            "max_tool_calls",
            "truncation",
        ] {
            assert!(
                joined.contains(field),
                "field = {field}, warnings = {warnings:?}"
            );
        }
        for field in [
            "metadata",
            "user",
            "service_tier",
            "include_obfuscation",
            "reasoning",
            "verbosity",
            "top_logprobs",
            "prompt_cache_key",
            "prompt_cache_retention",
            "safety_identifier",
        ] {
            assert!(
                !joined.contains(field),
                "field = {field}, warnings = {warnings:?}"
            );
        }
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

        let content = body["messages"][1]["content"]
            .as_array()
            .expect("claude content");
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
    fn translate_request_claude_to_non_anthropic_rejects_thinking_signature_provenance() {
        let body = json!({
            "model": "claude-3",
            "messages": [{
                "role": "assistant",
                "content": [{
                    "type": "thinking",
                    "thinking": "internal reasoning",
                    "signature": "sig_123"
                }]
            }]
        });

        for upstream_format in [
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Google,
        ] {
            let mut translated = body.clone();
            let err = translate_request(
                UpstreamFormat::Anthropic,
                upstream_format,
                "claude-3",
                &mut translated,
                false,
            )
            .expect_err("Anthropic request thinking provenance should fail closed");

            assert!(err.contains("thinking"), "err = {err}");
            assert!(err.contains("signature"), "err = {err}");
        }
    }

    #[test]
    fn translate_request_claude_to_non_anthropic_rejects_omitted_thinking_history() {
        let body = json!({
            "model": "claude-3",
            "messages": [{
                "role": "assistant",
                "content": [{
                    "type": "thinking",
                    "thinking": { "display": "omitted" },
                    "signature": "sig_123"
                }]
            }]
        });

        for upstream_format in [
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Google,
        ] {
            let mut translated = body.clone();
            let err = translate_request(
                UpstreamFormat::Anthropic,
                upstream_format,
                "claude-3",
                &mut translated,
                false,
            )
            .expect_err("Anthropic omitted thinking should fail closed");

            assert!(err.contains("thinking"), "err = {err}");
            assert!(err.contains("Anthropic"), "err = {err}");
        }
    }

    #[test]
    fn translate_request_claude_to_non_anthropic_rejects_nonportable_tool_definition_metadata() {
        let tool_cases = [
            (
                "strict",
                json!({
                    "name": "lookup_weather",
                    "description": "Weather lookup",
                    "input_schema": { "type": "object", "properties": {} },
                    "strict": true
                }),
            ),
            (
                "defer_loading",
                json!({
                    "name": "lookup_weather",
                    "description": "Weather lookup",
                    "input_schema": { "type": "object", "properties": {} },
                    "defer_loading": true
                }),
            ),
            (
                "allowed_callers",
                json!({
                    "name": "lookup_weather",
                    "description": "Weather lookup",
                    "input_schema": { "type": "object", "properties": {} },
                    "allowed_callers": ["assistant"]
                }),
            ),
            (
                "input_examples",
                json!({
                    "name": "lookup_weather",
                    "description": "Weather lookup",
                    "input_schema": { "type": "object", "properties": {} },
                    "input_examples": [{
                        "city": "Tokyo"
                    }]
                }),
            ),
            (
                "server-side",
                json!({
                    "type": "web_search_20250305",
                    "name": "web_search"
                }),
            ),
        ];

        for (label, tool) in tool_cases {
            for upstream_format in [
                UpstreamFormat::OpenAiCompletion,
                UpstreamFormat::OpenAiResponses,
                UpstreamFormat::Google,
            ] {
                let mut body = json!({
                    "model": "claude-3",
                    "messages": [{ "role": "user", "content": "Hi" }],
                    "tools": [tool.clone()]
                });

                let err = translate_request(
                    UpstreamFormat::Anthropic,
                    upstream_format,
                    "claude-3",
                    &mut body,
                    false,
                )
                .expect_err("Anthropic tool metadata should fail closed");

                assert!(err.contains("tool"), "label = {label}, err = {err}");
            }
        }
    }

    #[test]
    fn translate_request_claude_to_non_anthropic_allows_supported_tool_definition_fields() {
        for upstream_format in [
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Google,
        ] {
            let mut body = json!({
                "model": "claude-3",
                "messages": [{ "role": "user", "content": "Hi" }],
                "tools": [{
                    "name": "lookup_weather",
                    "description": "Weather lookup",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "city": { "type": "string" }
                        }
                    }
                }]
            });

            translate_request(
                UpstreamFormat::Anthropic,
                upstream_format,
                "claude-3",
                &mut body,
                false,
            )
            .expect("supported Anthropic tool fields should translate");
        }
    }

    #[test]
    fn translate_request_claude_to_non_anthropic_rejects_user_turn_that_would_reorder_tool_results()
    {
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
                    "content": [
                        { "type": "text", "text": "Before the result" },
                        {
                            "type": "tool_result",
                            "tool_use_id": "toolu_1",
                            "content": "sunny"
                        }
                    ]
                }
            ]
        });

        let err = translate_request(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiCompletion,
            "claude-3",
            &mut body,
            false,
        )
        .expect_err("mixed user turns that reorder tool_results should fail closed");

        assert!(err.contains("tool_result"), "err = {err}");
        assert!(err.contains("order"), "err = {err}");
    }

    #[test]
    fn translate_request_claude_to_openai_preserves_multiblock_system_without_injected_newlines() {
        let mut body = json!({
            "model": "claude-3",
            "system": [
                { "type": "text", "text": "System A" },
                { "type": "text", "text": "System B" }
            ],
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

        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages[0]["role"], "system");
        let content = messages[0]["content"].as_array().expect("system parts");
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "System A");
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[1]["text"], "System B");
    }

    #[test]
    fn translate_request_claude_to_gemini_preserves_multiblock_system_without_injected_newlines() {
        let mut body = json!({
            "model": "claude-3",
            "system": [
                { "type": "text", "text": "System A" },
                { "type": "text", "text": "System B" }
            ],
            "messages": [{ "role": "user", "content": "Hi" }]
        });

        translate_request(
            UpstreamFormat::Anthropic,
            UpstreamFormat::Google,
            "claude-3",
            &mut body,
            false,
        )
        .unwrap();

        let parts = body["systemInstruction"]["parts"]
            .as_array()
            .expect("system instruction parts");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["text"], "System A");
        assert_eq!(parts[1]["text"], "System B");
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
    fn translate_request_gemini_to_non_gemini_rejects_function_response_parts() {
        let body = json!({
            "model": "gemini-1.5",
            "contents": [{
                "role": "user",
                "parts": [{
                    "functionResponse": {
                        "id": "call_1",
                        "name": "lookup_weather",
                        "response": {
                            "result": {
                                "parts": [
                                    { "text": "sunny" }
                                ]
                            }
                        }
                    }
                }]
            }]
        });

        for upstream_format in [
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Anthropic,
        ] {
            let mut translated = body.clone();
            let err = translate_request(
                UpstreamFormat::Google,
                upstream_format,
                "gemini-1.5",
                &mut translated,
                false,
            )
            .expect_err("Gemini functionResponse.parts should fail closed");

            assert!(err.contains("functionResponse"), "err = {err}");
            assert!(err.contains("parts"), "err = {err}");
        }
    }

    #[test]
    fn convert_gemini_content_to_openai_rejects_function_response_result_parts_at_boundary() {
        let content = json!({
            "role": "user",
            "parts": [{
                "functionResponse": {
                    "id": "call_1",
                    "name": "lookup_weather",
                    "response": {
                        "result": {
                            "parts": [{ "text": "sunny" }]
                        }
                    }
                }
            }]
        });

        let err = convert_gemini_content_to_openai(&content)
            .expect_err("functionResponse.response.result.parts should fail closed at conversion");

        assert!(err.contains("functionResponse"), "err = {err}");
        assert!(err.contains("parts"), "err = {err}");
    }

    #[test]
    fn translate_request_gemini_to_openai_allows_structured_function_response_without_parts() {
        let mut body = json!({
            "model": "gemini-1.5",
            "contents": [{
                "role": "user",
                "parts": [{
                    "functionResponse": {
                        "id": "call_1",
                        "name": "lookup_weather",
                        "response": { "result": { "temperature": 22 } }
                    }
                }]
            }]
        });

        translate_request(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiCompletion,
            "gemini-1.5",
            &mut body,
            false,
        )
        .expect("structured functionResponse.response should still translate");

        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "tool");
        assert_eq!(messages[0]["tool_call_id"], "call_1");
    }

    #[test]
    fn translate_request_openai_to_gemini_keeps_json_tool_results_on_response_result_path() {
        let mut body = json!({
            "model": "gemini-1.5",
            "messages": [
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
                    "content": { "temperature": 22, "unit": "C" }
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

        let function_response = &body["contents"][1]["parts"][0]["functionResponse"];
        assert_eq!(function_response["response"]["result"]["temperature"], 22);
        assert!(function_response.get("parts").is_none(), "body = {body:?}");
    }

    #[test]
    fn translate_request_responses_to_gemini_keeps_json_tool_results_on_response_result_path() {
        let mut body = json!({
            "model": "gpt-4o",
            "input": [
                {
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "lookup_weather",
                    "arguments": "{\"city\":\"Tokyo\"}"
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_1",
                    "output": { "temperature": 22, "unit": "C" }
                }
            ]
        });

        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Google,
            "gemini-1.5",
            &mut body,
            false,
        )
        .unwrap();

        let function_response = &body["contents"][1]["parts"][0]["functionResponse"];
        assert_eq!(function_response["response"]["result"]["temperature"], 22);
        assert!(function_response.get("parts").is_none(), "body = {body:?}");
    }

    #[test]
    fn translate_request_claude_to_gemini_keeps_json_tool_results_on_response_result_path() {
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
                        "content": { "temperature": 22, "unit": "C" }
                    }]
                }
            ]
        });

        translate_request(
            UpstreamFormat::Anthropic,
            UpstreamFormat::Google,
            "gemini-1.5",
            &mut body,
            false,
        )
        .unwrap();

        let function_response = &body["contents"][1]["parts"][0]["functionResponse"];
        assert_eq!(function_response["response"]["result"]["temperature"], 22);
        assert!(function_response.get("parts").is_none(), "body = {body:?}");
    }

    #[test]
    fn translate_request_openai_to_gemini_moves_inline_multimodal_tool_results_into_function_response_parts(
    ) {
        let mut body = json!({
            "model": "gemini-3-pro",
            "messages": [
                {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "inspect_media", "arguments": "{\"city\":\"Tokyo\"}" }
                    }]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_1",
                    "content": [
                        { "type": "text", "text": "Captured artifacts" },
                        {
                            "type": "image_url",
                            "image_url": { "url": "data:image/png;base64,AAAA" }
                        },
                        {
                            "type": "file",
                            "file": {
                                "file_data": "data:application/pdf;base64,CCCC",
                                "filename": "report.pdf"
                            }
                        }
                    ]
                }
            ]
        });

        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-3-pro",
            &mut body,
            false,
        )
        .unwrap();

        let function_response = &body["contents"][1]["parts"][0]["functionResponse"];
        let result = function_response["response"]["result"]
            .as_array()
            .expect("result array");
        let parts = function_response["parts"]
            .as_array()
            .expect("functionResponse parts");

        assert_eq!(result[0]["type"], "text");
        assert_eq!(result[0]["text"], "Captured artifacts");
        assert_eq!(result[1]["type"], "image");
        assert_eq!(result[1]["image"]["$ref"], "call_1_part_1.png");
        assert_eq!(result[2]["type"], "file");
        assert_eq!(result[2]["file"]["$ref"], "call_1_part_2.pdf");
        assert_eq!(result[2]["filename"], "report.pdf");

        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["inlineData"]["displayName"], "call_1_part_1.png");
        assert_eq!(parts[0]["inlineData"]["mimeType"], "image/png");
        assert_eq!(parts[1]["inlineData"]["displayName"], "call_1_part_2.pdf");
        assert_eq!(parts[1]["inlineData"]["mimeType"], "application/pdf");
    }

    #[test]
    fn translate_request_responses_to_gemini_moves_inline_multimodal_tool_results_into_function_response_parts(
    ) {
        let mut body = json!({
            "model": "gpt-4o",
            "input": [
                {
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "inspect_media",
                    "arguments": "{\"city\":\"Tokyo\"}"
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_1",
                    "output": [
                        { "type": "input_text", "text": "Captured screenshot" },
                        {
                            "type": "input_image",
                            "image_url": "data:image/png;base64,AAAA"
                        }
                    ]
                }
            ]
        });

        translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Google,
            "gemini-3-pro",
            &mut body,
            false,
        )
        .unwrap();

        let function_response = &body["contents"][1]["parts"][0]["functionResponse"];
        let result = function_response["response"]["result"]
            .as_array()
            .expect("result array");
        let parts = function_response["parts"]
            .as_array()
            .expect("functionResponse parts");

        assert_eq!(result[0]["type"], "text");
        assert_eq!(result[0]["text"], "Captured screenshot");
        assert_eq!(result[1]["type"], "image");
        assert_eq!(result[1]["image"]["$ref"], "call_1_part_1.png");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["inlineData"]["displayName"], "call_1_part_1.png");
    }

    #[test]
    fn translate_request_claude_to_gemini_moves_inline_multimodal_tool_results_into_function_response_parts(
    ) {
        let mut body = json!({
            "model": "claude-3",
            "messages": [
                {
                    "role": "assistant",
                    "content": [{
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": "inspect_media",
                        "input": { "city": "Tokyo" }
                    }]
                },
                {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": "toolu_1",
                        "content": [
                            { "type": "text", "text": "Captured frame" },
                            {
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": "image/png",
                                    "data": "AAAA"
                                }
                            }
                        ]
                    }]
                }
            ]
        });

        translate_request(
            UpstreamFormat::Anthropic,
            UpstreamFormat::Google,
            "gemini-3-pro",
            &mut body,
            false,
        )
        .unwrap();

        let function_response = &body["contents"][1]["parts"][0]["functionResponse"];
        let result = function_response["response"]["result"]
            .as_array()
            .expect("result array");
        let parts = function_response["parts"]
            .as_array()
            .expect("functionResponse parts");

        assert_eq!(result[0]["type"], "text");
        assert_eq!(result[0]["text"], "Captured frame");
        assert_eq!(result[1]["type"], "image");
        assert_eq!(result[1]["image"]["$ref"], "toolu_1_part_1.png");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["inlineData"]["displayName"], "toolu_1_part_1.png");
        assert_eq!(parts[0]["inlineData"]["mimeType"], "image/png");
    }

    #[test]
    fn translate_request_openai_to_gemini_rejects_remote_tool_result_media_references() {
        let mut body = json!({
            "model": "gemini-3-pro",
            "messages": [
                {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "inspect_media", "arguments": "{\"city\":\"Tokyo\"}" }
                    }]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_1",
                    "content": [{
                        "type": "image_url",
                        "image_url": { "url": "https://example.com/cat.png" }
                    }]
                }
            ]
        });

        let err = translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-3-pro",
            &mut body,
            false,
        )
        .expect_err("remote tool result media should fail closed");

        assert!(err.contains("remote"), "err = {err}");
        assert!(err.contains("tool result"), "err = {err}");
    }

    #[test]
    fn translate_request_openai_to_gemini_rejects_file_id_tool_result_references() {
        let mut body = json!({
            "model": "gemini-3-pro",
            "messages": [
                {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "inspect_media", "arguments": "{\"city\":\"Tokyo\"}" }
                    }]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_1",
                    "content": [{
                        "type": "file",
                        "file": { "file_id": "file_123" }
                    }]
                }
            ]
        });

        let err = translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-3-pro",
            &mut body,
            false,
        )
        .expect_err("file_id tool result references should fail closed");

        assert!(err.contains("file_id"), "err = {err}");
        assert!(err.contains("tool result"), "err = {err}");
    }

    #[test]
    fn translate_request_openai_to_gemini_rejects_unknown_typed_tool_result_media_blocks() {
        let mut body = json!({
            "model": "gemini-3-pro",
            "messages": [
                {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "inspect_media", "arguments": "{\"city\":\"Tokyo\"}" }
                    }]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_1",
                    "content": [{
                        "type": "video",
                        "data": "AAAA"
                    }]
                }
            ]
        });

        let err = translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-3-pro",
            &mut body,
            false,
        )
        .expect_err("unknown typed tool result media should fail closed");

        assert!(err.contains("video"), "err = {err}");
        assert!(err.contains("tool result"), "err = {err}");
    }

    #[test]
    fn translate_request_openai_to_gemini_rejects_multimodal_function_response_parts_for_gemini_1_5(
    ) {
        let mut body = json!({
            "model": "gemini-1.5",
            "messages": [
                {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "inspect_media", "arguments": "{\"city\":\"Tokyo\"}" }
                    }]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_1",
                    "content": [{
                        "type": "image_url",
                        "image_url": { "url": "data:image/png;base64,AAAA" }
                    }]
                }
            ]
        });

        let err = translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-1.5",
            &mut body,
            false,
        )
        .expect_err("Gemini 1.5 should reject multimodal functionResponse.parts");

        assert!(err.contains("Gemini 3"), "err = {err}");
        assert!(err.contains("functionResponse.parts"), "err = {err}");
    }

    #[test]
    fn translate_request_openai_to_gemini_rejects_unsupported_multimodal_function_response_mime() {
        let mut body = json!({
            "model": "gemini-3-pro",
            "messages": [
                {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "inspect_media", "arguments": "{\"city\":\"Tokyo\"}" }
                    }]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_1",
                    "content": [{
                        "type": "input_audio",
                        "input_audio": { "data": "BBBB", "format": "wav" }
                    }]
                }
            ]
        });

        let err = translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-3-pro",
            &mut body,
            false,
        )
        .expect_err("unsupported tool-result MIME should fail closed");

        assert!(err.contains("audio/wav"), "err = {err}");
        assert!(err.contains("functionResponse.parts"), "err = {err}");
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
        assert_eq!(
            tool_calls[0]["proxied_tool_kind"],
            "anthropic_server_tool_use"
        );
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
    fn translate_response_responses_to_openai_maps_top_level_output_audio_to_message_audio() {
        let body = json!({
            "id": "resp_audio",
            "object": "response",
            "output": [
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": "Hi" }]
                },
                {
                    "type": "output_audio",
                    "data": "AAAA",
                    "transcript": "hello"
                }
            ]
        });

        let out = translate_response(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .expect("Responses output_audio should map to Chat assistant audio");

        assert_eq!(out["choices"][0]["message"]["content"], "Hi");
        assert_eq!(out["choices"][0]["message"]["audio"]["data"], "AAAA");
        assert_eq!(out["choices"][0]["message"]["audio"]["transcript"], "hello");
    }

    #[test]
    fn translate_response_responses_to_openai_preserves_output_text_logprobs() {
        let body = json!({
            "id": "resp_logprobs",
            "object": "response",
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": "Hi",
                    "logprobs": [{
                        "token": "Hi",
                        "bytes": [72, 105],
                        "logprob": -0.1,
                        "top_logprobs": [{
                            "token": "Hi",
                            "bytes": [72, 105],
                            "logprob": -0.1
                        }]
                    }]
                }]
            }]
        });

        let out = translate_response(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .expect("Responses output_text.logprobs should map to Chat choice.logprobs");

        assert_eq!(out["choices"][0]["message"]["content"], "Hi");
        assert_eq!(out["choices"][0]["logprobs"]["content"][0]["token"], "Hi");
        assert_eq!(
            out["choices"][0]["logprobs"]["content"][0]["top_logprobs"][0]["token"],
            "Hi"
        );
    }

    #[test]
    fn translate_response_responses_to_openai_accepts_legacy_nested_output_audio_shape() {
        let body = json!({
            "id": "resp_audio_legacy",
            "object": "response",
            "output": [{
                "type": "output_audio",
                "audio": {
                    "data": "AAAA",
                    "format": "wav"
                }
            }]
        });

        let out = translate_response(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .expect("legacy nested output_audio should still parse");

        assert_eq!(out["choices"][0]["message"]["audio"]["data"], "AAAA");
        assert_eq!(out["choices"][0]["message"]["audio"]["format"], "wav");
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
    fn translate_response_responses_to_openai_preserves_audio_prediction_and_unknown_usage_fields()
    {
        let body = json!({
            "id": "resp_usage",
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
                "service_tier": "priority",
                "provider_metric": 99,
                "input_tokens_details": {
                    "cached_tokens": 3,
                    "audio_tokens": 2,
                    "future_prompt_detail": 4
                },
                "output_tokens_details": {
                    "reasoning_tokens": 1,
                    "audio_tokens": 5,
                    "accepted_prediction_tokens": 6,
                    "rejected_prediction_tokens": 2,
                    "future_completion_detail": 8
                }
            }
        });

        let out = translate_response(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .unwrap();

        assert_eq!(out["usage"]["service_tier"], "priority");
        assert_eq!(out["usage"]["provider_metric"], 99);
        assert_eq!(out["usage"]["prompt_tokens_details"]["cached_tokens"], 3);
        assert_eq!(out["usage"]["prompt_tokens_details"]["audio_tokens"], 2);
        assert_eq!(
            out["usage"]["prompt_tokens_details"]["future_prompt_detail"],
            4
        );
        assert_eq!(
            out["usage"]["completion_tokens_details"]["reasoning_tokens"],
            1
        );
        assert_eq!(out["usage"]["completion_tokens_details"]["audio_tokens"], 5);
        assert_eq!(
            out["usage"]["completion_tokens_details"]["accepted_prediction_tokens"],
            6
        );
        assert_eq!(
            out["usage"]["completion_tokens_details"]["rejected_prediction_tokens"],
            2
        );
        assert_eq!(
            out["usage"]["completion_tokens_details"]["future_completion_detail"],
            8
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
    fn translate_response_openai_assistant_audio_maps_to_responses_output_audio() {
        let body = json!({
            "id": "chatcmpl_audio",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hi",
                    "audio": {
                        "data": "AAAA",
                        "transcript": "hello"
                    }
                },
                "finish_reason": "stop"
            }]
        });

        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            &body,
        )
        .expect("assistant audio should map to Responses output_audio");

        let output = out["output"].as_array().expect("responses output");
        assert_eq!(output[0]["type"], "message");
        assert_eq!(output[0]["content"][0]["text"], "Hi");
        assert_eq!(output[1]["type"], "output_audio");
        assert_eq!(output[1]["data"], "AAAA");
        assert_eq!(output[1]["transcript"], "hello");
        assert!(output[1].get("audio").is_none(), "output = {output:?}");
    }

    #[test]
    fn translate_response_openai_to_responses_preserves_choice_logprobs_on_output_text() {
        let body = json!({
            "id": "chatcmpl_logprobs",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hi"
                },
                "logprobs": {
                    "content": [{
                        "token": "Hi",
                        "bytes": [72, 105],
                        "logprob": -0.1,
                        "top_logprobs": [{
                            "token": "Hi",
                            "bytes": [72, 105],
                            "logprob": -0.1
                        }]
                    }],
                    "refusal": []
                },
                "finish_reason": "stop"
            }]
        });

        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            &body,
        )
        .expect("Chat choice.logprobs should map to Responses output_text.logprobs");

        let content = out["output"][0]["content"].as_array().expect("responses content");
        assert_eq!(content[0]["type"], "output_text");
        assert_eq!(content[0]["text"], "Hi");
        assert_eq!(content[0]["logprobs"][0]["token"], "Hi");
        assert_eq!(
            content[0]["logprobs"][0]["top_logprobs"][0]["token"],
            "Hi"
        );
    }

    #[test]
    fn translate_response_openai_assistant_audio_with_id_rejects_for_responses() {
        let body = json!({
            "id": "chatcmpl_audio_id",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hi",
                    "audio": {
                        "data": "AAAA",
                        "transcript": "hello",
                        "id": "aud_123"
                    }
                },
                "finish_reason": "stop"
            }]
        });

        let err = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            &body,
        )
        .expect_err("assistant audio ids should fail closed for Responses");

        assert!(err.contains("audio"), "err = {err}");
        assert!(err.contains("id"), "err = {err}");
    }

    #[test]
    fn translate_response_openai_assistant_audio_with_expires_at_rejects_for_responses() {
        let body = json!({
            "id": "chatcmpl_audio_exp",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hi",
                    "audio": {
                        "data": "AAAA",
                        "transcript": "hello",
                        "expires_at": 1234567890
                    }
                },
                "finish_reason": "stop"
            }]
        });

        let err = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            &body,
        )
        .expect_err("assistant audio expiry should fail closed for Responses");

        assert!(err.contains("audio"), "err = {err}");
        assert!(err.contains("expires_at"), "err = {err}");
    }

    #[test]
    fn translate_response_openai_assistant_audio_still_fails_closed_for_non_responses_targets() {
        let body = json!({
            "id": "chatcmpl_audio",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hi",
                    "audio": {
                        "data": "AAAA",
                        "format": "wav"
                    }
                },
                "finish_reason": "stop"
            }]
        });

        for client_format in [UpstreamFormat::Anthropic, UpstreamFormat::Google] {
            let err = translate_response(UpstreamFormat::OpenAiCompletion, client_format, &body)
                .expect_err("assistant audio should still fail closed for non-Responses sinks");
            assert!(err.contains("audio"), "err = {err}");
            assert!(err.contains("OpenAI"), "err = {err}");
        }
    }

    #[test]
    fn translate_response_openai_to_responses_preserves_audio_prediction_and_unknown_usage_fields()
    {
        let body = json!({
            "id": "chatcmpl_usage",
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
                "service_tier": "priority",
                "provider_metric": 99,
                "prompt_tokens_details": {
                    "cached_tokens": 3,
                    "audio_tokens": 2,
                    "future_prompt_detail": 4
                },
                "completion_tokens_details": {
                    "reasoning_tokens": 1,
                    "audio_tokens": 5,
                    "accepted_prediction_tokens": 6,
                    "rejected_prediction_tokens": 2,
                    "future_completion_detail": 8
                }
            }
        });

        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            &body,
        )
        .unwrap();

        assert_eq!(out["usage"]["service_tier"], "priority");
        assert_eq!(out["usage"]["provider_metric"], 99);
        assert_eq!(out["usage"]["input_tokens_details"]["cached_tokens"], 3);
        assert_eq!(out["usage"]["input_tokens_details"]["audio_tokens"], 2);
        assert_eq!(
            out["usage"]["input_tokens_details"]["future_prompt_detail"],
            4
        );
        assert_eq!(out["usage"]["output_tokens_details"]["reasoning_tokens"], 1);
        assert_eq!(out["usage"]["output_tokens_details"]["audio_tokens"], 5);
        assert_eq!(
            out["usage"]["output_tokens_details"]["accepted_prediction_tokens"],
            6
        );
        assert_eq!(
            out["usage"]["output_tokens_details"]["rejected_prediction_tokens"],
            2
        );
        assert_eq!(
            out["usage"]["output_tokens_details"]["future_completion_detail"],
            8
        );
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

        let content = out["output"][0]["content"]
            .as_array()
            .expect("responses content");
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
        assert!(
            content[1].get("annotations").is_none(),
            "content = {content:?}"
        );
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
        assert_eq!(tool_calls[0]["custom"]["name"], "code_exec");
        assert_eq!(tool_calls[0]["custom"]["input"], "print('hi')");
        assert_eq!(
            tool_calls[1]["proxied_tool_kind"],
            "anthropic_server_tool_use"
        );
    }

    #[test]
    fn translate_response_responses_portable_output_subset_stays_valid() {
        let body = json!({
            "id": "resp_portable",
            "object": "response",
            "created_at": 1,
            "status": "completed",
            "output": [
                {
                    "type": "reasoning",
                    "summary": [{
                        "type": "summary_text",
                        "text": "Need a tool."
                    }]
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{
                        "type": "output_text",
                        "text": "Looking it up."
                    }]
                },
                {
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "lookup_weather",
                    "arguments": "{\"city\":\"Tokyo\"}"
                },
                {
                    "type": "output_audio",
                    "data": "AAAA",
                    "transcript": "Looking it up."
                }
            ]
        });

        let out = translate_response(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .expect("portable Responses output subset should remain translatable");

        let message = &out["choices"][0]["message"];
        assert_eq!(message["reasoning_content"], "Need a tool.");
        assert_eq!(message["content"], "Looking it up.");
        assert_eq!(message["tool_calls"][0]["function"]["name"], "lookup_weather");
        assert_eq!(message["audio"]["data"], "AAAA");
        assert_eq!(message["audio"]["transcript"], "Looking it up.");
    }

    #[test]
    fn translate_response_responses_namespaced_tool_calls_fail_closed_for_non_responses_clients() {
        let body = json!({
            "id": "resp_namespaced_tools",
            "object": "response",
            "created_at": 1,
            "status": "completed",
            "output": [
                {
                    "type": "custom_tool_call",
                    "call_id": "call_custom",
                    "name": "lookup_account",
                    "namespace": "crm",
                    "input": "account_id=123"
                }
            ]
        });

        for client_format in [
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Anthropic,
            UpstreamFormat::Google,
        ] {
            let err = translate_response(UpstreamFormat::OpenAiResponses, client_format, &body)
                .expect_err("Responses namespaced tool calls should fail closed");
            assert!(err.contains("namespace"), "err = {err}");
        }
    }

    #[test]
    fn translate_response_responses_nonportable_output_items_fail_closed_for_non_responses_clients()
    {
        let cases = [
            (
                "computer_call_output",
                json!({
                    "type": "computer_call_output",
                    "call_id": "call_computer",
                    "output": {
                        "type": "computer_screenshot",
                        "image_url": "https://example.com/screen.png"
                    }
                }),
            ),
            (
                "compaction",
                json!({
                    "type": "compaction",
                    "id": "cmp_123",
                    "encrypted_content": "opaque"
                }),
            ),
        ];

        for (label, item) in cases {
            let body = json!({
                "id": format!("resp_{label}"),
                "object": "response",
                "created_at": 1,
                "status": "completed",
                "output": [item]
            });

            for client_format in [
                UpstreamFormat::OpenAiCompletion,
                UpstreamFormat::Anthropic,
                UpstreamFormat::Google,
            ] {
                let err = translate_response(UpstreamFormat::OpenAiResponses, client_format, &body)
                    .expect_err("nonportable Responses output items should fail closed");
                assert!(err.contains(label), "label = {label}, err = {err}");
            }
        }
    }

    #[test]
    fn translate_response_responses_reasoning_encrypted_content_fails_closed_for_non_responses_clients(
    ) {
        let body = json!({
            "id": "resp_reasoning_encrypted",
            "object": "response",
            "created_at": 1,
            "status": "completed",
            "output": [{
                "type": "reasoning",
                "summary": [{
                    "type": "summary_text",
                    "text": "Private reasoning."
                }],
                "encrypted_content": "opaque_state"
            }]
        });

        for client_format in [
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Anthropic,
            UpstreamFormat::Google,
        ] {
            let err = translate_response(UpstreamFormat::OpenAiResponses, client_format, &body)
                .expect_err("encrypted reasoning should fail closed on non-Responses clients");
            assert!(err.contains("encrypted_content"), "err = {err}");
        }
    }

    #[test]
    fn translate_response_responses_tool_call_output_items_fail_closed_for_non_responses_clients() {
        let cases = [
            json!({
                "type": "function_call_output",
                "call_id": "call_fn",
                "output": "done"
            }),
            json!({
                "type": "custom_tool_call_output",
                "call_id": "call_custom",
                "output": "done"
            }),
        ];

        for item in cases {
            let label = item["type"].as_str().expect("item type");
            let body = json!({
                "id": format!("resp_{label}"),
                "object": "response",
                "created_at": 1,
                "status": "completed",
                "output": [item]
            });

            for client_format in [
                UpstreamFormat::OpenAiCompletion,
                UpstreamFormat::Anthropic,
                UpstreamFormat::Google,
            ] {
                let err = translate_response(UpstreamFormat::OpenAiResponses, client_format, &body)
                    .expect_err("Responses tool-call output items should fail closed");
                assert!(err.contains(label), "label = {label}, err = {err}");
            }
        }
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
        assert_eq!(
            content[0]["citations"][0]["url"],
            "https://example.com/fact"
        );
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
        assert_eq!(out["usage"]["output_tokens_details"]["reasoning_tokens"], 4);
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
    fn translate_response_openai_to_gemini_maps_response_logprobs() {
        let body = json!({
            "id": "chatcmpl_logprobs_gemini",
            "object": "chat.completion",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "Hi" },
                "logprobs": {
                    "content": [{
                        "token": "Hi",
                        "bytes": [72, 105],
                        "logprob": -0.1,
                        "top_logprobs": [
                            { "token": "Hi", "bytes": [72, 105], "logprob": -0.1 },
                            { "token": "Hey", "bytes": [72, 101, 121], "logprob": -0.4 }
                        ]
                    }],
                    "refusal": []
                },
                "finish_reason": "stop"
            }]
        });

        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            &body,
        )
        .expect("Chat response logprobs should map to Gemini candidate logprobs");

        assert_eq!(out["candidates"][0]["avgLogprobs"], -0.1);
        assert_eq!(out["candidates"][0]["logprobsResult"]["logProbabilitySum"], -0.1);
        assert_eq!(
            out["candidates"][0]["logprobsResult"]["chosenCandidates"][0]["token"],
            "Hi"
        );
        assert_eq!(
            out["candidates"][0]["logprobsResult"]["topCandidates"][0]["candidates"][0]["token"],
            "Hi"
        );
        assert_eq!(
            out["candidates"][0]["logprobsResult"]["topCandidates"][0]["candidates"][1]["token"],
            "Hey"
        );
    }

    #[test]
    fn translate_response_openai_to_gemini_rejects_nonportable_refusal_logprobs() {
        let body = json!({
            "id": "chatcmpl_refusal_logprobs_gemini",
            "object": "chat.completion",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "refusal": "I can't help with that."
                },
                "logprobs": {
                    "content": [],
                    "refusal": [{
                        "token": "I",
                        "bytes": [73],
                        "logprob": -0.1,
                        "top_logprobs": []
                    }]
                },
                "finish_reason": "content_filter"
            }]
        });

        let err = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            &body,
        )
        .expect_err("refusal logprobs should fail closed for Gemini translation");

        assert!(err.contains("refusal"), "err = {err}");
        assert!(err.contains("Gemini"), "err = {err}");
    }

    #[test]
    fn translate_response_responses_to_gemini_maps_output_text_logprobs() {
        let body = json!({
            "id": "resp_logprobs_gemini",
            "object": "response",
            "model": "gpt-4o",
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": "Hi",
                    "logprobs": [{
                        "token": "Hi",
                        "bytes": [72, 105],
                        "logprob": -0.1,
                        "top_logprobs": [
                            { "token": "Hi", "bytes": [72, 105], "logprob": -0.1 },
                            { "token": "Hey", "bytes": [72, 101, 121], "logprob": -0.4 }
                        ]
                    }]
                }]
            }]
        });

        let out = translate_response(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Google,
            &body,
        )
        .expect("Responses output_text.logprobs should map to Gemini candidate logprobs");

        assert_eq!(out["candidates"][0]["avgLogprobs"], -0.1);
        assert_eq!(out["candidates"][0]["logprobsResult"]["logProbabilitySum"], -0.1);
        assert_eq!(
            out["candidates"][0]["logprobsResult"]["chosenCandidates"][0]["token"],
            "Hi"
        );
        assert_eq!(
            out["candidates"][0]["logprobsResult"]["topCandidates"][0]["candidates"][1]["token"],
            "Hey"
        );
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
    fn translate_response_gemini_to_openai_preserves_response_logprobs() {
        let body = json!({
            "response": {
                "responseId": "gem_resp_logprobs_chat",
                "modelVersion": "gemini-2.5",
                "candidates": [{
                    "content": {
                        "role": "model",
                        "parts": [{ "text": "Hi" }]
                    },
                    "finishReason": "STOP",
                    "avgLogprobs": -0.1,
                    "logprobsResult": {
                        "logProbabilitySum": -0.1,
                        "chosenCandidates": [{
                            "token": "Hi",
                            "tokenId": 42,
                            "logProbability": -0.1
                        }],
                        "topCandidates": [{
                            "candidates": [
                                { "token": "Hi", "tokenId": 42, "logProbability": -0.1 },
                                { "token": "Hey", "tokenId": 43, "logProbability": -0.4 }
                            ]
                        }]
                    }
                }]
            }
        });

        let out = translate_response(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .expect("Gemini candidate logprobs should map to Chat choice.logprobs");

        assert_eq!(out["choices"][0]["message"]["content"], "Hi");
        assert_eq!(out["choices"][0]["logprobs"]["content"][0]["token"], "Hi");
        assert_eq!(
            out["choices"][0]["logprobs"]["content"][0]["top_logprobs"][1]["token"],
            "Hey"
        );
    }

    #[test]
    fn translate_response_gemini_to_responses_preserves_response_logprobs() {
        let body = json!({
            "response": {
                "responseId": "gem_resp_logprobs_responses",
                "modelVersion": "gemini-2.5",
                "candidates": [{
                    "content": {
                        "role": "model",
                        "parts": [{ "text": "Hi" }]
                    },
                    "finishReason": "STOP",
                    "avgLogprobs": -0.1,
                    "logprobsResult": {
                        "logProbabilitySum": -0.1,
                        "chosenCandidates": [{
                            "token": "Hi",
                            "tokenId": 42,
                            "logProbability": -0.1
                        }],
                        "topCandidates": [{
                            "candidates": [
                                { "token": "Hi", "tokenId": 42, "logProbability": -0.1 },
                                { "token": "Hey", "tokenId": 43, "logProbability": -0.4 }
                            ]
                        }]
                    }
                }]
            }
        });

        let out = translate_response(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiResponses,
            &body,
        )
        .expect("Gemini candidate logprobs should map to Responses output_text.logprobs");

        let content = out["output"][0]["content"].as_array().expect("responses content");
        assert_eq!(content[0]["type"], "output_text");
        assert_eq!(content[0]["text"], "Hi");
        assert_eq!(content[0]["logprobs"][0]["token"], "Hi");
        assert_eq!(content[0]["logprobs"][0]["top_logprobs"][1]["token"], "Hey");
    }

    #[test]
    fn translate_response_gemini_to_non_gemini_rejects_avg_logprobs_without_token_detail() {
        let body = json!({
            "response": {
                "responseId": "gem_resp_avg_only",
                "modelVersion": "gemini-2.5",
                "candidates": [{
                    "content": {
                        "role": "model",
                        "parts": [{ "text": "Hi" }]
                    },
                    "finishReason": "STOP",
                    "avgLogprobs": -0.1
                }]
            }
        });

        for client_format in [
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
        ] {
            let err = translate_response(UpstreamFormat::Google, client_format, &body)
                .expect_err("Gemini avgLogprobs without logprobsResult should fail closed");
            assert!(err.contains("avgLogprobs"), "err = {err}");
            assert!(err.contains("logprobsResult"), "err = {err}");
        }
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
