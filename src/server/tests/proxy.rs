use super::*;

const INTERNAL_TOOL_BRIDGE_CONTEXT_FIELD: &str = "_llmup_tool_bridge_context";

fn parse_sse_events(body: &[u8]) -> Vec<Value> {
    let mut buffer = body.to_vec();
    let mut events = Vec::new();
    while let Some(event) = crate::streaming::take_one_sse_event(&mut buffer) {
        events.push(event);
    }
    events
}

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
async fn live_responses_plain_text_custom_tool_bridge_to_openai_balanced_keeps_visible_tool_names_stable(
) {
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
                "content": [{ "type": "input_text", "text": "Create hello.txt" }]
            }],
            "tools": [{
                "type": "custom",
                "name": "code_exec",
                "description": "Executes code",
                "format": { "type": "text" }
            }],
            "tool_choice": {
                "type": "custom",
                "name": "code_exec"
            },
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);

    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    let upstream_body = &recorded[0];
    let tools = upstream_body["tools"].as_array().expect("upstream tools");
    assert_eq!(tools[0]["function"]["name"], "code_exec");
    assert_eq!(
        upstream_body["tool_choice"]["function"]["name"],
        "code_exec"
    );
    let serialized = upstream_body.to_string();
    assert!(
        !serialized.contains("__llmup_custom__code_exec"),
        "translated live request leaked prefixed tool name: {upstream_body:?}"
    );
    assert!(
        upstream_body.get("_llmup_tool_bridge_context").is_none(),
        "internal bridge context must not be sent upstream: {upstream_body:?}"
    );

    server.abort();
}

#[tokio::test]
async fn live_responses_rejects_external_tool_bridge_context_ingress() {
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
                "content": [{ "type": "input_text", "text": "Create hello.txt" }]
            }],
            "_llmup_tool_bridge_context": {
                "visible_tool_names": {
                    "__llmup_custom__code_exec": "code_exec"
                }
            },
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("json body bytes");
    let body: Value = serde_json::from_slice(&body).expect("json body");
    let message = body["error"]["message"]
        .as_str()
        .expect("error message string");
    assert!(
        message.contains(INTERNAL_TOOL_BRIDGE_CONTEXT_FIELD),
        "message = {message}"
    );
    assert!(message.contains("internal-only"), "message = {message}");

    let recorded = requests.lock().await;
    assert!(recorded.is_empty(), "requests = {recorded:?}");

    server.abort();
}

#[tokio::test]
async fn live_responses_custom_tool_bridge_to_openai_strict_rejects_plain_text_custom_tools() {
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
    {
        let mut runtime = state.runtime.write().await;
        let namespace = runtime
            .namespaces
            .get_mut(DEFAULT_NAMESPACE)
            .expect("default namespace");
        namespace.config.compatibility_mode = crate::config::CompatibilityMode::Strict;
    }

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
                "content": [{ "type": "input_text", "text": "Run this script" }]
            }],
            "tools": [{
                "type": "custom",
                "name": "code_exec",
                "description": "Executes code",
                "format": { "type": "text" }
            }],
            "tool_choice": {
                "type": "custom",
                "name": "code_exec"
            },
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let recorded = requests.lock().await;
    assert!(recorded.is_empty(), "requests = {recorded:?}");

    server.abort();
}

#[tokio::test]
async fn live_responses_grammar_custom_tool_bridge_to_openai_balanced_rejects() {
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
                "content": [{ "type": "input_text", "text": "Create hello.txt" }]
            }],
            "tools": [{
                "type": "custom",
                "name": "apply_patch",
                "description": "Apply a patch",
                "format": {
                    "type": "grammar",
                    "syntax": "lark",
                    "definition": "start: /.+/"
                }
            }],
            "tool_choice": {
                "type": "custom",
                "name": "apply_patch"
            },
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let recorded = requests.lock().await;
    assert!(recorded.is_empty(), "requests = {recorded:?}");

    server.abort();
}

#[tokio::test]
async fn live_responses_grammar_custom_tool_bridge_to_openai_max_compat_allows_with_warning_and_stable_names(
) {
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
    {
        let mut runtime = state.runtime.write().await;
        let namespace = runtime
            .namespaces
            .get_mut(DEFAULT_NAMESPACE)
            .expect("default namespace");
        namespace.config.compatibility_mode = crate::config::CompatibilityMode::MaxCompat;
    }

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
                "content": [{ "type": "input_text", "text": "Create hello.txt" }]
            }],
            "tools": [{
                "type": "custom",
                "name": "apply_patch",
                "description": "Apply a patch",
                "format": {
                    "type": "grammar",
                    "syntax": "lark",
                    "definition": "start: /.+/"
                }
            }],
            "tool_choice": {
                "type": "custom",
                "name": "apply_patch"
            },
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
        warnings.iter().any(|warning| {
            warning.contains("apply_patch") && warning.contains("OpenAI Chat Completions")
        }),
        "warnings = {warnings:?}"
    );

    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    let upstream_body = &recorded[0];
    let tools = upstream_body["tools"].as_array().expect("upstream tools");
    assert_eq!(tools[0]["function"]["name"], "apply_patch");
    let description = tools[0]["function"]["description"]
        .as_str()
        .expect("bridged OpenAI tool description");
    assert!(
        description.contains("OpenAI Chat Completions receives this tool"),
        "description = {description}"
    );
    assert!(
        description.contains("OpenAI Chat Completions will not enforce it structurally"),
        "description = {description}"
    );
    assert!(
        description.contains("syntax: lark"),
        "description = {description}"
    );
    assert_eq!(
        upstream_body["tool_choice"]["function"]["name"],
        "apply_patch"
    );
    let serialized = upstream_body.to_string();
    assert!(
        !serialized.contains("__llmup_custom__"),
        "translated live request leaked prefixed tool name: {upstream_body:?}"
    );
    assert!(
        upstream_body.get("_llmup_tool_bridge_context").is_none(),
        "internal bridge context must not be sent upstream: {upstream_body:?}"
    );

    server.abort();
}

#[tokio::test]
async fn live_responses_custom_tool_bridge_to_anthropic_strict_rejects_plain_text_custom_tools() {
    let response_body = serde_json::json!({
        "id": "msg_1",
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "text", "text": "Hi" }],
        "model": "claude-3-7-sonnet",
        "stop_reason": "end_turn",
        "usage": { "input_tokens": 1, "output_tokens": 1 }
    });
    let (mock_base, requests, server) = spawn_anthropic_messages_mock(response_body).await;
    let state = app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::Anthropic);
    {
        let mut runtime = state.runtime.write().await;
        let namespace = runtime
            .namespaces
            .get_mut(DEFAULT_NAMESPACE)
            .expect("default namespace");
        namespace.config.compatibility_mode = crate::config::CompatibilityMode::Strict;
    }

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/responses".to_string(),
        serde_json::json!({
            "model": "claude-3-7-sonnet",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Run this script" }]
            }],
            "tools": [{
                "type": "custom",
                "name": "code_exec",
                "description": "Executes code",
                "format": { "type": "text" }
            }],
            "tool_choice": {
                "type": "custom",
                "name": "code_exec"
            },
            "stream": false
        }),
        "claude-3-7-sonnet".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let recorded = requests.lock().await;
    assert!(recorded.is_empty(), "requests = {recorded:?}");

    server.abort();
}

#[tokio::test]
async fn live_responses_grammar_custom_tool_bridge_to_anthropic_balanced_rejects() {
    let response_body = serde_json::json!({
        "id": "msg_1",
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "text", "text": "Hi" }],
        "model": "claude-3-7-sonnet",
        "stop_reason": "end_turn",
        "usage": { "input_tokens": 1, "output_tokens": 1 }
    });
    let (mock_base, requests, server) = spawn_anthropic_messages_mock(response_body).await;
    let state = app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::Anthropic);

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/responses".to_string(),
        serde_json::json!({
            "model": "claude-3-7-sonnet",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Create hello.txt" }]
            }],
            "tools": [{
                "type": "custom",
                "name": "apply_patch",
                "description": "Apply a patch",
                "format": {
                    "type": "grammar",
                    "syntax": "lark",
                    "definition": "start: /.+/"
                }
            }],
            "tool_choice": {
                "type": "custom",
                "name": "apply_patch"
            },
            "stream": false
        }),
        "claude-3-7-sonnet".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let recorded = requests.lock().await;
    assert!(recorded.is_empty(), "requests = {recorded:?}");

    server.abort();
}

#[tokio::test]
async fn live_responses_grammar_custom_tool_bridge_to_anthropic_max_compat_allows_with_warning_and_stable_names(
) {
    let response_body = serde_json::json!({
        "id": "msg_1",
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "text", "text": "Hi" }],
        "model": "claude-3-7-sonnet",
        "stop_reason": "end_turn",
        "usage": { "input_tokens": 1, "output_tokens": 1 }
    });
    let (mock_base, requests, server) = spawn_anthropic_messages_mock(response_body).await;
    let state = app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::Anthropic);
    {
        let mut runtime = state.runtime.write().await;
        let namespace = runtime
            .namespaces
            .get_mut(DEFAULT_NAMESPACE)
            .expect("default namespace");
        namespace.config.compatibility_mode = crate::config::CompatibilityMode::MaxCompat;
    }

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/responses".to_string(),
        serde_json::json!({
            "model": "claude-3-7-sonnet",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Create hello.txt" }]
            }],
            "tools": [{
                "type": "custom",
                "name": "apply_patch",
                "description": "Apply a patch",
                "format": {
                    "type": "grammar",
                    "syntax": "lark",
                    "definition": "start: /.+/"
                }
            }],
            "tool_choice": {
                "type": "custom",
                "name": "apply_patch"
            },
            "stream": false
        }),
        "claude-3-7-sonnet".to_string(),
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
        warnings
            .iter()
            .any(|warning| warning.contains("apply_patch") && warning.contains("Anthropic")),
        "warnings = {warnings:?}"
    );

    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    let upstream_body = &recorded[0];
    assert_eq!(upstream_body["tools"][0]["name"], "apply_patch");
    assert_eq!(upstream_body["tool_choice"]["name"], "apply_patch");
    let serialized = upstream_body.to_string();
    assert!(
        !serialized.contains("__llmup_custom__"),
        "translated live request leaked prefixed tool name: {upstream_body:?}"
    );
    assert!(
        upstream_body.get("_llmup_tool_bridge_context").is_none(),
        "internal bridge context must not be sent upstream: {upstream_body:?}"
    );

    server.abort();
}

#[tokio::test]
async fn live_responses_gemini_custom_tool_stream_preserves_sse_and_stable_tool_identity() {
    let patch_input = "*** Begin Patch\n*** Add File: hello.txt\n+hello\n*** End Patch\n";
    let response_bodies = vec![
        serde_json::json!({
            "responseId": "resp_gemini_custom",
            "modelVersion": "gemini-2.5-flash",
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{
                        "functionCall": {
                            "id": "call_apply_patch",
                            "name": "apply_patch",
                            "args": { "input": patch_input }
                        }
                    }]
                },
                "finishReason": "STOP"
            }]
        }),
        serde_json::json!({
            "responseId": "resp_gemini_custom",
            "modelVersion": "gemini-2.5-flash",
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": []
                },
                "finishReason": "STOP"
            }]
        }),
    ];
    let (mock_base, requests, server) =
        spawn_google_stream_generate_content_mock(response_bodies).await;
    let state = app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::Google);
    {
        let mut runtime = state.runtime.write().await;
        let namespace = runtime
            .namespaces
            .get_mut(DEFAULT_NAMESPACE)
            .expect("default namespace");
        namespace.config.compatibility_mode = crate::config::CompatibilityMode::MaxCompat;
    }

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/responses".to_string(),
        serde_json::json!({
            "model": "gemini-2.5-flash",
            "tools": [{
                "type": "custom",
                "name": "apply_patch",
                "description": "Apply a patch",
                "format": {
                    "type": "grammar",
                    "syntax": "lark",
                    "definition": "start: /.+/"
                }
            }],
            "tool_choice": {
                "type": "custom",
                "name": "apply_patch"
            },
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Create hello.txt" }]
            }],
            "stream": true
        }),
        "gemini-2.5-flash".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("stream body bytes");
    let body_text = String::from_utf8(body.to_vec()).expect("stream body utf8");
    let events = parse_sse_events(&body);
    let event_types = events
        .iter()
        .filter_map(|event| event.get("type").and_then(Value::as_str))
        .collect::<Vec<_>>();
    assert!(
        event_types.contains(&"response.output_item.added"),
        "body = {body_text}"
    );
    assert!(
        event_types.contains(&"response.custom_tool_call_input.delta"),
        "body = {body_text}"
    );
    assert!(
        event_types.contains(&"response.custom_tool_call_input.done"),
        "body = {body_text}"
    );
    assert!(
        !event_types.contains(&"response.function_call_arguments.delta"),
        "bridge should not regress into function-call delta events: {body_text}"
    );
    let output_item_done = events
        .iter()
        .find(|event| {
            event.get("type").and_then(Value::as_str) == Some("response.output_item.done")
        })
        .expect("response.output_item.done");
    assert_eq!(output_item_done["item"]["type"], "custom_tool_call");
    assert_eq!(output_item_done["item"]["name"], "apply_patch");
    assert_eq!(output_item_done["item"]["input"], patch_input);
    assert!(
        !body_text.contains("__llmup_custom__"),
        "Gemini bridge must not leak prefixed tool names to Responses stream: {body_text}"
    );

    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    assert!(
        recorded[0].0.ends_with(":streamGenerateContent"),
        "streaming request must use Gemini stream path: {:?}",
        recorded[0].0
    );
    assert!(
        !recorded[0].0.contains(":generateContent"),
        "streaming request must not be downgraded to unary Gemini path: {:?}",
        recorded[0].0
    );
    assert!(
        recorded[0]
            .1
            .get(INTERNAL_TOOL_BRIDGE_CONTEXT_FIELD)
            .is_none(),
        "internal bridge context must not be sent upstream: {:?}",
        recorded[0].1
    );
    let serialized = recorded[0].1.to_string();
    assert!(
        !serialized.contains("__llmup_custom__"),
        "Gemini bridge must not leak prefixed tool names upstream: {:?}",
        recorded[0].1
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

#[tokio::test]
async fn live_openai_request_uses_configured_default_output_limit_for_anthropic_upstream() {
    let response_body = serde_json::json!({
        "id": "msg_1",
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "text", "text": "Hi" }],
        "model": "claude-3-7-sonnet",
        "stop_reason": "end_turn",
        "usage": { "input_tokens": 1, "output_tokens": 1 }
    });
    let (mock_base, requests, server) = spawn_anthropic_messages_mock(response_body).await;
    let state = app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::Anthropic);
    {
        let mut runtime = state.runtime.write().await;
        let namespace = runtime
            .namespaces
            .get_mut(DEFAULT_NAMESPACE)
            .expect("default namespace");
        namespace.config.model_aliases.insert(
            "minimax-openai".to_string(),
            crate::config::ModelAlias {
                upstream_name: "primary".to_string(),
                upstream_model: "claude-3-7-sonnet".to_string(),
                limits: Some(crate::config::ModelLimits {
                    context_window: None,
                    max_output_tokens: Some(128_000),
                }),
                surface: None,
            },
        );
    }

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/chat/completions".to_string(),
        serde_json::json!({
            "model": "minimax-openai",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }),
        "minimax-openai".to_string(),
        crate::formats::UpstreamFormat::OpenAiCompletion,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    assert_eq!(recorded[0]["model"], "claude-3-7-sonnet");
    assert_eq!(
        recorded[0]["max_tokens"],
        128_000,
        "configured default output limit should propagate to real Anthropic upstream body when the client omits it: {:?}",
        recorded[0]
    );

    server.abort();
}

#[test]
fn resolve_requested_model_or_error_requires_model_for_multi_upstream_namespace() {
    let config = crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(30),
        compatibility_mode: crate::config::CompatibilityMode::Balanced,
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
                limits: None,
                surface_defaults: None,
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
                limits: None,
                surface_defaults: None,
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
        compatibility_mode: crate::config::CompatibilityMode::Balanced,
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
                limits: None,
                surface_defaults: None,
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
                limits: None,
                surface_defaults: None,
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

#[tokio::test]
async fn openai_responses_non_stream_transport_error_uses_json_error_shape() {
    let state = app_state_for_single_upstream_with_timeout(
        "http://127.0.0.1:9/v1".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        std::time::Duration::from_millis(50),
    );

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/responses".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "input": "Hi",
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("json body bytes");
    let body: Value = serde_json::from_slice(&body).expect("json body");
    assert_eq!(body["error"]["type"], "server_error");
}

#[tokio::test]
async fn streaming_requests_are_not_cut_off_by_unary_upstream_timeout() {
    let (mock_base, server) =
        spawn_delayed_openai_completion_stream_mock(std::time::Duration::from_millis(150)).await;
    let state = app_state_for_single_upstream_with_timeout(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiCompletion,
        std::time::Duration::from_millis(50),
    );

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/chat/completions".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiCompletion,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("stream body bytes");
    let body = String::from_utf8(body.to_vec()).expect("utf8 stream body");
    assert!(body.contains("\"content\":\"Hi\""), "body = {body}");
    assert!(body.contains("data: [DONE]"), "body = {body}");

    server.abort();
}
