use serde_json::Value;

use crate::formats::UpstreamFormat;

use super::assessment::{gemini_normalized_logprobs_controls, normalized_openai_audio_contract};
use super::media::{
    base64_data_uri_parts, http_or_https_remote_url, openai_file_data_reference_from_part,
    OpenAiFileDataReference,
};
use super::messages::{
    custom_tools_not_portable_message, gemini_function_response_parts_not_portable_message,
};
use super::models::{
    NormalizedDecodingControls, NormalizedJsonSchemaOutputShape, NormalizedOutputShape,
    NormalizedRequestControls, NormalizedToolPolicy, SemanticToolKind,
};
use super::openai_family::{
    normalized_output_shape_to_openai_response_format, openai_declared_function_tools,
    openai_normalized_request_controls, openai_select_function_tools_by_name,
};
use super::response_protocols::push_gemini_function_call_part;
use super::tools::{
    insert_request_scoped_tool_bridge_context, request_scoped_tool_bridge_context_from_body,
    semantic_tool_kind_from_value, validate_public_tool_name_not_reserved,
};

fn gemini_system_instruction_non_text_part_message(part: &Value, label: &str) -> String {
    let mime = gemini_part_field(part, "inlineData", "inline_data")
        .or_else(|| gemini_part_field(part, "fileData", "file_data"))
        .and_then(|data| {
            data.get("mimeType")
                .or_else(|| data.get("mime_type"))
                .and_then(Value::as_str)
        })
        .map(|mime| format!(" with MIME `{mime}`"))
        .unwrap_or_default();
    format!(
        "Gemini systemInstruction part `{label}`{mime} cannot be faithfully translated to non-Gemini targets; systemInstruction.parts only support text parts."
    )
}

fn extract_gemini_system_instruction_text(content: &Value) -> Result<String, String> {
    if let Some(s) = content.as_str() {
        return Ok(s.to_string());
    }
    let Some(parts) = content.get("parts").and_then(Value::as_array) else {
        return Ok(String::new());
    };
    let mut text_parts = Vec::new();
    for part in parts {
        for (camel, snake, label) in [
            ("inlineData", "inline_data", "inlineData"),
            ("fileData", "file_data", "fileData"),
            ("functionCall", "function_call", "functionCall"),
            ("functionResponse", "function_response", "functionResponse"),
        ] {
            if part.get(camel).is_some() || part.get(snake).is_some() {
                return Err(gemini_system_instruction_non_text_part_message(part, label));
            }
        }
        if part.get("thought").is_some() {
            return Err(gemini_system_instruction_non_text_part_message(
                part, "thought",
            ));
        }
        if let Some(text) = part.get("text") {
            let Some(text) = text.as_str() else {
                return Err(
                    "Gemini systemInstruction text parts require string `text` values to translate to non-Gemini targets."
                        .to_string(),
                );
            };
            text_parts.push(text);
            continue;
        }
        return Err(gemini_system_instruction_non_text_part_message(
            part,
            &gemini_part_kind_label(part),
        ));
    }
    Ok(text_parts.join(""))
}

pub(super) fn gemini_function_response_has_nonportable_parts(function_response: &Value) -> bool {
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

fn collapse_gemini_parts_for_openai(parts: &[Value]) -> Value {
    if parts.len() == 1 && parts[0].get("type").and_then(Value::as_str) == Some("text") {
        return parts[0]
            .get("text")
            .cloned()
            .unwrap_or(Value::String(String::new()));
    }
    Value::Array(parts.to_vec())
}

pub(super) fn gemini_request_field<'a>(
    value: &'a Value,
    camel: &str,
    snake: &str,
) -> Option<&'a Value> {
    gemini_nested_field(value, camel, snake)
}

pub(super) fn gemini_request_object_field<'a>(
    value: &'a Value,
    camel: &str,
    snake: &str,
) -> Option<&'a serde_json::Map<String, Value>> {
    gemini_request_field(value, camel, snake).and_then(Value::as_object)
}

pub(super) fn gemini_request_array_field<'a>(
    value: &'a Value,
    camel: &str,
    snake: &str,
) -> Option<&'a Vec<Value>> {
    gemini_request_field(value, camel, snake).and_then(Value::as_array)
}

pub(super) fn gemini_request_object_field_from_object<'a>(
    value: &'a serde_json::Map<String, Value>,
    camel: &str,
    snake: &str,
) -> Option<&'a serde_json::Map<String, Value>> {
    value
        .get(camel)
        .or_else(|| value.get(snake))
        .and_then(Value::as_object)
}

pub(super) fn gemini_request_system_instruction(body: &Value) -> Option<&Value> {
    gemini_request_field(body, "systemInstruction", "system_instruction")
}

pub(super) fn gemini_request_generation_config(body: &Value) -> Option<&Value> {
    gemini_request_field(body, "generationConfig", "generation_config")
}

pub(super) fn gemini_request_tools(body: &Value) -> Option<&Vec<Value>> {
    gemini_request_array_field(body, "tools", "tools")
}

pub(super) fn gemini_request_tool_config(body: &Value) -> Option<&serde_json::Map<String, Value>> {
    gemini_request_object_field(body, "toolConfig", "tool_config")
}

pub(super) fn gemini_request_function_calling_config_from_object(
    tool_config: &serde_json::Map<String, Value>,
) -> Option<&serde_json::Map<String, Value>> {
    gemini_request_object_field_from_object(
        tool_config,
        "functionCallingConfig",
        "function_calling_config",
    )
}

pub(super) fn gemini_generation_config_field<'a>(
    body: &'a Value,
    camel: &str,
    snake: &str,
) -> Option<&'a Value> {
    gemini_request_generation_config(body)
        .and_then(|generation_config| gemini_request_field(generation_config, camel, snake))
        .filter(|value| !value.is_null())
}

pub(super) fn gemini_function_declaration_output_schema_field(
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

pub(super) fn gemini_function_output_schema_not_portable_message(
    declaration: &Value,
    field_name: &str,
    target_label: &str,
) -> String {
    format!(
        "Gemini FunctionDeclaration `{}` field `{field_name}` cannot be faithfully translated to {target_label}",
        gemini_function_declaration_name(declaration)
    )
}

pub(super) fn gemini_request_nonportable_output_shape_message(
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

pub(super) fn gemini_normalized_output_shape(
    body: &Value,
) -> Result<Option<NormalizedOutputShape>, String> {
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

pub(super) fn gemini_normalized_decoding_controls(body: &Value) -> NormalizedDecodingControls {
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

pub(super) fn normalized_output_shape_to_gemini_generation_config(
    shape: &NormalizedOutputShape,
) -> Value {
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

pub(super) fn normalized_output_shape_to_claude_output_config(
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

pub(super) fn openai_stop_to_gemini_stop_sequences(stop: &Value) -> Value {
    if stop.is_array() {
        stop.clone()
    } else {
        Value::Array(vec![stop.clone()])
    }
}

pub(super) fn openai_portable_function_tools(
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

pub(super) fn gemini_tool_function_declarations(tool: &Value) -> Option<&Vec<Value>> {
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

pub(super) fn gemini_function_declaration_name(declaration: &Value) -> &str {
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

pub(super) fn gemini_openai_function_tool_from_declaration(
    declaration: &Value,
) -> Result<Value, String> {
    if let Some((field_name, _)) = gemini_function_declaration_output_schema_field(declaration) {
        return Err(gemini_function_output_schema_not_portable_message(
            declaration,
            field_name,
            "non-Gemini targets",
        ));
    }

    if let Some(name) = declaration.get("name").and_then(Value::as_str) {
        validate_public_tool_name_not_reserved(name)?;
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

pub(super) fn gemini_openai_function_tools_from_request(
    body: &Value,
) -> Result<Vec<Value>, String> {
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

pub(super) fn gemini_openai_function_tool_name(tool: &Value) -> Option<&str> {
    tool.get("function")
        .and_then(|function| function.get("name"))
        .and_then(Value::as_str)
}

pub(super) fn gemini_select_openai_tools_by_name(
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

pub(super) fn gemini_validated_allowed_function_names(
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
        validate_public_tool_name_not_reserved(name)?;
        validated_names.push(name.to_string());
    }

    gemini_select_openai_tools_by_name(openai_tools, &validated_names)?;
    Ok(Some(validated_names))
}

pub(super) fn gemini_normalized_request_controls(
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

pub(super) fn normalized_tool_policy_to_openai_tool_choice(
    tool_policy: &NormalizedToolPolicy,
) -> Value {
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

pub(super) fn gemini_to_openai(
    body: &mut Value,
    target_format: UpstreamFormat,
) -> Result<(), String> {
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
        let text = extract_gemini_system_instruction_text(si)?;
        if !text.is_empty() {
            result["messages"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({ "role": "system", "content": text }));
        }
    }
    if let Some(contents) = body.get("contents").and_then(Value::as_array) {
        for content in contents {
            for msg in convert_gemini_content_to_openai_for_target(content, target_format)? {
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

pub(super) fn gemini_part_field<'a>(
    part: &'a Value,
    camel: &str,
    snake: &str,
) -> Option<&'a Value> {
    part.get(camel).or_else(|| part.get(snake))
}

pub(super) fn gemini_nested_field<'a>(
    value: &'a Value,
    camel: &str,
    snake: &str,
) -> Option<&'a Value> {
    value.get(camel).or_else(|| value.get(snake))
}

pub(super) fn gemini_part_kind_label(part: &Value) -> String {
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

pub(super) fn normalize_audio_format_from_mime(mime: &str) -> String {
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

pub(super) fn openai_audio_mime_type(format: &str) -> String {
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

fn gemini_video_not_portable_message(part_kind: &str, mime: &str) -> String {
    format!(
        "Gemini {part_kind} MIME `{mime}` is video input and cannot be faithfully translated to non-Gemini targets; video/* must not be coerced into generic file parts."
    )
}

fn gemini_mime_type_is_video(mime: &str) -> bool {
    mime.split(';')
        .next()
        .unwrap_or(mime)
        .trim()
        .to_ascii_lowercase()
        .starts_with("video/")
}

pub(super) fn openai_file_part_from_gemini_inline_data(mime: &str, data: &str) -> Value {
    serde_json::json!({
        "type": "file",
        "file": {
            "file_data": format!("data:{mime};base64,{data}"),
            "mime_type": mime
        }
    })
}

fn gemini_file_data_file_uri(file_data: &Value) -> Option<&str> {
    file_data
        .get("fileUri")
        .or_else(|| file_data.get("file_uri"))
        .and_then(Value::as_str)
        .filter(|file_uri| !file_uri.trim().is_empty())
}

fn gemini_file_data_openai_target_label(target_format: UpstreamFormat) -> &'static str {
    match target_format {
        UpstreamFormat::OpenAiCompletion => "OpenAI Chat Completions",
        UpstreamFormat::OpenAiResponses => "OpenAI Responses",
        UpstreamFormat::Anthropic => "Anthropic",
        UpstreamFormat::Google => "Gemini",
    }
}

pub(super) fn gemini_file_data_to_openai_part(
    file_data: &Value,
    target_format: UpstreamFormat,
) -> Result<Value, String> {
    let target_label = gemini_file_data_openai_target_label(target_format);
    let Some(file_uri) = gemini_file_data_file_uri(file_data) else {
        return Err(format!(
            "Gemini fileData.fileUri requires a non-empty string to translate to {target_label}."
        ));
    };
    let Some(file_url) = http_or_https_remote_url(file_uri) else {
        return Err(format!(
            "Gemini fileData.fileUri `{file_uri}` cannot be faithfully translated to {target_label}; provider-native or local URIs need an explicit fetch/upload adapter."
        ));
    };
    if target_format != UpstreamFormat::Anthropic {
        return Err(format!(
            "Gemini fileData.fileUri `{file_uri}` cannot be faithfully translated to {target_label}; this translator only preserves inlineData for OpenAI targets until an explicit fetch/upload adapter exists."
        ));
    }

    let mut file = serde_json::Map::new();
    file.insert("file_url".to_string(), Value::String(file_url.to_string()));
    if let Some(mime_type) = file_data
        .get("mimeType")
        .or_else(|| file_data.get("mime_type"))
        .cloned()
    {
        file.insert("mime_type".to_string(), mime_type);
    }
    if let Some(filename) = file_data
        .get("displayName")
        .or_else(|| file_data.get("display_name"))
        .cloned()
    {
        file.insert("filename".to_string(), filename);
    }
    Ok(serde_json::json!({
        "type": "file",
        "file": Value::Object(file)
    }))
}

pub(super) fn convert_gemini_content_to_openai(content: &Value) -> Result<Vec<Value>, String> {
    convert_gemini_content_to_openai_for_target(content, UpstreamFormat::OpenAiCompletion)
}

pub(super) fn convert_gemini_content_to_openai_for_target(
    content: &Value,
    target_format: UpstreamFormat,
) -> Result<Vec<Value>, String> {
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
            if gemini_mime_type_is_video(mime) {
                return Err(gemini_video_not_portable_message("inlineData", mime));
            }
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
            let mime = file_data
                .get("mimeType")
                .or_else(|| file_data.get("mime_type"))
                .and_then(Value::as_str)
                .unwrap_or("application/octet-stream");
            if gemini_mime_type_is_video(mime) {
                return Err(gemini_video_not_portable_message("fileData", mime));
            }
            openai_parts.push(gemini_file_data_to_openai_part(file_data, target_format)?);
        }
        if let Some(fc) = gemini_part_field(part, "functionCall", "function_call") {
            recognized = true;
            if let Some(name) = fc.get("name").and_then(Value::as_str) {
                validate_public_tool_name_not_reserved(name)?;
            }
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

pub(super) fn uuid_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{t:x}")
}

pub(super) fn normalized_tool_policy_to_gemini_function_calling_config(
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

pub(super) fn flush_pending_gemini_function_responses(
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

pub(super) fn openai_content_part_to_gemini_part(part: &Value) -> Result<Option<Value>, String> {
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
            match openai_file_data_reference_from_part(part)? {
                OpenAiFileDataReference::InlineData { mime_type, data } => {
                    Ok(Some(serde_json::json!({
                        "inlineData": { "mimeType": mime_type, "data": data }
                    })))
                }
                OpenAiFileDataReference::HttpRemoteUrl {
                    mime_type,
                    url: file_uri,
                }
                | OpenAiFileDataReference::ProviderOrLocalUri {
                    mime_type,
                    uri: file_uri,
                } => {
                    let mut file_data_part = serde_json::json!({
                        "fileData": { "mimeType": mime_type, "fileUri": file_uri }
                    });
                    if let Some(filename) = file.get("filename").cloned() {
                        file_data_part["fileData"]["displayName"] = filename;
                    }
                    Ok(Some(file_data_part))
                }
                OpenAiFileDataReference::BareBase64 { mime_type, data } => {
                    let Some(mime_type) = mime_type else {
                        return Err(
                            "OpenAI file_data payloads need MIME or filename provenance to translate to Gemini; use a MIME-bearing data URI or include a filename with a known extension."
                                .to_string(),
                        );
                    };
                    Ok(Some(serde_json::json!({
                        "inlineData": { "mimeType": mime_type, "data": data }
                    })))
                }
            }
        }
        other => Err(format!(
            "OpenAI content part `{other}` cannot be faithfully translated to Gemini."
        )),
    }
}

pub(super) fn gemini_display_name_extension_for_mime_type(mime_type: &str) -> String {
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

pub(super) fn gemini_tool_result_display_name(
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

pub(super) fn gemini_function_response_parts_supported_for_model(model: &str) -> bool {
    model.trim().to_ascii_lowercase().contains("gemini-3")
}

pub(super) fn gemini_function_response_parts_supported_mime_type(mime_type: &str) -> bool {
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

pub(super) fn ensure_gemini_function_response_part_supported(
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

pub(super) fn gemini_tool_result_inline_part(
    display_name: &str,
    mime_type: &str,
    data: &str,
) -> Value {
    serde_json::json!({
        "inlineData": {
            "displayName": display_name,
            "mimeType": mime_type,
            "data": data
        }
    })
}

pub(super) fn gemini_tool_result_media_response_item(
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

pub(super) fn gemini_tool_result_parse_image_data_uri(
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

pub(super) fn gemini_tool_result_block_kind(block: &Value) -> Option<&str> {
    block.get("type").and_then(Value::as_str)
}

pub(super) fn gemini_tool_result_array_media_status(blocks: &[Value]) -> Result<bool, String> {
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

pub(super) fn gemini_tool_result_block_to_response_and_part(
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
                gemini_tool_result_media_response_item("image", &display_name, &mime_type, None),
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
                gemini_tool_result_media_response_item("audio", &display_name, &mime_type, None),
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
            if file.get("file_data").and_then(Value::as_str).is_none() {
                return Err(
                    "Gemini tool result `file` blocks require inline file_data to translate to Gemini."
                        .to_string(),
                );
            };
            let (mime_type, data) = match openai_file_data_reference_from_part(block)? {
                OpenAiFileDataReference::InlineData { mime_type, data } => {
                    (mime_type, data.to_string())
                }
                OpenAiFileDataReference::HttpRemoteUrl { url: file_uri, .. }
                | OpenAiFileDataReference::ProviderOrLocalUri { uri: file_uri, .. } => {
                    return Err(format!(
                        "Gemini tool result file reference `{file_uri}` cannot be faithfully translated without inline file bytes."
                    ));
                }
                OpenAiFileDataReference::BareBase64 { mime_type, data } => {
                    let Some(mime_type) = mime_type else {
                        return Err(
                            "Gemini tool result `file` blocks require MIME provenance for bare base64 file_data."
                                .to_string(),
                        );
                    };
                    (mime_type, data.to_string())
                }
            };
            ensure_gemini_function_response_part_supported(target_model, &mime_type)?;
            let display_name = gemini_tool_result_display_name(call_id, media_index, &mime_type);
            Ok((
                gemini_tool_result_media_response_item("file", &display_name, &mime_type, filename),
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
                gemini_tool_result_media_response_item("image", &display_name, &mime_type, None),
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

pub(super) fn tool_message_to_gemini_function_response(
    msg: &Value,
    function_name: Value,
    target_model: &str,
) -> Result<Value, String> {
    if semantic_tool_kind_from_value(msg) == SemanticToolKind::OpenAiCustom {
        return Err(custom_tools_not_portable_message(UpstreamFormat::Google));
    }
    if let Some(name) = function_name.as_str() {
        validate_public_tool_name_not_reserved(name)?;
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

pub(super) fn openai_to_gemini(body: &mut Value, target_model: &str) -> Result<(), String> {
    let controls = openai_normalized_request_controls(body)?;
    let bridge_context = request_scoped_tool_bridge_context_from_body(body);
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
            let mut parts = Vec::new();
            let tool_calls = msg.get("tool_calls").and_then(Value::as_array);
            if tool_calls.is_none() {
                if let Some(reasoning) = msg
                    .get("reasoning_content")
                    .and_then(Value::as_str)
                    .filter(|reasoning| !reasoning.is_empty())
                {
                    parts.push(serde_json::json!({ "thought": true, "text": reasoning }));
                }
            }
            parts.extend(openai_content_to_gemini_parts(msg.get("content"))?);
            if let Some(tc) = tool_calls {
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
    if let Some(bridge_context) = bridge_context.as_ref() {
        insert_request_scoped_tool_bridge_context(&mut result, bridge_context);
    }
    *body = result;
    Ok(())
}

pub(super) fn openai_content_to_gemini_parts(
    content: Option<&Value>,
) -> Result<Vec<Value>, String> {
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
