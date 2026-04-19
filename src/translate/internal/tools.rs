use serde_json::Value;

use crate::formats::UpstreamFormat;

use super::messages::{custom_tools_not_portable_message, translation_target_label};
use super::models::{
    NormalizedOpenAiFamilyCustomTool, NormalizedOpenAiFamilyFunctionTool,
    NormalizedOpenAiFamilyNamespaceTool, NormalizedOpenAiFamilyToolCall,
    NormalizedOpenAiFamilyToolDef, SemanticTextPart, SemanticToolKind, SemanticToolResultContent,
};

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

pub(crate) const OPENAI_RESPONSES_CUSTOM_BRIDGE_PREFIX: &str = "__llmup_custom__";

pub(crate) fn openai_responses_custom_tool_bridge_name(name: &str) -> String {
    format!("{OPENAI_RESPONSES_CUSTOM_BRIDGE_PREFIX}{name}")
}

fn openai_responses_custom_tool_name_from_bridge(name: &str) -> Option<&str> {
    name.strip_prefix(OPENAI_RESPONSES_CUSTOM_BRIDGE_PREFIX)
        .filter(|name| !name.is_empty())
}

pub(crate) fn openai_responses_custom_tool_bridge_arguments(input: &str) -> Result<String, String> {
    serde_json::to_string(&serde_json::json!({ "input": input }))
        .map_err(|err| format!("serialize OpenAI Responses custom tool bridge arguments: {err}"))
}

fn openai_responses_custom_tool_input_from_bridge_arguments(
    arguments: &str,
) -> Result<String, String> {
    let value: Value = serde_json::from_str(arguments)
        .map_err(|err| format!("decode OpenAI Responses custom tool bridge arguments: {err}"))?;
    value
        .get("input")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or(
            "OpenAI Responses custom tool bridge arguments must contain a string `input` field."
                .to_string(),
        )
}

pub(crate) fn openai_tool_arguments_raw(tool_call: &Value) -> Option<&str> {
    tool_call
        .get("function")
        .and_then(|function| function.get("arguments"))
        .or_else(|| tool_call.get("arguments"))
        .and_then(Value::as_str)
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

pub(crate) fn normalized_responses_tool_definition(
    tool: &Value,
) -> Result<Option<NormalizedOpenAiFamilyToolDef>, String> {
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

pub(crate) fn normalized_openai_tool_definitions_from_request(
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

pub(crate) fn normalized_tool_definition_to_openai_with_custom_bridge(
    tool: &NormalizedOpenAiFamilyToolDef,
) -> Result<Value, String> {
    match tool {
        NormalizedOpenAiFamilyToolDef::Custom(custom) => {
            let mut payload = serde_json::Map::new();
            payload.insert(
                "name".to_string(),
                Value::String(openai_responses_custom_tool_bridge_name(&custom.name)),
            );
            if let Some(description) = custom.description.clone() {
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
        .map(|tool_call| normalized_tool_call_to_openai(&tool_call))
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
        _ => Ok(Some(normalized_tool_call_to_openai(&tool_call))),
    }
}

pub(crate) fn responses_tool_call_item_to_openai_tool_call_with_custom_bridge_strict(
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
                    "name": openai_responses_custom_tool_bridge_name(&name),
                    "arguments": openai_responses_custom_tool_bridge_arguments(&input)?
                }
            });
            if let Some(proxied_tool_kind) = proxied_tool_kind {
                tool_call["proxied_tool_kind"] = proxied_tool_kind;
            }
            Ok(Some(tool_call))
        }
        other => Ok(Some(normalized_tool_call_to_openai(&other))),
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
    normalized_openai_tool_call(tool_call)
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
        })
}

pub(crate) fn openai_tool_call_to_responses_item_decoding_custom_bridge(
    tool_call: &Value,
) -> Result<Value, String> {
    let call = match normalized_openai_tool_call(tool_call) {
        Ok(Some(call)) => call,
        Ok(None) | Err(_) => {
            return Ok(openai_tool_call_to_responses_item(tool_call));
        }
    };

    match call {
        NormalizedOpenAiFamilyToolCall::Function {
            id,
            name,
            arguments,
            namespace,
            proxied_tool_kind,
        } => {
            if let Some(custom_name) = openai_responses_custom_tool_name_from_bridge(&name) {
                let mut item = serde_json::json!({
                    "type": "custom_tool_call",
                    "call_id": id,
                    "name": custom_name,
                    "input": openai_responses_custom_tool_input_from_bridge_arguments(&arguments)?
                });
                if let Some(proxied_tool_kind) = proxied_tool_kind {
                    item["proxied_tool_kind"] = proxied_tool_kind;
                }
                Ok(item)
            } else {
                Ok(normalized_tool_call_to_responses_item(
                    NormalizedOpenAiFamilyToolCall::Function {
                        id,
                        name,
                        arguments,
                        namespace,
                        proxied_tool_kind,
                    },
                ))
            }
        }
        other => Ok(normalized_tool_call_to_responses_item(other)),
    }
}
