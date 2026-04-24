use serde_json::Value;

pub(crate) const REQUEST_SCOPED_TOOL_BRIDGE_CONTEXT_FIELD: &str = "_llmup_tool_bridge_context";
pub(crate) const OPENAI_RESPONSES_CUSTOM_BRIDGE_PREFIX: &str = "__llmup_custom__";

pub(crate) const GENERIC_UPSTREAM_ERROR_MESSAGE: &str = "The upstream provider returned an error.";
pub(crate) const INTERNAL_ARTIFACT_ERROR_MESSAGE: &str =
    "upstream payload contained an internal proxy artifact";

pub(crate) fn contains_internal_artifact_text(value: &str) -> bool {
    value.contains(REQUEST_SCOPED_TOOL_BRIDGE_CONTEXT_FIELD)
        || value.contains(OPENAI_RESPONSES_CUSTOM_BRIDGE_PREFIX)
}

pub(crate) fn sanitize_public_error_message(value: &str) -> String {
    if value.trim().is_empty() || contains_internal_artifact_text(value) {
        GENERIC_UPSTREAM_ERROR_MESSAGE.to_string()
    } else {
        value.to_string()
    }
}

pub(crate) fn contains_internal_context_field(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            key == REQUEST_SCOPED_TOOL_BRIDGE_CONTEXT_FIELD
                || contains_internal_context_field(value)
        }),
        Value::Array(items) => items.iter().any(contains_internal_context_field),
        _ => false,
    }
}
