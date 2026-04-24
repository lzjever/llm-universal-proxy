use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    body::Body,
    http::{Response, StatusCode},
    response::IntoResponse,
    Json,
};
use serde_json::Value;

use crate::formats::UpstreamFormat;
use crate::internal_artifacts::{
    contains_internal_artifact_text, sanitize_public_error_message, GENERIC_UPSTREAM_ERROR_MESSAGE,
};

pub(super) fn client_closed_response(format: UpstreamFormat) -> Response<Body> {
    error_response(
        format,
        StatusCode::from_u16(499).expect("499 should be a valid HTTP status"),
        "downstream client disconnected",
    )
}

pub(super) fn format_upstream_unavailable_message(
    name: &str,
    availability: &crate::discovery::UpstreamAvailability,
) -> String {
    match availability {
        crate::discovery::UpstreamAvailability::Available => {
            format!("resolved upstream `{name}` is unavailable")
        }
        crate::discovery::UpstreamAvailability::Unavailable { reason } => {
            format!("resolved upstream `{name}` is unavailable: {reason}")
        }
    }
}

pub(super) fn error_response(
    format: UpstreamFormat,
    status: StatusCode,
    message: &str,
) -> Response<Body> {
    let normalized_error = normalize_upstream_error(status, message);
    match format {
        UpstreamFormat::OpenAiCompletion | UpstreamFormat::OpenAiResponses => {
            (status, Json(openai_error_body(&normalized_error))).into_response()
        }
        UpstreamFormat::Anthropic => (
            status,
            Json(anthropic_error_body(status, &normalized_error)),
        )
            .into_response(),
        UpstreamFormat::Google => (
            status,
            Json(serde_json::json!({
                "error": {
                    "code": status.as_u16(),
                    "message": normalized_error.message,
                    "status": google_status_text(status),
                }
            })),
        )
            .into_response(),
    }
}

pub(super) fn streaming_error_response(
    format: UpstreamFormat,
    status: StatusCode,
    message: &str,
) -> Response<Body> {
    let public_message = if contains_internal_artifact_text(message) {
        GENERIC_UPSTREAM_ERROR_MESSAGE.to_string()
    } else {
        message.to_string()
    };
    if format != UpstreamFormat::OpenAiResponses {
        return error_response(format, status, &public_message);
    }

    let normalized_error = normalize_upstream_error(status, &public_message);
    let response_id = format!("resp_error_{}", uuid::Uuid::new_v4().simple());
    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let payload = serde_json::json!({
        "type": "response.failed",
        "sequence_number": 0,
        "response": {
            "id": response_id,
            "object": "response",
            "created_at": created_at,
            "status": "failed",
            "background": false,
            "error": {
                "type": normalized_error.error_type,
                "code": normalized_error.code,
                "message": normalized_error.message,
            },
            "incomplete_details": null,
            "usage": null,
            "metadata": {}
        }
    });
    let body = format!("event: response.failed\ndata: {payload}\n\n");

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/event-stream")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .body(Body::from(body))
        .unwrap()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NormalizedUpstreamError {
    pub(super) message: String,
    pub(super) error_type: &'static str,
    pub(super) code: Option<&'static str>,
}

pub(super) fn normalize_upstream_error(
    status: StatusCode,
    raw_message: &str,
) -> NormalizedUpstreamError {
    let parsed = serde_json::from_str::<Value>(raw_message).ok();
    let extracted_message = parsed
        .as_ref()
        .and_then(extract_error_message)
        .filter(|message| !message.is_empty())
        .map(|message| sanitize_public_error_message(&message))
        .unwrap_or_else(|| sanitize_public_error_message(raw_message));
    let signal = parsed
        .as_ref()
        .map(extract_error_signal)
        .unwrap_or_default();
    let signal = signal.to_ascii_lowercase();
    let message_for_classification = parsed
        .as_ref()
        .and_then(extract_error_message)
        .unwrap_or_else(|| raw_message.to_string());
    let message_lc = message_for_classification.to_ascii_lowercase();
    let combined = format!("{signal} {message_lc}");

    let (error_type, code) = if status == StatusCode::TOO_MANY_REQUESTS
        || combined.contains("rate limit")
        || combined.contains("rate_limit")
    {
        ("rate_limit_error", Some("rate_limit_exceeded"))
    } else if combined.contains("quota")
        || combined.contains("insufficient_quota")
        || combined.contains("credit balance")
    {
        ("insufficient_quota", Some("insufficient_quota"))
    } else if status.is_server_error()
        || combined.contains("overloaded")
        || combined.contains("slow down")
        || combined.contains("server_is_overloaded")
        || combined.contains("temporarily unavailable")
        || combined.contains("service unavailable")
    {
        ("server_error", Some("server_is_overloaded"))
    } else if combined.contains("context_length_exceeded")
        || combined.contains("context window")
        || combined.contains("maximum context length")
        || combined.contains("prompt is too long")
        || combined.contains("prompt too long")
        || combined.contains("too many tokens")
        || combined.contains("token limit exceeded")
    {
        ("invalid_request_error", Some("context_length_exceeded"))
    } else if combined.contains("invalid_prompt")
        || combined.contains("safety reasons")
        || combined.contains("prompt blocked")
    {
        ("invalid_request_error", Some("invalid_prompt"))
    } else if status.is_client_error() {
        ("invalid_request_error", Some("invalid_request_error"))
    } else {
        ("server_error", None)
    };

    NormalizedUpstreamError {
        message: extracted_message,
        error_type,
        code,
    }
}

fn extract_error_message(body: &Value) -> Option<String> {
    body.get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .or_else(|| {
            body.get("error")
                .and_then(|error| error.get("error"))
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
        })
        .or_else(|| body.get("message").and_then(Value::as_str))
        .map(ToString::to_string)
}

fn extract_error_signal(body: &Value) -> String {
    let candidates = [
        body.get("error")
            .and_then(|error| error.get("code"))
            .and_then(Value::as_str),
        body.get("error")
            .and_then(|error| error.get("type"))
            .and_then(Value::as_str),
        body.get("error")
            .and_then(|error| error.get("status"))
            .and_then(Value::as_str),
        body.get("code").and_then(Value::as_str),
        body.get("type").and_then(Value::as_str),
        body.get("status").and_then(Value::as_str),
    ];

    candidates
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ")
}

fn openai_error_body(error: &NormalizedUpstreamError) -> Value {
    serde_json::json!({
        "error": {
            "message": error.message,
            "type": error.error_type,
            "code": error.code,
        }
    })
}

fn anthropic_error_body(status: StatusCode, error: &NormalizedUpstreamError) -> Value {
    let error_type = match status.as_u16() {
        401 => "authentication_error",
        403 => "permission_error",
        404 => "not_found_error",
        413 => "request_too_large",
        429 => "rate_limit_error",
        529 => "overloaded_error",
        400..=499 => "invalid_request_error",
        500..=599 => "api_error",
        _ => "api_error",
    };

    serde_json::json!({
        "type": "error",
        "error": {
            "type": error_type,
            "message": error.message,
        }
    })
}

pub(super) fn normalized_non_stream_upstream_error(
    upstream_format: UpstreamFormat,
    client_format: UpstreamFormat,
    _upstream_body: &Value,
) -> Option<(StatusCode, String)> {
    if !matches!(
        client_format,
        UpstreamFormat::OpenAiCompletion | UpstreamFormat::OpenAiResponses
    ) {
        return None;
    }

    match upstream_format {
        UpstreamFormat::Anthropic => None,
        _ => None,
    }
}

pub(super) fn classify_post_translation_non_stream_status(
    client_format: UpstreamFormat,
    body: &Value,
) -> StatusCode {
    match client_format {
        UpstreamFormat::Anthropic => match body.get("type").and_then(Value::as_str) {
            Some("error") => match body
                .get("error")
                .and_then(|error| error.get("type"))
                .and_then(Value::as_str)
            {
                Some("invalid_request_error") => StatusCode::BAD_REQUEST,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            },
            _ => StatusCode::OK,
        },
        _ => StatusCode::OK,
    }
}

pub(super) fn append_compatibility_warning_headers(
    response: &mut Response<Body>,
    warnings: &[String],
) {
    for warning in warnings {
        let sanitized = warning.replace(['\r', '\n'], " ");
        if let Ok(value) = axum::http::HeaderValue::from_str(&sanitized) {
            response
                .headers_mut()
                .append("x-proxy-compat-warning", value);
        }
    }
}

fn google_status_text(status: StatusCode) -> &'static str {
    match status {
        StatusCode::BAD_REQUEST => "INVALID_ARGUMENT",
        StatusCode::UNAUTHORIZED => "UNAUTHENTICATED",
        StatusCode::FORBIDDEN => "PERMISSION_DENIED",
        StatusCode::NOT_FOUND => "NOT_FOUND",
        StatusCode::TOO_MANY_REQUESTS => "RESOURCE_EXHAUSTED",
        StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT => {
            "UNAVAILABLE"
        }
        _ => "INTERNAL",
    }
}
