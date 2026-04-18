use super::*;

#[test]
fn classify_request_boundary_rejects_translated_stateful_responses_controls() {
    let decision = classify_request_boundary(
        crate::formats::UpstreamFormat::OpenAiResponses,
        crate::formats::UpstreamFormat::Anthropic,
        &serde_json::json!({
            "conversation": { "id": "conv_1" },
            "background": true
        }),
    );

    let RequestBoundaryDecision::Reject(message) = decision else {
        panic!("expected rejection, got {decision:?}");
    };
    assert!(message.contains("conversation"));
    assert!(message.contains("background"));
    assert!(message.contains("native OpenAI Responses"));
}

#[test]
fn classify_request_boundary_keeps_warning_path_for_allowed_degradation() {
    let decision = classify_request_boundary(
        crate::formats::UpstreamFormat::OpenAiResponses,
        crate::formats::UpstreamFormat::Anthropic,
        &serde_json::json!({
            "store": true,
            "tools": [{ "type": "web_search" }]
        }),
    );

    let RequestBoundaryDecision::AllowWithWarnings(warnings) = decision else {
        panic!("expected warning path, got {decision:?}");
    };
    assert!(warnings.iter().any(|warning| warning.contains("store")));
    assert!(warnings
        .iter()
        .any(|warning| warning.contains("non-function Responses tools")));
}

#[test]
fn classify_request_boundary_warns_for_gemini_top_k_drop_policy() {
    let decision = classify_request_boundary(
        crate::formats::UpstreamFormat::Google,
        crate::formats::UpstreamFormat::OpenAiCompletion,
        &serde_json::json!({
            "contents": [{
                "role": "user",
                "parts": [{ "text": "Hi" }]
            }],
            "generationConfig": {
                "topK": 40
            }
        }),
    );

    let RequestBoundaryDecision::AllowWithWarnings(warnings) = decision else {
        panic!("expected warning path, got {decision:?}");
    };
    assert!(warnings.iter().any(|warning| warning.contains("topK")));
}

#[tokio::test]
async fn live_responses_store_drop_surfaces_warning_header() {
    let response_body = serde_json::json!({
        "id": "chatcmpl_1",
        "object": "chat.completion",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "Hi" },
            "finish_reason": "stop"
        }]
    });
    let (mock_base, requests, server) = spawn_openai_completion_mock(response_body).await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiCompletion);

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/responses".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Hi" }]
            }],
            "store": true,
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let warnings = response
        .headers()
        .get_all("x-proxy-compat-warning")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>();
    assert!(
        warnings.iter().any(|warning| warning.contains("store")),
        "warnings = {warnings:?}"
    );

    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    assert!(
        recorded[0].get("store").is_none(),
        "translated request should drop store: {:?}",
        recorded[0]
    );

    server.abort();
}

#[tokio::test]
async fn live_gemini_top_k_drop_surfaces_warning_header() {
    let response_body = serde_json::json!({
        "id": "chatcmpl_1",
        "object": "chat.completion",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "Hi" },
            "finish_reason": "stop"
        }]
    });
    let (mock_base, requests, server) = spawn_openai_completion_mock(response_body).await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiCompletion);

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/google/v1beta/models/gpt-4o-mini:generateContent".to_string(),
        serde_json::json!({
            "contents": [{
                "role": "user",
                "parts": [{ "text": "Hi" }]
            }],
            "generationConfig": {
                "topK": 40
            }
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::Google,
        Some(false),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let warnings = response
        .headers()
        .get_all("x-proxy-compat-warning")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>();
    assert!(
        warnings.iter().any(|warning| warning.contains("topK")),
        "warnings = {warnings:?}"
    );

    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    assert!(
        recorded[0]
            .get("top_k")
            .or_else(|| recorded[0].get("topK"))
            .is_none(),
        "translated request should drop topK: {:?}",
        recorded[0]
    );
    assert!(
        recorded[0]
            .get("generationConfig")
            .and_then(|config| config.get("topK"))
            .is_none(),
        "translated request should drop nested topK: {:?}",
        recorded[0]
    );

    server.abort();
}

#[test]
fn resolve_requested_model_or_error_requires_model_for_multi_upstream_namespace() {
    let config = crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(30),
        upstreams: vec![
            crate::config::UpstreamConfig {
                name: "a".to_string(),
                api_root: "https://example.com/v1".to_string(),
                fixed_upstream_format: Some(crate::formats::UpstreamFormat::OpenAiResponses),
                fallback_credential_env: None,
                fallback_credential_actual: None,
                fallback_api_key: None,
                auth_policy: crate::config::AuthPolicy::ClientOrFallback,
                upstream_headers: Vec::new(),
            },
            crate::config::UpstreamConfig {
                name: "b".to_string(),
                api_root: "https://example.org/v1".to_string(),
                fixed_upstream_format: Some(crate::formats::UpstreamFormat::OpenAiResponses),
                fallback_credential_env: None,
                fallback_credential_actual: None,
                fallback_api_key: None,
                auth_policy: crate::config::AuthPolicy::ClientOrFallback,
                upstream_headers: Vec::new(),
            },
        ],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: crate::config::DebugTraceConfig::default(),
    };

    let error = resolve_requested_model_or_error(
        &config,
        "",
        crate::formats::UpstreamFormat::OpenAiResponses,
        &serde_json::json!({}),
    )
    .expect_err("missing model should fail");

    assert!(error.contains("request must include a routable `model`"));
}

#[test]
fn resolve_requested_model_or_error_explains_previous_response_boundary() {
    let config = crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(30),
        upstreams: vec![
            crate::config::UpstreamConfig {
                name: "a".to_string(),
                api_root: "https://example.com/v1".to_string(),
                fixed_upstream_format: Some(crate::formats::UpstreamFormat::OpenAiResponses),
                fallback_credential_env: None,
                fallback_credential_actual: None,
                fallback_api_key: None,
                auth_policy: crate::config::AuthPolicy::ClientOrFallback,
                upstream_headers: Vec::new(),
            },
            crate::config::UpstreamConfig {
                name: "b".to_string(),
                api_root: "https://example.org/v1".to_string(),
                fixed_upstream_format: Some(crate::formats::UpstreamFormat::OpenAiResponses),
                fallback_credential_env: None,
                fallback_credential_actual: None,
                fallback_api_key: None,
                auth_policy: crate::config::AuthPolicy::ClientOrFallback,
                upstream_headers: Vec::new(),
            },
        ],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: crate::config::DebugTraceConfig::default(),
    };

    let error = resolve_requested_model_or_error(
        &config,
        "",
        crate::formats::UpstreamFormat::OpenAiResponses,
        &serde_json::json!({ "previous_response_id": "resp_1" }),
    )
    .expect_err("missing model should fail");

    assert!(error.contains("previous_response_id"));
    assert!(error.contains("does not reconstruct response-to-upstream state"));
}
