use serde_json::Value;

use super::assessment::{
    openai_normalized_logprobs_controls, request_stream_include_obfuscation,
    responses_normalized_logprobs_controls, responses_reasoning_effort, responses_text_verbosity,
};
use super::models::{
    NormalizedDecodingControls, NormalizedJsonSchemaOutputShape, NormalizedOutputShape,
    NormalizedRequestControls, NormalizedToolPolicy,
};

pub(super) fn openai_response_format_json_schema(
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

pub(super) fn responses_text_format_json_schema(
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

pub(super) fn openai_normalized_output_shape(
    body: &Value,
) -> Result<Option<NormalizedOutputShape>, String> {
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

pub(super) fn responses_normalized_output_shape(
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

pub(super) fn openai_normalized_decoding_controls(body: &Value) -> NormalizedDecodingControls {
    NormalizedDecodingControls {
        stop: body.get("stop").cloned(),
        seed: body.get("seed").cloned(),
        presence_penalty: body.get("presence_penalty").cloned(),
        frequency_penalty: body.get("frequency_penalty").cloned(),
        top_k: None,
    }
}

pub(super) fn openai_function_tool_name(value: &Value) -> Option<&str> {
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

pub(super) fn openai_declared_function_tools(body: &Value) -> Vec<Value> {
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

pub(super) fn openai_select_function_tools_by_name(
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

pub(super) fn openai_tool_choice_allowed_tools_object(
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

pub(super) fn openai_legacy_allowed_tool_names(body: &Value) -> Option<Vec<String>> {
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

pub(super) fn openai_normalized_tool_policy(
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

pub(super) fn openai_normalized_request_controls(
    body: &Value,
) -> Result<NormalizedRequestControls, String> {
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

pub(super) fn responses_normalized_request_controls(
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

pub(super) fn normalized_output_shape_to_openai_response_format(
    shape: &NormalizedOutputShape,
) -> Value {
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

pub(super) fn normalized_output_shape_to_responses_text_format(
    shape: &NormalizedOutputShape,
) -> Value {
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

pub(super) fn openai_response_has_assistant_audio(body: &Value) -> bool {
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

pub(super) fn extract_openai_refusal(message: &Value) -> Option<String> {
    message
        .get("refusal")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|text| !text.is_empty())
}

pub(super) fn extract_openai_content_text(content: Option<&Value>) -> String {
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

pub(super) fn collapse_openai_text_parts(parts: &[Value]) -> Value {
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

pub(super) fn copy_remaining_usage_fields(
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
        target_map
            .entry(key.clone())
            .or_insert_with(|| value.clone());
    }
}

pub(super) fn extract_responses_text_content(content: Option<&Value>) -> String {
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
