use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::formats::UpstreamFormat;

use super::messages::{
    custom_tools_not_portable_message, reserved_openai_custom_bridge_prefix_message,
    translation_target_label,
};
use super::models::{
    NormalizedOpenAiFamilyCustomTool, NormalizedOpenAiFamilyFunctionTool,
    NormalizedOpenAiFamilyNamespaceTool, NormalizedOpenAiFamilyToolCall,
    NormalizedOpenAiFamilyToolDef, SemanticTextPart, SemanticToolKind, SemanticToolResultContent,
};

pub(crate) fn anthropic_tool_use_type_for_openai_tool_call(
    tool_call: &Value,
) -> Result<&'static str, String> {
    validate_openai_public_tool_identity(tool_call)?;
    match semantic_tool_kind_from_value(tool_call) {
        SemanticToolKind::OpenAiCustom => {
            Err(custom_tools_not_portable_message(UpstreamFormat::Anthropic))
        }
        SemanticToolKind::AnthropicServerTool => {
            if let Some(name) = tool_call
                .get("function")
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
            {
                validate_public_tool_name_not_reserved(name)?;
            }
            Ok("server_tool_use")
        }
        SemanticToolKind::Function => {
            if let Some(name) = tool_call
                .get("function")
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
            {
                validate_public_tool_name_not_reserved(name)?;
            }
            Ok("tool_use")
        }
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

pub(crate) fn semantic_tool_output_item_type(kind: SemanticToolKind) -> &'static str {
    match kind {
        SemanticToolKind::OpenAiCustom => "custom_tool_call_output",
        SemanticToolKind::AnthropicServerTool | SemanticToolKind::Function => {
            "function_call_output"
        }
    }
}

pub(crate) fn responses_item_is_tool_output(item: &Value) -> bool {
    matches!(
        item.get("type").and_then(Value::as_str),
        Some("function_call_output") | Some("custom_tool_call_output")
    )
}

pub(crate) fn content_value_is_effectively_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(text) => text.is_empty(),
        Value::Array(items) => items.is_empty(),
        _ => false,
    }
}

pub(crate) fn semantic_tool_result_content_from_value(
    content: Option<&Value>,
) -> SemanticToolResultContent {
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

pub(crate) fn semantic_tool_result_content_to_value(content: &SemanticToolResultContent) -> Value {
    match content {
        SemanticToolResultContent::Text(text) => Value::String(text.clone()),
        SemanticToolResultContent::Json(value) => value.clone(),
        SemanticToolResultContent::TypedBlocks(items) => Value::Array(items.clone()),
    }
}

pub(crate) fn openai_tool_result_content_to_responses_output(
    content: Option<&Value>,
) -> Result<Value, String> {
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

pub(crate) fn responses_tool_output_to_openai_tool_content(
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

pub(crate) fn semantic_text_part_from_claude_block(block: &Value) -> Option<SemanticTextPart> {
    let text = block.get("text").and_then(Value::as_str)?.to_string();
    let annotations = block
        .get("citations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Some(SemanticTextPart { text, annotations })
}

pub(crate) fn semantic_text_part_from_openai_part(part: &Value) -> Option<SemanticTextPart> {
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

pub(crate) fn semantic_text_part_to_openai_value(part: &SemanticTextPart) -> Value {
    let mut value = serde_json::json!({
        "type": "text",
        "text": part.text,
    });
    if !part.annotations.is_empty() {
        value["annotations"] = Value::Array(part.annotations.clone());
    }
    value
}

pub(crate) fn semantic_text_part_to_responses_value(
    part: &SemanticTextPart,
    content_type: &str,
) -> Value {
    let mut value = serde_json::json!({
        "type": content_type,
        "text": part.text,
    });
    if !part.annotations.is_empty() {
        value["annotations"] = Value::Array(part.annotations.clone());
    }
    value
}

pub(crate) fn openai_custom_tool_payload(value: &Value) -> Option<&serde_json::Map<String, Value>> {
    if value.get("type").and_then(Value::as_str) != Some("custom") {
        return None;
    }
    value
        .get("custom")
        .and_then(Value::as_object)
        .or_else(|| value.get("function").and_then(Value::as_object))
        .or_else(|| value.as_object().filter(|obj| obj.get("name").is_some()))
}

pub(crate) fn openai_custom_tool_name(value: &Value) -> Option<&str> {
    openai_custom_tool_payload(value)?
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
}

pub(crate) fn openai_custom_tool_input_raw(value: &Value) -> Option<&str> {
    let payload = openai_custom_tool_payload(value)?;
    payload
        .get("input")
        .or_else(|| payload.get("arguments"))
        .and_then(Value::as_str)
}

pub(crate) fn openai_custom_tool_format_is_plain_text(format: Option<&Value>) -> bool {
    match format {
        None | Some(Value::Null) => true,
        Some(Value::String(kind)) => kind == "text",
        Some(Value::Object(object)) => {
            object.get("type").and_then(Value::as_str) == Some("text") && object.len() == 1
        }
        _ => false,
    }
}

fn openai_custom_tool_string_constraint_format_parts(
    format: Option<&Value>,
) -> Option<(&str, &str)> {
    let Value::Object(object) = format? else {
        return None;
    };
    if object.get("type").and_then(Value::as_str) != Some("grammar") {
        return None;
    }

    let direct_syntax = object.get("syntax").and_then(Value::as_str);
    let direct_definition = object.get("definition").and_then(Value::as_str);
    if let (Some(syntax), Some(definition)) = (direct_syntax, direct_definition) {
        let definition = definition.trim();
        if !syntax.is_empty() && !definition.is_empty() {
            return Some((syntax, definition));
        }
    }

    let grammar = object.get("grammar").and_then(Value::as_object)?;
    let syntax = grammar.get("syntax").and_then(Value::as_str)?;
    let definition = grammar.get("definition").and_then(Value::as_str)?.trim();
    if syntax.is_empty() || definition.is_empty() {
        return None;
    }
    Some((syntax, definition))
}

pub(crate) fn openai_custom_tool_format_supports_anthropic_bridge(format: Option<&Value>) -> bool {
    openai_custom_tool_format_is_plain_text(format)
        || openai_custom_tool_string_constraint_format_parts(format).is_some()
}

pub(crate) fn openai_custom_tool_bridge_description_for_target(
    target_format: UpstreamFormat,
    description: Option<&Value>,
    format: Option<&Value>,
) -> Option<Value> {
    let Some((syntax, definition)) = openai_custom_tool_string_constraint_format_parts(format)
    else {
        return description.cloned();
    };
    let target_label = translation_target_label(target_format);

    let mut bridged = description
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_default();
    if !bridged.is_empty() {
        bridged.push_str("\n\n");
    }
    bridged.push_str(
        &format!(
            "Bridge note: {target_label} receives this tool through the canonical `{{ \"input\": string }}` wrapper. "
        ),
    );
    bridged.push_str(
        "The original OpenAI custom tool constrained that single string input with the following format contract. ",
    );
    bridged.push_str(&format!(
        "{target_label} will not enforce it structurally, so follow it exactly.\n"
    ));
    bridged.push_str(&format!("syntax: {syntax}\n"));
    bridged.push_str(definition);
    Some(Value::String(bridged))
}

pub(crate) const OPENAI_RESPONSES_CUSTOM_BRIDGE_PREFIX: &str = "__llmup_custom__";
pub(crate) const REQUEST_SCOPED_TOOL_BRIDGE_CONTEXT_FIELD: &str = "_llmup_tool_bridge_context";
const TOOL_BRIDGE_CONTEXT_VERSION: u64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolBridgeSourceKind {
    CustomText,
    CustomGrammar,
}

impl ToolBridgeSourceKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::CustomText => "custom_text",
            Self::CustomGrammar => "custom_grammar",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "custom_text" => Some(Self::CustomText),
            "custom_grammar" => Some(Self::CustomGrammar),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolBridgeTransportKind {
    FunctionObjectWrapper,
}

impl ToolBridgeTransportKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::FunctionObjectWrapper => "function_object_wrapper",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "function_object_wrapper" => Some(Self::FunctionObjectWrapper),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolBridgeWrapperField {
    Input,
}

impl ToolBridgeWrapperField {
    fn as_str(self) -> &'static str {
        match self {
            Self::Input => "input",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "input" => Some(Self::Input),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolBridgeCanonicalShape {
    SingleRequiredString,
}

impl ToolBridgeCanonicalShape {
    fn as_str(self) -> &'static str {
        match self {
            Self::SingleRequiredString => "single_required_string",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "single_required_string" => Some(Self::SingleRequiredString),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolBridgeContextEntry {
    pub(crate) stable_name: String,
    pub(crate) source_kind: ToolBridgeSourceKind,
    pub(crate) transport_kind: ToolBridgeTransportKind,
    pub(crate) wrapper_field: ToolBridgeWrapperField,
    pub(crate) expected_canonical_shape: ToolBridgeCanonicalShape,
}

impl ToolBridgeContextEntry {
    fn from_custom(custom: &NormalizedOpenAiFamilyCustomTool) -> Self {
        Self {
            stable_name: custom.name.clone(),
            source_kind: request_scoped_custom_bridge_source_kind(custom),
            transport_kind: ToolBridgeTransportKind::FunctionObjectWrapper,
            wrapper_field: ToolBridgeWrapperField::Input,
            expected_canonical_shape: ToolBridgeCanonicalShape::SingleRequiredString,
        }
    }

    fn from_value(stable_name: &str, value: &Value) -> Option<Self> {
        if openai_responses_custom_tool_bridge_prefix_is_reserved(stable_name) {
            return None;
        }
        let object = value.as_object()?;
        let declared_stable_name = object.get("stable_name").and_then(Value::as_str)?;
        if declared_stable_name.is_empty() || declared_stable_name != stable_name {
            return None;
        }
        Some(Self {
            stable_name: stable_name.to_string(),
            source_kind: ToolBridgeSourceKind::from_str(
                object.get("source_kind").and_then(Value::as_str)?,
            )?,
            transport_kind: ToolBridgeTransportKind::from_str(
                object.get("transport_kind").and_then(Value::as_str)?,
            )?,
            wrapper_field: ToolBridgeWrapperField::from_str(
                object.get("wrapper_field").and_then(Value::as_str)?,
            )?,
            expected_canonical_shape: ToolBridgeCanonicalShape::from_str(
                object
                    .get("expected_canonical_shape")
                    .and_then(Value::as_str)?,
            )?,
        })
    }

    fn to_value(&self) -> Value {
        serde_json::json!({
            "stable_name": self.stable_name,
            "source_kind": self.source_kind.as_str(),
            "transport_kind": self.transport_kind.as_str(),
            "wrapper_field": self.wrapper_field.as_str(),
            "expected_canonical_shape": self.expected_canonical_shape.as_str()
        })
    }

    fn expects_canonical_input_wrapper(&self) -> bool {
        self.transport_kind == ToolBridgeTransportKind::FunctionObjectWrapper
            && self.wrapper_field == ToolBridgeWrapperField::Input
            && self.expected_canonical_shape == ToolBridgeCanonicalShape::SingleRequiredString
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolBridgeContext {
    pub(crate) version: u64,
    pub(crate) compatibility_mode: String,
    pub(crate) entries: BTreeMap<String, ToolBridgeContextEntry>,
}

impl ToolBridgeContext {
    fn new(compatibility_mode: &str) -> Self {
        Self {
            version: TOOL_BRIDGE_CONTEXT_VERSION,
            compatibility_mode: compatibility_mode.to_string(),
            entries: BTreeMap::new(),
        }
    }

    pub(crate) fn from_value(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        let version = object.get("version").and_then(Value::as_u64)?;
        if version != TOOL_BRIDGE_CONTEXT_VERSION {
            return None;
        }
        let compatibility_mode = object.get("compatibility_mode").and_then(Value::as_str)?;
        if !matches!(compatibility_mode, "strict" | "balanced" | "max_compat") {
            return None;
        }
        let entries_object = object.get("entries").and_then(Value::as_object)?;
        let mut entries = BTreeMap::new();
        for (stable_name, entry_value) in entries_object {
            let entry = ToolBridgeContextEntry::from_value(stable_name, entry_value)?;
            entries.insert(stable_name.clone(), entry);
        }
        if entries.is_empty() {
            return None;
        }
        Some(Self {
            version,
            compatibility_mode: compatibility_mode.to_string(),
            entries,
        })
    }

    pub(crate) fn to_value(&self) -> Value {
        let entries = self
            .entries
            .iter()
            .map(|(stable_name, entry)| (stable_name.clone(), entry.to_value()))
            .collect::<serde_json::Map<_, _>>();
        serde_json::json!({
            "version": self.version,
            "compatibility_mode": self.compatibility_mode,
            "entries": entries
        })
    }

    pub(crate) fn set_compatibility_mode(&mut self, compatibility_mode: &str) {
        self.compatibility_mode = compatibility_mode.to_string();
    }

    pub(crate) fn expects_canonical_input_wrapper(&self, name: &str) -> bool {
        self.entries
            .get(name)
            .is_some_and(ToolBridgeContextEntry::expects_canonical_input_wrapper)
    }
}

pub(crate) fn request_scoped_tool_bridge_context_from_body(
    body: &Value,
) -> Option<ToolBridgeContext> {
    body.get(REQUEST_SCOPED_TOOL_BRIDGE_CONTEXT_FIELD)
        .and_then(ToolBridgeContext::from_value)
}

pub(crate) fn insert_request_scoped_tool_bridge_context(
    body: &mut Value,
    bridge_context: &ToolBridgeContext,
) {
    if let Some(object) = body.as_object_mut() {
        object.insert(
            REQUEST_SCOPED_TOOL_BRIDGE_CONTEXT_FIELD.to_string(),
            bridge_context.to_value(),
        );
    }
}

pub(crate) fn openai_responses_custom_tool_bridge_arguments(input: &str) -> Result<String, String> {
    serde_json::to_string(&serde_json::json!({ "input": input }))
        .map_err(|err| format!("serialize OpenAI Responses custom tool bridge arguments: {err}"))
}

pub(crate) fn openai_responses_custom_tool_input_from_bridge_value(
    value: &Value,
) -> Option<String> {
    let object = value.as_object()?;
    if object.len() != 1 {
        return None;
    }
    object
        .get("input")
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub(crate) fn openai_responses_custom_tool_input_from_bridge_arguments(
    arguments: &str,
) -> Result<String, String> {
    let value: Value = serde_json::from_str(arguments)
        .map_err(|err| format!("decode OpenAI Responses custom tool bridge arguments: {err}"))?;
    openai_responses_custom_tool_input_from_bridge_value(&value).ok_or(
        "OpenAI Responses custom tool bridge arguments must be the canonical object `{ \"input\": string }`."
            .to_string(),
    )
}

pub(crate) fn openai_responses_custom_tool_bridge_prefix_is_reserved(name: &str) -> bool {
    name.starts_with(OPENAI_RESPONSES_CUSTOM_BRIDGE_PREFIX)
}

pub(crate) fn validate_public_tool_name_not_reserved(name: &str) -> Result<(), String> {
    if openai_responses_custom_tool_bridge_prefix_is_reserved(name) {
        return Err(reserved_openai_custom_bridge_prefix_message(name));
    }
    Ok(())
}

fn validate_public_tool_identity_value_not_reserved(value: Option<&Value>) -> Result<(), String> {
    if let Some(value) = value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        validate_public_tool_name_not_reserved(value)?;
    }
    Ok(())
}

fn selector_container_key_is_visible_identity_carrier(key: &str) -> bool {
    matches!(
        key,
        "function"
            | "custom"
            | "tool"
            | "tools"
            | "allowed_tool"
            | "allowed_tools"
            | "allowedTools"
            | "selected_tool"
            | "selected_tools"
            | "selectedTools"
            | "allowedFunctionNames"
            | "allowed_function_names"
            | "functionCallingConfig"
            | "function_calling_config"
            | "toolConfig"
            | "tool_config"
    )
}

fn validate_public_selector_visible_identity_at(value: &Value, path: &str) -> Result<(), String> {
    if let Some(name) = value.as_str().filter(|value| !value.is_empty()) {
        return validate_public_tool_name_not_reserved(name)
            .map_err(|err| format!("{path}: {err}"));
    }

    match value {
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                validate_public_selector_visible_identity_at(item, &format!("{path}[{index}]"))?;
            }
        }
        Value::Object(object) => {
            for key in ["name", "namespace"] {
                if let Some(name) = object.get(key).and_then(Value::as_str) {
                    validate_public_tool_name_not_reserved(name)
                        .map_err(|err| format!("{path}.{key}: {err}"))?;
                }
            }
            for (key, nested) in object {
                if selector_container_key_is_visible_identity_carrier(key) {
                    validate_public_selector_visible_identity_at(nested, &format!("{path}.{key}"))?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn validate_public_selector_visible_identity(value: &Value) -> Result<(), String> {
    validate_public_selector_visible_identity_at(value, "$")
}

pub(crate) fn validate_public_selector_visible_identities(value: &Value) -> Result<(), String> {
    validate_public_selector_visible_identity(value)
}

pub(crate) fn validate_openai_public_tool_identity(value: &Value) -> Result<(), String> {
    validate_public_tool_identity_value_not_reserved(Some(value))?;
    validate_public_tool_identity_value_not_reserved(value.get("name"))?;
    validate_public_tool_identity_value_not_reserved(
        value
            .get("function")
            .and_then(|function| function.get("name")),
    )?;
    validate_public_tool_identity_value_not_reserved(
        value.get("custom").and_then(|custom| custom.get("name")),
    )?;
    Ok(())
}

pub(crate) fn validate_openai_public_tool_choice_identity(choice: &Value) -> Result<(), String> {
    validate_public_selector_visible_identity(choice)?;
    validate_openai_public_tool_identity(choice)?;
    let Some(choice_obj) = choice.as_object() else {
        return Ok(());
    };

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
        validate_openai_public_tool_identity(tool)?;
    }
    Ok(())
}

fn validate_responses_public_nested_tool_reference_identity(
    value: Option<&Value>,
) -> Result<(), String> {
    let Some(value) = value else {
        return Ok(());
    };
    validate_public_tool_identity_value_not_reserved(Some(value))?;
    validate_public_tool_identity_value_not_reserved(value.get("name"))?;
    validate_public_tool_identity_value_not_reserved(value.get("namespace"))?;
    Ok(())
}

fn validate_responses_public_tool_reference_identity(value: &Value) -> Result<(), String> {
    validate_public_selector_visible_identity(value)?;
    validate_public_tool_identity_value_not_reserved(Some(value))?;
    validate_public_tool_identity_value_not_reserved(value.get("name"))?;
    validate_public_tool_identity_value_not_reserved(value.get("namespace"))?;
    validate_responses_public_nested_tool_reference_identity(value.get("function"))?;
    validate_responses_public_nested_tool_reference_identity(value.get("custom"))?;
    Ok(())
}

pub(crate) fn validate_responses_public_tool_choice_identity(choice: &Value) -> Result<(), String> {
    validate_responses_public_tool_reference_identity(choice)?;
    let Some(choice_obj) = choice.as_object() else {
        return Ok(());
    };

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
        validate_responses_public_tool_reference_identity(tool)?;
    }
    Ok(())
}

pub(crate) fn validate_responses_public_tool_metadata_identity(
    value: &Value,
) -> Result<(), String> {
    if let Some(tools) = value.get("tools").and_then(Value::as_array) {
        for tool in tools {
            validate_responses_public_tool_reference_identity(tool)?;
        }
    }
    if let Some(tool_choice) = value.get("tool_choice").filter(|value| !value.is_null()) {
        validate_responses_public_tool_choice_identity(tool_choice)?;
    }
    Ok(())
}

pub(crate) fn validate_responses_public_request_object_tool_identity(
    value: &Value,
) -> Result<(), String> {
    validate_responses_public_tool_metadata_identity(value)?;
    if let Some(items) = value.get("input").and_then(Value::as_array) {
        for item in items {
            validate_responses_public_tool_call_item_identity(item)?;
        }
    }
    Ok(())
}

pub(crate) fn validate_responses_public_stream_event_tool_identity(
    event: &Value,
) -> Result<(), String> {
    validate_responses_public_tool_metadata_identity(event)?;
    if matches!(
        event.get("type").and_then(Value::as_str),
        Some(
            "response.function_call_arguments.delta"
                | "response.function_call_arguments.done"
                | "response.custom_tool_call_input.delta"
                | "response.custom_tool_call_input.done"
        )
    ) {
        validate_public_tool_identity_value_not_reserved(event.get("name"))?;
        validate_public_tool_identity_value_not_reserved(event.get("namespace"))?;
    }
    Ok(())
}

pub(crate) fn validate_responses_public_tool_call_item_identity(
    item: &Value,
) -> Result<(), String> {
    if matches!(
        item.get("type").and_then(Value::as_str),
        Some("function_call" | "custom_tool_call")
    ) {
        validate_public_tool_identity_value_not_reserved(item.get("name"))?;
        validate_public_tool_identity_value_not_reserved(item.get("namespace"))?;
    }
    Ok(())
}

pub(crate) fn validate_responses_public_output_tool_identity(output: &Value) -> Result<(), String> {
    let Some(items) = output.as_array() else {
        return Ok(());
    };
    for item in items {
        validate_responses_public_tool_call_item_identity(item)?;
    }
    Ok(())
}

pub(crate) fn validate_responses_public_response_object_tool_identity(
    response: &Value,
) -> Result<(), String> {
    validate_responses_public_tool_metadata_identity(response)?;
    if let Some(output) = response.get("output") {
        validate_responses_public_output_tool_identity(output)?;
    }
    Ok(())
}

fn request_scoped_custom_bridge_source_kind(
    custom: &NormalizedOpenAiFamilyCustomTool,
) -> ToolBridgeSourceKind {
    if openai_custom_tool_format_is_plain_text(custom.format.as_ref()) {
        ToolBridgeSourceKind::CustomText
    } else {
        ToolBridgeSourceKind::CustomGrammar
    }
}

pub(crate) fn request_scoped_openai_custom_bridge_context(
    tools: &[NormalizedOpenAiFamilyToolDef],
) -> Option<ToolBridgeContext> {
    let mut bridge_context = ToolBridgeContext::new("balanced");
    for tool in tools {
        let NormalizedOpenAiFamilyToolDef::Custom(custom) = tool else {
            continue;
        };
        bridge_context.entries.insert(
            custom.name.clone(),
            ToolBridgeContextEntry::from_custom(custom),
        );
    }
    if bridge_context.entries.is_empty() {
        None
    } else {
        Some(bridge_context)
    }
}

pub(crate) fn request_scoped_openai_custom_bridge_conflict_name(
    tools: &[NormalizedOpenAiFamilyToolDef],
) -> Option<String> {
    let mut function_names = BTreeSet::new();
    let mut custom_names = BTreeSet::new();
    for tool in tools {
        match tool {
            NormalizedOpenAiFamilyToolDef::Function(function) => {
                function_names.insert(function.name.clone());
            }
            NormalizedOpenAiFamilyToolDef::Custom(custom) => {
                custom_names.insert(custom.name.clone());
            }
            NormalizedOpenAiFamilyToolDef::Namespace(_) => {}
        }
    }
    function_names
        .into_iter()
        .find(|name| custom_names.contains(name))
}

pub(crate) fn openai_tool_arguments_raw(tool_call: &Value) -> Option<&str> {
    tool_call
        .get("function")
        .and_then(|function| function.get("arguments"))
        .or_else(|| tool_call.get("arguments"))
        .and_then(Value::as_str)
}

pub(crate) const INTERNAL_NON_REPLAYABLE_TOOL_CALL_FIELD: &str = "_llmup_non_replayable_tool_call";
const INTERNAL_NON_REPLAYABLE_TOOL_CALL_REASON: &str = "incomplete_arguments";
const INTERNAL_NON_REPLAYABLE_TOOL_CALL_VERSION: u64 = 1;
const INTERNAL_NON_REPLAYABLE_TOOL_CALL_SIGNATURE_FIELD: &str = "sig";
const INTERNAL_REPLAY_MARKER_KEY_ENV: &str = "LLMUP_INTERNAL_REPLAY_MARKER_KEY";

fn internal_replay_marker_key() -> &'static str {
    static KEY: OnceLock<String> = OnceLock::new();
    KEY.get_or_init(|| {
        if let Some(existing) = std::env::var(INTERNAL_REPLAY_MARKER_KEY_ENV)
            .ok()
            .filter(|value| !value.is_empty())
        {
            return existing;
        }
        let generated = Uuid::new_v4().to_string();
        std::env::set_var(INTERNAL_REPLAY_MARKER_KEY_ENV, &generated);
        generated
    })
}

fn non_replayable_tool_call_name_and_raw(value: &Value) -> (Option<&str>, Option<&str>) {
    let name = value
        .get("function")
        .and_then(|function| function.get("name"))
        .and_then(Value::as_str)
        .or_else(|| openai_custom_tool_name(value))
        .or_else(|| value.get("name").and_then(Value::as_str));
    let raw = openai_tool_arguments_raw(value)
        .or_else(|| openai_custom_tool_input_raw(value))
        .or_else(|| responses_tool_call_input_raw(value));
    (name, raw)
}

fn non_replayable_tool_call_signature(value: &Value) -> Option<String> {
    let (name, raw) = non_replayable_tool_call_name_and_raw(value);
    let payload = serde_json::json!({
        "v": INTERNAL_NON_REPLAYABLE_TOOL_CALL_VERSION,
        "reason": INTERNAL_NON_REPLAYABLE_TOOL_CALL_REASON,
        "name": name.unwrap_or(""),
        "raw": raw.unwrap_or("")
    });
    let encoded = serde_json::to_vec(&payload).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(internal_replay_marker_key().as_bytes());
    hasher.update([0]);
    hasher.update(encoded);
    Some(hex::encode(hasher.finalize()))
}

fn trusted_non_replayable_tool_call_marker(value: &Value) -> Option<Value> {
    let marker = value
        .get(INTERNAL_NON_REPLAYABLE_TOOL_CALL_FIELD)?
        .as_object()?;
    if marker.get("reason").and_then(Value::as_str)
        != Some(INTERNAL_NON_REPLAYABLE_TOOL_CALL_REASON)
    {
        return None;
    }
    if marker.get("v").and_then(Value::as_u64) != Some(INTERNAL_NON_REPLAYABLE_TOOL_CALL_VERSION) {
        return None;
    }
    let signature = marker
        .get(INTERNAL_NON_REPLAYABLE_TOOL_CALL_SIGNATURE_FIELD)
        .and_then(Value::as_str)?;
    (Some(signature) == non_replayable_tool_call_signature(value).as_deref())
        .then(|| Value::Object(marker.clone()))
}

fn tool_call_text_for_partial_replay(name: Option<&str>, raw: Option<&str>) -> String {
    let name = name.unwrap_or("unknown_tool");
    match raw.map(str::trim).filter(|raw| !raw.is_empty()) {
        Some(raw) => format!("Tool call `{name}` with partial arguments: {raw}"),
        None => format!("Tool call `{name}` had incomplete arguments."),
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn mark_tool_call_as_non_replayable(value: &mut Value) {
    let Some(signature) = non_replayable_tool_call_signature(value) else {
        return;
    };
    let Some(obj) = value.as_object_mut() else {
        return;
    };
    obj.insert(
        INTERNAL_NON_REPLAYABLE_TOOL_CALL_FIELD.to_string(),
        serde_json::json!({
            "reason": INTERNAL_NON_REPLAYABLE_TOOL_CALL_REASON,
            "v": INTERNAL_NON_REPLAYABLE_TOOL_CALL_VERSION,
            INTERNAL_NON_REPLAYABLE_TOOL_CALL_SIGNATURE_FIELD: signature
        }),
    );
}

pub(crate) fn tool_call_is_marked_non_replayable(value: &Value) -> bool {
    trusted_non_replayable_tool_call_marker(value).is_some()
}

pub(crate) fn copy_non_replayable_tool_call_marker(source: &Value, dest: &mut Value) {
    if trusted_non_replayable_tool_call_marker(source).is_none() {
        return;
    }
    // The signature is bound to the current tool-call shape, so bridge rewrites
    // must re-attest the destination instead of copying the source marker verbatim.
    mark_tool_call_as_non_replayable(dest);
}

pub(crate) fn openai_tool_call_partial_replay_text(tool_call: &Value) -> String {
    tool_call_text_for_partial_replay(
        tool_call
            .get("function")
            .and_then(|function| function.get("name"))
            .and_then(Value::as_str)
            .or_else(|| openai_custom_tool_name(tool_call)),
        openai_tool_arguments_raw(tool_call).or_else(|| openai_custom_tool_input_raw(tool_call)),
    )
}

pub(crate) fn responses_tool_call_partial_replay_text(item: &Value) -> String {
    tool_call_text_for_partial_replay(
        item.get("name").and_then(Value::as_str),
        responses_tool_call_input_raw(item),
    )
}

pub(crate) fn openai_function_tool_payload(
    value: &Value,
) -> Option<&serde_json::Map<String, Value>> {
    if value.get("type").and_then(Value::as_str) != Some("function") {
        return None;
    }
    value
        .get("function")
        .and_then(Value::as_object)
        .or_else(|| value.as_object().filter(|obj| obj.get("name").is_some()))
}

pub(crate) fn openai_tool_arguments_to_structured_value(
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

pub(crate) fn raw_json_object_to_structured_value(
    raw: &str,
    target_label: &str,
) -> Result<Value, String> {
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

pub(crate) fn normalized_openai_tool_definition(
    tool: &Value,
) -> Result<Option<NormalizedOpenAiFamilyToolDef>, String> {
    validate_openai_public_tool_identity(tool)?;
    match tool.get("type").and_then(Value::as_str) {
        Some("function") => {
            let payload = openai_function_tool_payload(tool)
                .ok_or("OpenAI function tools require a `function` payload.".to_string())?;
            let name = payload
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .ok_or("OpenAI function tools require a non-empty function name.".to_string())?;
            validate_public_tool_name_not_reserved(name)?;
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
            validate_public_tool_name_not_reserved(name)?;
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

pub(crate) fn normalized_responses_tool_definition(
    tool: &Value,
) -> Result<Option<NormalizedOpenAiFamilyToolDef>, String> {
    validate_responses_public_tool_reference_identity(tool)?;
    match tool.get("type").and_then(Value::as_str) {
        Some("function") => {
            let name = tool
                .get("name")
                .or_else(|| {
                    tool.get("function")
                        .and_then(|function| function.get("name"))
                })
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .ok_or("OpenAI Responses function tools require a non-empty name.".to_string())?;
            validate_public_tool_name_not_reserved(name)?;
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
            validate_public_tool_name_not_reserved(name)?;
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
            validate_public_tool_name_not_reserved(name)?;
            Ok(Some(NormalizedOpenAiFamilyToolDef::Namespace(
                NormalizedOpenAiFamilyNamespaceTool {
                    name: name.to_string(),
                },
            )))
        }
        _ => Ok(None),
    }
}

fn openai_function_uses_canonical_custom_bridge_wrapper(
    function: &NormalizedOpenAiFamilyFunctionTool,
) -> bool {
    function.parameters.as_ref()
        == Some(&serde_json::json!({
            "type": "object",
            "properties": {
                "input": { "type": "string" }
            },
            "required": ["input"],
            "additionalProperties": false
        }))
}

pub(crate) fn normalized_openai_tool_definitions_from_request_with_request_scoped_custom_bridge(
    body: &Value,
    bridge_context: Option<&ToolBridgeContext>,
) -> Result<Vec<NormalizedOpenAiFamilyToolDef>, String> {
    body.get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools.iter().try_fold(Vec::new(), |mut normalized, tool| {
                let Some(tool) = normalized_openai_tool_definition(tool)? else {
                    return Ok(normalized);
                };
                let tool = match tool {
                    NormalizedOpenAiFamilyToolDef::Function(function)
                        if request_scoped_openai_custom_bridge_expects_canonical_input_wrapper(
                            bridge_context,
                            &function.name,
                        ) && openai_function_uses_canonical_custom_bridge_wrapper(&function) =>
                    {
                        NormalizedOpenAiFamilyToolDef::Custom(NormalizedOpenAiFamilyCustomTool {
                            name: function.name,
                            description: function.description,
                            format: None,
                        })
                    }
                    other => other,
                };
                normalized.push(tool);
                Ok(normalized)
            })
        })
        .unwrap_or_else(|| Ok(Vec::new()))
}

pub(crate) fn normalized_responses_tool_definitions_from_request(
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

pub(crate) fn normalized_tool_definition_to_openai(
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

pub(crate) fn normalized_tool_definition_to_openai_with_request_scoped_custom_bridge(
    tool: &NormalizedOpenAiFamilyToolDef,
    target_format: UpstreamFormat,
) -> Result<Value, String> {
    match tool {
        NormalizedOpenAiFamilyToolDef::Custom(custom) => {
            let mut payload = serde_json::Map::new();
            payload.insert("name".to_string(), Value::String(custom.name.clone()));
            let description = openai_custom_tool_bridge_description_for_target(
                target_format,
                custom.description.as_ref(),
                custom.format.as_ref(),
            );
            if let Some(description) = description {
                payload.insert("description".to_string(), description);
            }
            payload.insert(
                "parameters".to_string(),
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "input": { "type": "string" }
                    },
                    "required": ["input"],
                    "additionalProperties": false
                }),
            );
            Ok(serde_json::json!({
                "type": "function",
                "function": Value::Object(payload)
            }))
        }
        _ => normalized_tool_definition_to_openai(tool),
    }
}

pub(crate) fn normalized_tool_definition_to_responses(
    tool: &NormalizedOpenAiFamilyToolDef,
) -> Value {
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

pub(crate) fn responses_tool_call_input_raw(item: &Value) -> Option<&str> {
    match semantic_tool_kind_from_value(item) {
        SemanticToolKind::OpenAiCustom => item.get("input").and_then(Value::as_str),
        _ => item.get("arguments").and_then(Value::as_str),
    }
}

pub(crate) fn responses_tool_call_to_structured_value(
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

pub(crate) fn normalized_openai_tool_call(
    tool_call: &Value,
) -> Result<Option<NormalizedOpenAiFamilyToolCall>, String> {
    validate_openai_public_tool_identity(tool_call)?;
    let Some(tool_type) = tool_call.get("type").and_then(Value::as_str) else {
        return Ok(None);
    };
    match tool_type {
        "function" => {
            let Some(function) = tool_call.get("function").and_then(Value::as_object) else {
                return Err("OpenAI function tool calls require a `function` payload.".to_string());
            };
            let name = function
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .ok_or(
                    "OpenAI function tool calls require a non-empty function name.".to_string(),
                )?;
            validate_public_tool_name_not_reserved(name)?;
            Ok(Some(NormalizedOpenAiFamilyToolCall::Function {
                id: tool_call.get("id").cloned(),
                name: name.to_string(),
                arguments: openai_tool_arguments_raw(tool_call)
                    .unwrap_or("{}")
                    .to_string(),
                namespace: None,
                proxied_tool_kind: tool_call.get("proxied_tool_kind").cloned(),
            }))
        }
        "custom" => {
            let name = openai_custom_tool_name(tool_call)
                .ok_or("OpenAI custom tool calls require a `custom.name` field.".to_string())?;
            validate_public_tool_name_not_reserved(name)?;
            Ok(Some(NormalizedOpenAiFamilyToolCall::Custom {
                id: tool_call.get("id").cloned(),
                name: name.to_string(),
                input: openai_custom_tool_input_raw(tool_call)
                    .unwrap_or("")
                    .to_string(),
                namespace: None,
                proxied_tool_kind: tool_call.get("proxied_tool_kind").cloned(),
            }))
        }
        _ => Ok(None),
    }
}

pub(crate) fn normalized_responses_tool_call(
    item: &Value,
) -> Result<Option<NormalizedOpenAiFamilyToolCall>, String> {
    match item.get("type").and_then(Value::as_str) {
        Some("function_call") | Some("custom_tool_call") => {}
        _ => return Ok(None),
    }
    validate_responses_public_tool_call_item_identity(item)?;

    let name = item
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .ok_or("OpenAI Responses tool calls require a non-empty name.".to_string())?;
    validate_public_tool_name_not_reserved(name)?;
    let namespace = item
        .get("namespace")
        .and_then(Value::as_str)
        .map(str::to_string);
    let proxied_tool_kind = item.get("proxied_tool_kind").cloned();
    Ok(Some(match semantic_tool_kind_from_value(item) {
        SemanticToolKind::OpenAiCustom => NormalizedOpenAiFamilyToolCall::Custom {
            id: item.get("call_id").cloned(),
            name: name.to_string(),
            input: responses_tool_call_input_raw(item)
                .unwrap_or("")
                .to_string(),
            namespace,
            proxied_tool_kind,
        },
        _ => NormalizedOpenAiFamilyToolCall::Function {
            id: item.get("call_id").cloned(),
            name: name.to_string(),
            arguments: responses_tool_call_input_raw(item)
                .unwrap_or("{}")
                .to_string(),
            namespace,
            proxied_tool_kind,
        },
    }))
}

pub(crate) fn normalized_tool_call_to_openai(call: &NormalizedOpenAiFamilyToolCall) -> Value {
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

pub(crate) fn responses_tool_call_item_to_openai_tool_call(item: &Value) -> Option<Value> {
    normalized_responses_tool_call(item)
        .ok()
        .flatten()
        .map(|tool_call| {
            let mut tool_call = normalized_tool_call_to_openai(&tool_call);
            copy_non_replayable_tool_call_marker(item, &mut tool_call);
            tool_call
        })
}

pub(crate) fn responses_tool_call_item_to_openai_tool_call_strict(
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
        _ => {
            let mut tool_call = normalized_tool_call_to_openai(&tool_call);
            copy_non_replayable_tool_call_marker(item, &mut tool_call);
            Ok(Some(tool_call))
        }
    }
}

pub(crate) fn responses_tool_call_item_to_openai_tool_call_with_request_scoped_custom_bridge_strict(
    item: &Value,
    target_label: &str,
) -> Result<Option<Value>, String> {
    let Some(tool_call) = normalized_responses_tool_call(item)? else {
        return Ok(None);
    };
    match tool_call {
        NormalizedOpenAiFamilyToolCall::Function {
            name,
            namespace: Some(_),
            ..
        } => Err(format!(
            "OpenAI Responses namespaced tool call `{name}` cannot be faithfully translated to {target_label}"
        )),
        NormalizedOpenAiFamilyToolCall::Custom {
            name,
            namespace: Some(_),
            ..
        } => Err(format!(
            "OpenAI Responses namespaced tool call `{name}` cannot be faithfully translated to {target_label}"
        )),
        NormalizedOpenAiFamilyToolCall::Custom {
            id,
            name,
            input,
            proxied_tool_kind,
            ..
        } => {
            let mut tool_call = serde_json::json!({
                "id": id,
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": openai_responses_custom_tool_bridge_arguments(&input)?
                }
            });
            if let Some(proxied_tool_kind) = proxied_tool_kind {
                tool_call["proxied_tool_kind"] = proxied_tool_kind;
            }
            copy_non_replayable_tool_call_marker(item, &mut tool_call);
            Ok(Some(tool_call))
        }
        other => {
            let mut tool_call = normalized_tool_call_to_openai(&other);
            copy_non_replayable_tool_call_marker(item, &mut tool_call);
            Ok(Some(tool_call))
        }
    }
}

fn normalized_tool_call_to_responses_item(call: NormalizedOpenAiFamilyToolCall) -> Value {
    match call {
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
    }
}

pub(crate) fn openai_tool_call_to_responses_item(tool_call: &Value) -> Value {
    let mut item = normalized_openai_tool_call(tool_call)
        .ok()
        .flatten()
        .map(normalized_tool_call_to_responses_item)
        .unwrap_or_else(|| {
            serde_json::json!({
                "type": "function_call",
                "call_id": tool_call.get("id"),
                "name": tool_call.get("function").and_then(|f| f.get("name")),
                "arguments": openai_tool_arguments_raw(tool_call).unwrap_or("{}")
            })
        });
    copy_non_replayable_tool_call_marker(tool_call, &mut item);
    item
}

pub(crate) fn request_scoped_openai_custom_bridge_expects_canonical_input_wrapper(
    bridge_context: Option<&ToolBridgeContext>,
    name: &str,
) -> bool {
    bridge_context.is_some_and(|ctx| ctx.expects_canonical_input_wrapper(name))
}

pub(crate) fn openai_tool_call_to_responses_item_decoding_custom_bridge_with_context(
    tool_call: &Value,
    bridge_context: Option<&ToolBridgeContext>,
) -> Result<Value, String> {
    let call = match normalized_openai_tool_call(tool_call) {
        Ok(Some(call)) => call,
        Ok(None) => {
            return Ok(openai_tool_call_to_responses_item(tool_call));
        }
        Err(err) => return Err(err),
    };

    match call {
        NormalizedOpenAiFamilyToolCall::Function {
            id,
            name,
            arguments,
            namespace,
            proxied_tool_kind,
        } => {
            if request_scoped_openai_custom_bridge_expects_canonical_input_wrapper(
                bridge_context,
                &name,
            ) {
                match openai_responses_custom_tool_input_from_bridge_arguments(&arguments) {
                    Ok(input) => {
                        let mut item = serde_json::json!({
                            "type": "custom_tool_call",
                            "call_id": id,
                            "name": name,
                            "input": input
                        });
                        if let Some(proxied_tool_kind) = proxied_tool_kind {
                            item["proxied_tool_kind"] = proxied_tool_kind;
                        }
                        copy_non_replayable_tool_call_marker(tool_call, &mut item);
                        Ok(item)
                    }
                    Err(_) => {
                        let mut item = normalized_tool_call_to_responses_item(
                            NormalizedOpenAiFamilyToolCall::Function {
                                id,
                                name,
                                arguments,
                                namespace,
                                proxied_tool_kind,
                            },
                        );
                        copy_non_replayable_tool_call_marker(tool_call, &mut item);
                        Ok(item)
                    }
                }
            } else {
                let mut item = normalized_tool_call_to_responses_item(
                    NormalizedOpenAiFamilyToolCall::Function {
                        id,
                        name,
                        arguments,
                        namespace,
                        proxied_tool_kind,
                    },
                );
                copy_non_replayable_tool_call_marker(tool_call, &mut item);
                Ok(item)
            }
        }
        other => {
            let mut item = normalized_tool_call_to_responses_item(other);
            copy_non_replayable_tool_call_marker(tool_call, &mut item);
            Ok(item)
        }
    }
}
