use serde_json::Value;

use crate::translate::{
    validate_responses_public_request_object_tool_identity,
    validate_responses_public_response_object_tool_identity,
    validate_responses_public_tool_call_item_identity,
};

pub(super) const REQUEST_SCOPED_TOOL_BRIDGE_CONTEXT_FIELD: &str = "_llmup_tool_bridge_context";

pub(super) fn reject_internal_request_scoped_tool_bridge_context(body: &Value) -> Option<String> {
    contains_internal_request_scoped_tool_bridge_context(body).then(|| {
        format!(
            "request must not include internal-only field `{REQUEST_SCOPED_TOOL_BRIDGE_CONTEXT_FIELD}`"
        )
    })
}

pub(super) fn validate_openai_responses_resource_request_body(body: &Value) -> Result<(), String> {
    if let Some(message) = reject_internal_request_scoped_tool_bridge_context(body) {
        return Err(message);
    }
    validate_responses_public_request_object_tool_identity(body)
}

pub(super) fn validate_openai_responses_resource_response_body(body: &Value) -> Result<(), String> {
    if contains_internal_request_scoped_tool_bridge_context(body) {
        return Err(format!(
            "upstream response contained internal-only field `{REQUEST_SCOPED_TOOL_BRIDGE_CONTEXT_FIELD}`"
        ));
    }

    validate_responses_public_response_object_tool_identity(body).map_err(|err| {
        format!("upstream response contained reserved public tool identity: {err}")
    })?;
    if let Some(response) = body.get("response") {
        validate_responses_public_response_object_tool_identity(response).map_err(|err| {
            format!("upstream response contained reserved public tool identity: {err}")
        })?;
    }
    if let Some(item) = body.get("item") {
        validate_responses_public_tool_call_item_identity(item).map_err(|err| {
            format!("upstream response contained reserved public tool identity: {err}")
        })?;
    }
    Ok(())
}

fn contains_internal_request_scoped_tool_bridge_context(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            key == REQUEST_SCOPED_TOOL_BRIDGE_CONTEXT_FIELD
                || contains_internal_request_scoped_tool_bridge_context(value)
        }),
        Value::Array(items) => items
            .iter()
            .any(contains_internal_request_scoped_tool_bridge_context),
        _ => false,
    }
}
