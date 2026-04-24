use super::*;

#[test]
fn normalize_upstream_error_maps_context_window_messages() {
    let error = normalize_upstream_error(
        StatusCode::BAD_REQUEST,
        r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 215000 tokens > 200000 limit"}}"#,
    );

    assert_eq!(
        error,
        NormalizedUpstreamError {
            message: "prompt is too long: 215000 tokens > 200000 limit".to_string(),
            error_type: "invalid_request_error",
            code: Some("context_length_exceeded"),
        }
    );
}

#[test]
fn normalize_upstream_error_preserves_rate_limit_signal() {
    let error = normalize_upstream_error(
        StatusCode::TOO_MANY_REQUESTS,
        r#"{"error":{"message":"Please slow down.","type":"rate_limit_error"}}"#,
    );

    assert_eq!(
        error,
        NormalizedUpstreamError {
            message: "Please slow down.".to_string(),
            error_type: "rate_limit_error",
            code: Some("rate_limit_exceeded"),
        }
    );
}

#[test]
fn normalize_upstream_error_sanitizes_raw_fallback_internal_artifacts() {
    let error = normalize_upstream_error(
        StatusCode::BAD_GATEWAY,
        "raw upstream body leaked __llmup_custom__secret and _llmup_tool_bridge_context",
    );

    assert_eq!(error.error_type, "server_error");
    assert!(!error.message.contains("__llmup_custom__"));
    assert!(!error.message.contains("_llmup_tool_bridge_context"));
    assert!(!error.message.contains("secret"));
}

#[test]
fn normalize_upstream_error_sanitizes_json_fallback_internal_artifacts_without_message() {
    let error = normalize_upstream_error(
        StatusCode::BAD_GATEWAY,
        r#"{"error":{"type":"server_error","debug":"__llmup_custom__secret"},"_llmup_tool_bridge_context":{"entries":{}}}"#,
    );

    assert_eq!(error.error_type, "server_error");
    assert_eq!(
        error.message,
        crate::internal_artifacts::GENERIC_UPSTREAM_ERROR_MESSAGE
    );
    assert!(!error.message.contains("__llmup_custom__"));
    assert!(!error.message.contains("_llmup_tool_bridge_context"));
    assert!(!error.message.contains("secret"));
}

#[test]
fn normalize_upstream_error_sanitizes_reserved_identity_rejection_message() {
    let error = normalize_upstream_error(
        StatusCode::BAD_GATEWAY,
        "OpenAI Responses function name `__llmup_custom__apply_patch` uses reserved bridge prefix `__llmup_custom__`",
    );

    assert_eq!(
        error.message,
        crate::internal_artifacts::GENERIC_UPSTREAM_ERROR_MESSAGE
    );
    assert!(!error.message.contains("__llmup_custom__"));
    assert!(!error.message.contains("apply_patch"));
}

#[tokio::test]
async fn error_response_anthropic_raw_429_uses_rate_limit_error_and_normalized_message() {
    let response = error_response(
        crate::formats::UpstreamFormat::Anthropic,
        StatusCode::TOO_MANY_REQUESTS,
        r#"{"error":{"message":"Please slow down.","type":"rate_limit_error"}}"#,
    );

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body bytes");
    let body: serde_json::Value = serde_json::from_slice(&body).expect("json body");

    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "rate_limit_error");
    assert_eq!(body["error"]["message"], "Please slow down.");
}

#[tokio::test]
async fn error_response_anthropic_raw_401_maps_to_authentication_error() {
    let response = error_response(
        crate::formats::UpstreamFormat::Anthropic,
        StatusCode::UNAUTHORIZED,
        r#"{"error":{"message":"Bad API key.","type":"invalid_request_error"}}"#,
    );

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body bytes");
    let body: serde_json::Value = serde_json::from_slice(&body).expect("json body");

    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "authentication_error");
    assert_eq!(body["error"]["message"], "Bad API key.");
}

#[tokio::test]
async fn error_response_anthropic_raw_403_maps_to_permission_error() {
    let response = error_response(
        crate::formats::UpstreamFormat::Anthropic,
        StatusCode::FORBIDDEN,
        r#"{"error":{"message":"Access denied.","type":"invalid_request_error"}}"#,
    );

    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body bytes");
    let body: serde_json::Value = serde_json::from_slice(&body).expect("json body");

    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "permission_error");
    assert_eq!(body["error"]["message"], "Access denied.");
}

#[tokio::test]
async fn error_response_anthropic_raw_404_maps_to_not_found_error() {
    let response = error_response(
        crate::formats::UpstreamFormat::Anthropic,
        StatusCode::NOT_FOUND,
        r#"{"error":{"message":"Model not found.","type":"invalid_request_error"}}"#,
    );

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body bytes");
    let body: serde_json::Value = serde_json::from_slice(&body).expect("json body");

    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "not_found_error");
    assert_eq!(body["error"]["message"], "Model not found.");
}

#[tokio::test]
async fn error_response_anthropic_raw_413_maps_to_request_too_large() {
    let response = error_response(
        crate::formats::UpstreamFormat::Anthropic,
        StatusCode::PAYLOAD_TOO_LARGE,
        r#"{"error":{"message":"Payload too large.","type":"invalid_request_error"}}"#,
    );

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body bytes");
    let body: serde_json::Value = serde_json::from_slice(&body).expect("json body");

    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "request_too_large");
    assert_eq!(body["error"]["message"], "Payload too large.");
}

#[tokio::test]
async fn error_response_anthropic_raw_503_maps_to_api_error() {
    let response = error_response(
        crate::formats::UpstreamFormat::Anthropic,
        StatusCode::SERVICE_UNAVAILABLE,
        r#"{"error":{"message":"Backend overloaded.","type":"server_error"}}"#,
    );

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body bytes");
    let body: serde_json::Value = serde_json::from_slice(&body).expect("json body");

    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "api_error");
    assert_eq!(body["error"]["message"], "Backend overloaded.");
}

#[tokio::test]
async fn error_response_google_sanitizes_internal_artifacts() {
    let response = error_response(
        crate::formats::UpstreamFormat::Google,
        StatusCode::BAD_GATEWAY,
        r#"{"error":{"message":"provider leaked __llmup_custom__secret and _llmup_tool_bridge_context","type":"server_error"}}"#,
    );

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body bytes");
    let body_text = String::from_utf8(body.to_vec()).expect("utf8 body");

    assert!(!body_text.contains("__llmup_custom__"), "{body_text}");
    assert!(
        !body_text.contains("_llmup_tool_bridge_context"),
        "{body_text}"
    );
    assert!(!body_text.contains("secret"), "{body_text}");
}

#[test]
fn streaming_error_response_returns_responses_failed_event() {
    let response = streaming_error_response(
        crate::formats::UpstreamFormat::OpenAiResponses,
        StatusCode::BAD_REQUEST,
        r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long"}}"#,
    );

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("text/event-stream")
    );

    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let body = runtime.block_on(async move {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        String::from_utf8(bytes.to_vec()).expect("utf8 body")
    });

    assert!(body.contains("event: response.failed"));
    assert!(body.contains("\"code\":\"context_length_exceeded\""));
    assert!(body.contains("\"message\":\"prompt is too long\""));
}

#[test]
fn streaming_error_response_sanitizes_internal_artifacts() {
    let response = streaming_error_response(
        crate::formats::UpstreamFormat::OpenAiResponses,
        StatusCode::BAD_GATEWAY,
        "raw stream error with __llmup_custom__secret and _llmup_tool_bridge_context",
    );

    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let body = runtime.block_on(async move {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        String::from_utf8(bytes.to_vec()).expect("utf8 body")
    });

    assert!(body.contains("event: response.failed"));
    assert!(!body.contains("__llmup_custom__"), "{body}");
    assert!(!body.contains("_llmup_tool_bridge_context"), "{body}");
    assert!(!body.contains("secret"), "{body}");
}

#[test]
fn normalized_non_stream_upstream_error_does_not_promote_anthropic_context_window_stop() {
    let upstream_body = serde_json::json!({
        "type": "message",
        "stop_reason": "model_context_window_exceeded"
    });

    let actual = normalized_non_stream_upstream_error(
        crate::formats::UpstreamFormat::Anthropic,
        crate::formats::UpstreamFormat::OpenAiResponses,
        &upstream_body,
    );

    assert_eq!(actual, None);
}

#[test]
fn classify_post_translation_non_stream_status_keeps_anthropic_message_success() {
    let status = classify_post_translation_non_stream_status(
        crate::formats::UpstreamFormat::Anthropic,
        &serde_json::json!({
            "type": "message",
            "content": [{ "type": "text", "text": "Hi" }]
        }),
    );

    assert_eq!(status, StatusCode::OK);
}

#[test]
fn classify_post_translation_non_stream_status_maps_anthropic_tool_error_to_400() {
    let status = classify_post_translation_non_stream_status(
        crate::formats::UpstreamFormat::Anthropic,
        &serde_json::json!({
            "type": "error",
            "error": {
                "type": "invalid_request_error",
                "message": "The provider reported a tool or protocol error."
            }
        }),
    );

    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[test]
fn classify_post_translation_non_stream_status_maps_anthropic_api_error_to_500() {
    let status = classify_post_translation_non_stream_status(
        crate::formats::UpstreamFormat::Anthropic,
        &serde_json::json!({
            "type": "error",
            "error": {
                "type": "api_error",
                "message": "The provider returned an error."
            }
        }),
    );

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
}

#[test]
fn append_compatibility_warning_headers_exposes_each_warning() {
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .body(Body::empty())
        .expect("response");
    let warnings = vec![
        "first warning".to_string(),
        "second warning with\nnewline".to_string(),
    ];

    append_compatibility_warning_headers(&mut response, &warnings);

    let values: Vec<_> = response
        .headers()
        .get_all("x-proxy-compat-warning")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    assert_eq!(values, vec!["first warning", "second warning with newline"]);
}
