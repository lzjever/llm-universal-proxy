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

async fn spawn_openai_completion_stream_mock_with_events(
    response_events: Vec<Value>,
) -> (String, Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
    use bytes::Bytes;
    use futures_util::stream;

    #[derive(Clone)]
    struct MockState {
        requests: Arc<Mutex<Vec<Value>>>,
        response_events: Vec<Value>,
    }

    async fn handle_chat_completions(
        State(state): State<MockState>,
        Json(body): Json<Value>,
    ) -> Response<Body> {
        state.requests.lock().await.push(body);

        let pieces = state
            .response_events
            .iter()
            .flat_map(|event| {
                let event_bytes =
                    serde_json::to_vec(event).expect("serialize OpenAI streaming payload");
                vec![
                    Ok::<Bytes, std::io::Error>(Bytes::from_static(b"data: ")),
                    Ok(Bytes::from(event_bytes)),
                    Ok(Bytes::from_static(b"\n\n")),
                ]
            })
            .chain(std::iter::once(Ok(Bytes::from_static(b"data: [DONE]\n\n"))))
            .collect::<Vec<_>>();
        let body_stream = stream::iter(pieces);

        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .body(Body::from_stream(body_stream))
            .expect("streaming response")
    }

    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/chat/completions", post(handle_chat_completions))
        .with_state(MockState {
            requests: requests.clone(),
            response_events,
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind OpenAI stream mock upstream");
    let addr = listener
        .local_addr()
        .expect("OpenAI stream mock local addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("OpenAI stream mock server");
    });

    (format!("http://{addr}"), requests, server)
}

async fn spawn_openai_responses_mock(
    response_body: Value,
) -> (String, Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
    #[derive(Clone)]
    struct MockState {
        requests: Arc<Mutex<Vec<Value>>>,
        response_body: Value,
    }

    async fn handle_responses(
        State(state): State<MockState>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        state.requests.lock().await.push(body);
        Json(state.response_body)
    }

    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/responses", post(handle_responses))
        .with_state(MockState {
            requests: requests.clone(),
            response_body,
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind OpenAI Responses mock upstream");
    let addr = listener
        .local_addr()
        .expect("OpenAI Responses mock local addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("OpenAI Responses mock server");
    });

    (format!("http://{addr}"), requests, server)
}

async fn spawn_google_generate_content_mock(
    response_body: Value,
) -> (
    String,
    Arc<Mutex<Vec<(String, Value)>>>,
    tokio::task::JoinHandle<()>,
) {
    #[derive(Clone)]
    struct MockState {
        requests: Arc<Mutex<Vec<(String, Value)>>>,
        response_body: Value,
    }

    async fn handle_generate_content(
        uri: axum::http::Uri,
        State(state): State<MockState>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        state
            .requests
            .lock()
            .await
            .push((uri.path().trim_start_matches('/').to_string(), body));
        Json(state.response_body)
    }

    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/*path", post(handle_generate_content))
        .with_state(MockState {
            requests: requests.clone(),
            response_body,
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind Gemini mock upstream");
    let addr = listener.local_addr().expect("Gemini mock local addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("Gemini mock server");
    });

    (format!("http://{addr}"), requests, server)
}

async fn spawn_anthropic_messages_stream_mock(
    response_events: Vec<Value>,
) -> (String, Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
    use bytes::Bytes;
    use futures_util::stream;

    #[derive(Clone)]
    struct MockState {
        requests: Arc<Mutex<Vec<Value>>>,
        response_events: Vec<Value>,
    }

    async fn handle_messages(
        State(state): State<MockState>,
        Json(body): Json<Value>,
    ) -> Response<Body> {
        state.requests.lock().await.push(body);

        let pieces = state
            .response_events
            .iter()
            .flat_map(|event| {
                let event_type = event
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("message_delta")
                    .to_string();
                let event_bytes =
                    serde_json::to_vec(event).expect("serialize anthropic streaming payload");
                vec![
                    Ok::<Bytes, std::io::Error>(Bytes::from(format!("event: {event_type}\n"))),
                    Ok(Bytes::from_static(b"data: ")),
                    Ok(Bytes::from(event_bytes)),
                    Ok(Bytes::from_static(b"\n\n")),
                ]
            })
            .collect::<Vec<_>>();
        let body_stream = stream::iter(pieces);

        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .body(Body::from_stream(body_stream))
            .expect("streaming response")
    }

    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/v1/messages", post(handle_messages))
        .route("/messages", post(handle_messages))
        .with_state(MockState {
            requests: requests.clone(),
            response_events,
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind anthropic stream mock upstream");
    let addr = listener
        .local_addr()
        .expect("anthropic stream mock local addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("anthropic stream mock server");
    });

    (format!("http://{addr}"), requests, server)
}

fn anthropic_commentary_then_tool_use_events() -> Vec<Value> {
    vec![
        serde_json::json!({
            "type": "message_start",
            "message": {
                "id": "msg_commentary",
                "type": "message",
                "role": "assistant",
                "model": "claude-3-7-sonnet",
                "content": [],
                "stop_reason": null,
                "stop_sequence": null,
                "usage": { "input_tokens": 0, "output_tokens": 0 }
            }
        }),
        serde_json::json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": { "type": "text", "text": "" }
        }),
        serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "text_delta", "text": "Preamble line\\n" }
        }),
        serde_json::json!({
            "type": "content_block_stop",
            "index": 0
        }),
        serde_json::json!({
            "type": "content_block_start",
            "index": 1,
            "content_block": {
                "type": "tool_use",
                "id": "call_1",
                "name": "exec_command",
                "input": {}
            }
        }),
        serde_json::json!({
            "type": "content_block_delta",
            "index": 1,
            "delta": { "type": "input_json_delta", "partial_json": "{\"cmd\":\"pwd\"}" }
        }),
        serde_json::json!({
            "type": "content_block_stop",
            "index": 1
        }),
        serde_json::json!({
            "type": "message_delta",
            "delta": { "stop_reason": "tool_use" },
            "usage": { "input_tokens": 12, "output_tokens": 4 }
        }),
        serde_json::json!({
            "type": "message_stop"
        }),
    ]
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
async fn same_format_openai_streaming_passthrough_rejects_reserved_tool_name() {
    let response_events = vec![serde_json::json!({
        "id": "chatcmpl-reserved",
        "object": "chat.completion.chunk",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_reserved",
                    "type": "function",
                    "function": {
                        "name": "__llmup_custom__apply_patch",
                        "arguments": "{}"
                    }
                }]
            },
            "finish_reason": null
        }]
    })];
    let (mock_base, requests, server) =
        spawn_openai_completion_stream_mock_with_events(response_events).await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiCompletion);

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
    let body_text = String::from_utf8(body.to_vec()).expect("stream body utf8");
    assert!(
        body_text.contains("\"code\":\"reserved_openai_custom_bridge_prefix\""),
        "body = {body_text}"
    );
    assert!(
        !body_text.contains("\"name\":\"__llmup_custom__apply_patch\""),
        "same-format passthrough leaked reserved tool name: {body_text}"
    );

    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    server.abort();
}

#[tokio::test]
async fn live_openai_same_format_empty_policy_rejects_reserved_legacy_function_name() {
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
        "/openai/v1/chat/completions".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "messages": [{ "role": "user", "content": "Hi" }],
            "functions": [{
                "name": "__llmup_custom__legacy_exec",
                "parameters": { "type": "object", "properties": {} }
            }],
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiCompletion,
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
    assert_eq!(
        message,
        crate::internal_artifacts::GENERIC_UPSTREAM_ERROR_MESSAGE
    );
    assert!(!message.contains("__llmup_custom__"), "message = {message}");
    assert!(!message.contains("legacy_exec"), "message = {message}");

    let recorded = requests.lock().await;
    assert!(recorded.is_empty(), "requests = {recorded:?}");
    server.abort();
}

#[tokio::test]
async fn live_gemini_same_format_empty_policy_rejects_reserved_top_level_response_tool_name() {
    let response_body = serde_json::json!({
        "candidates": [{
            "content": {
                "role": "model",
                "parts": [{ "text": "Hi" }]
            },
            "finishReason": "STOP"
        }]
    });
    let (mock_base, requests, server) = spawn_google_generate_content_mock(response_body).await;
    let state = app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::Google);

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/google/v1beta/models/gemini-2.5-flash:generateContent".to_string(),
        serde_json::json!({
            "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
            "response": {
                "candidates": [{
                    "content": {
                        "role": "model",
                        "parts": [{
                            "functionCall": {
                                "name": "__llmup_custom__apply_patch",
                                "args": {}
                            }
                        }]
                    }
                }]
            }
        }),
        "gemini-2.5-flash".to_string(),
        crate::formats::UpstreamFormat::Google,
        Some(false),
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
    assert_eq!(
        message,
        crate::internal_artifacts::GENERIC_UPSTREAM_ERROR_MESSAGE
    );
    assert!(!message.contains("__llmup_custom__"), "message = {message}");
    assert!(!message.contains("apply_patch"), "message = {message}");

    let recorded = requests.lock().await;
    assert!(recorded.is_empty(), "requests = {recorded:?}");
    server.abort();
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
async fn live_responses_custom_tool_bridge_to_openai_restores_non_stream_response_custom_tool_call()
{
    let patch_input = "*** Begin Patch\n*** Add File: hello.txt\n+hello\n*** End Patch\n";
    let response_body = serde_json::json!({
        "id": "chatcmpl_1",
        "object": "chat.completion",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_apply_patch",
                    "type": "function",
                    "function": {
                        "name": "apply_patch",
                        "arguments": serde_json::to_string(
                            &serde_json::json!({ "input": patch_input })
                        )
                        .expect("bridge args")
                    }
                }]
            },
            "finish_reason": "tool_calls"
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
                "format": { "type": "text" }
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
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("json body bytes");
    let body: Value = serde_json::from_slice(&body).expect("json body");
    assert_eq!(
        body["output"][0],
        serde_json::json!({
            "type": "custom_tool_call",
            "call_id": "call_apply_patch",
            "name": "apply_patch",
            "input": patch_input
        })
    );
    let serialized = body.to_string();
    assert!(
        !serialized.contains("__llmup_custom__"),
        "live response leaked reserved bridge prefix: {body:?}"
    );

    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    let upstream_body = &recorded[0];
    assert_eq!(upstream_body["tools"][0]["function"]["name"], "apply_patch");
    assert!(
        upstream_body
            .get(INTERNAL_TOOL_BRIDGE_CONTEXT_FIELD)
            .is_none(),
        "internal bridge context must not be sent upstream: {upstream_body:?}"
    );

    server.abort();
}

#[tokio::test]
async fn live_openai_responses_same_format_success_rejects_upstream_bridge_context_leak() {
    let response_body = serde_json::json!({
        "_llmup_tool_bridge_context": {
            "version": 1,
            "compatibility_mode": "balanced",
            "entries": {
                "code_exec": {
                    "stable_name": "code_exec",
                    "source_kind": "custom_text",
                    "transport_kind": "function_object_wrapper",
                    "wrapper_field": "input",
                    "expected_canonical_shape": "single_required_string"
                }
            }
        },
        "id": "resp_leaky_context",
        "object": "response",
        "created_at": 1,
        "status": "completed",
        "output": []
    });
    let (mock_base, requests, server) = spawn_openai_responses_mock(response_body).await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);

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
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("json body bytes");
    let body_text = String::from_utf8(body.to_vec()).expect("json body utf8");
    assert!(
        !body_text.contains("_llmup_tool_bridge_context"),
        "public egress leaked internal bridge context: {body_text}"
    );
    assert!(
        !body_text.contains("__llmup_custom__"),
        "public egress leaked internal custom prefix: {body_text}"
    );

    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    server.abort();
}

#[tokio::test]
async fn live_openai_responses_same_format_success_rejects_reserved_tool_identity_without_leak() {
    let response_body = serde_json::json!({
        "id": "resp_reserved_identity",
        "object": "response",
        "created_at": 1,
        "status": "completed",
        "output": [{
            "type": "custom_tool_call",
            "call_id": "call_reserved",
            "name": "__llmup_custom__apply_patch",
            "input": "*** Begin Patch\n*** End Patch\n"
        }]
    });
    let (mock_base, requests, server) = spawn_openai_responses_mock(response_body).await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);

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
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("json body bytes");
    let body_text = String::from_utf8(body.to_vec()).expect("json body utf8");
    assert!(
        !body_text.contains("__llmup_custom__"),
        "same-format non-stream rejection leaked reserved prefix: {body_text}"
    );
    assert!(
        !body_text.contains("_llmup_tool_bridge_context"),
        "same-format non-stream rejection leaked bridge context field: {body_text}"
    );

    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    server.abort();
}

#[tokio::test]
async fn live_openai_responses_same_format_success_preserves_regular_text_and_schema_descriptions()
{
    let public_text = "plain success text mentions __llmup_custom__apply_patch";
    let schema_description =
        "schema docs may mention __llmup_custom__apply_patch as literal user text";
    let response_body = serde_json::json!({
        "id": "resp_plain_reserved_text",
        "object": "response",
        "created_at": 1,
        "status": "completed",
        "output": [{
            "type": "message",
            "id": "msg_plain_reserved_text",
            "role": "assistant",
            "content": [{
                "type": "output_text",
                "text": public_text,
                "annotations": []
            }]
        }],
        "tools": [{
            "type": "function",
            "name": "describe_patch_token",
            "description": schema_description,
            "parameters": {
                "type": "object",
                "properties": {
                    "literal": {
                        "type": "string",
                        "description": schema_description
                    }
                }
            }
        }]
    });
    let (mock_base, requests, server) = spawn_openai_responses_mock(response_body).await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);

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

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("json body bytes");
    let body_text = String::from_utf8(body.to_vec()).expect("json body utf8");
    let body: Value = serde_json::from_str(&body_text).expect("json body");
    assert_eq!(body["output"][0]["content"][0]["text"], public_text);
    assert_eq!(body["tools"][0]["description"], schema_description);
    assert_eq!(
        body["tools"][0]["parameters"]["properties"]["literal"]["description"],
        schema_description
    );
    assert!(body_text.contains("__llmup_custom__apply_patch"));

    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    server.abort();
}

#[tokio::test]
async fn same_format_openai_streaming_passthrough_preserves_regular_delta_content() {
    let public_text = "delta content mentions __llmup_custom__apply_patch as text";
    let response_events = vec![serde_json::json!({
        "id": "chatcmpl-plain-reserved-text",
        "object": "chat.completion.chunk",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "delta": {
                "content": public_text
            },
            "finish_reason": null
        }]
    })];
    let (mock_base, requests, server) =
        spawn_openai_completion_stream_mock_with_events(response_events).await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiCompletion);

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
    let body_text = String::from_utf8(body.to_vec()).expect("stream body utf8");
    assert!(body_text.contains(public_text), "body = {body_text}");
    assert!(
        !body_text.contains("reserved_openai_custom_bridge_prefix"),
        "plain delta content should not be treated as a reserved tool identity: {body_text}"
    );
    assert!(
        !body_text.contains("response.failed"),
        "plain delta content should not fail the stream: {body_text}"
    );

    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
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
                "version": 1,
                "compatibility_mode": "max_compat",
                "entries": {
                    "code_exec": {
                        "stable_name": "code_exec",
                        "source_kind": "custom_text",
                        "transport_kind": "function_object_wrapper",
                        "wrapper_field": "input",
                        "expected_canonical_shape": "single_required_string"
                    }
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
    assert_eq!(
        message,
        crate::internal_artifacts::GENERIC_UPSTREAM_ERROR_MESSAGE
    );
    assert!(
        !message.contains(INTERNAL_TOOL_BRIDGE_CONTEXT_FIELD),
        "message = {message}"
    );

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
async fn live_responses_anthropic_stream_emits_commentary_message_done_before_tool_item() {
    let (mock_base, requests, server) =
        spawn_anthropic_messages_stream_mock(anthropic_commentary_then_tool_use_events()).await;
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
                "content": [{ "type": "input_text", "text": "Run pwd" }]
            }],
            "tools": [{
                "type": "function",
                "name": "exec_command",
                "description": "Run a shell command",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "cmd": { "type": "string" }
                    },
                    "required": ["cmd"],
                    "additionalProperties": false
                }
            }],
            "tool_choice": {
                "type": "function",
                "name": "exec_command"
            },
            "stream": true
        }),
        "claude-3-7-sonnet".to_string(),
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

    let commentary_done_idx = events
        .iter()
        .position(|event| {
            event.get("type").and_then(Value::as_str) == Some("response.output_item.done")
                && event["item"]["type"] == "message"
        })
        .expect("completed assistant message item");
    let commentary_done = &events[commentary_done_idx];
    assert_eq!(
        commentary_done["item"]["phase"], "commentary",
        "body = {body_text}"
    );
    assert_eq!(commentary_done["item"]["status"], "completed");
    assert!(
        commentary_done["item"]["content"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|part| {
                part.get("type").and_then(Value::as_str) == Some("output_text")
                    && part
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .contains("Preamble line")
            }),
        "body = {body_text}"
    );

    let tool_added_idx = events
        .iter()
        .position(|event| {
            event.get("type").and_then(Value::as_str) == Some("response.output_item.added")
                && event["item"]["type"] == "function_call"
                && event["item"]["name"] == "exec_command"
        })
        .expect("function-call item added");
    assert!(
        commentary_done_idx < tool_added_idx,
        "commentary message should complete before tool work begins: {body_text}"
    );

    let tool_args_delta_idx = events
        .iter()
        .position(|event| {
            event.get("type").and_then(Value::as_str)
                == Some("response.function_call_arguments.delta")
                && event["name"] == "exec_command"
        })
        .expect("function-call arguments delta");
    let tool_done_idx = events
        .iter()
        .position(|event| {
            event.get("type").and_then(Value::as_str) == Some("response.output_item.done")
                && event["item"]["type"] == "function_call"
                && event["item"]["name"] == "exec_command"
        })
        .expect("function-call item done");
    assert!(
        tool_added_idx < tool_args_delta_idx && tool_args_delta_idx < tool_done_idx,
        "tool item lifecycle should stay intact: {body_text}"
    );
    assert_eq!(
        events[tool_done_idx]["item"]["arguments"],
        "{\"cmd\":\"pwd\"}"
    );
    assert!(
        !body_text.contains("__llmup_custom__"),
        "translated stream must not leak internal bridge artifacts: {body_text}"
    );

    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    assert_eq!(recorded[0]["tools"][0]["name"], "exec_command");
    assert_eq!(recorded[0]["stream"], true);

    server.abort();
}

#[tokio::test]
async fn live_responses_anthropic_stream_keeps_tool_item_lifecycle_after_commentary_preamble() {
    let (mock_base, _requests, server) =
        spawn_anthropic_messages_stream_mock(anthropic_commentary_then_tool_use_events()).await;
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
                "content": [{ "type": "input_text", "text": "Run pwd" }]
            }],
            "tools": [{
                "type": "function",
                "name": "exec_command",
                "description": "Run a shell command",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "cmd": { "type": "string" }
                    },
                    "required": ["cmd"],
                    "additionalProperties": false
                }
            }],
            "tool_choice": {
                "type": "function",
                "name": "exec_command"
            },
            "stream": true
        }),
        "claude-3-7-sonnet".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("stream body bytes");
    let body_text = String::from_utf8(body.to_vec()).expect("stream body utf8");
    let events = parse_sse_events(&body);

    let tool_added_idx = events
        .iter()
        .position(|event| {
            event.get("type").and_then(Value::as_str) == Some("response.output_item.added")
                && event["item"]["type"] == "function_call"
                && event["item"]["name"] == "exec_command"
        })
        .expect("function-call item added");
    let tool_args_delta_idx = events
        .iter()
        .position(|event| {
            event.get("type").and_then(Value::as_str)
                == Some("response.function_call_arguments.delta")
                && event["name"] == "exec_command"
        })
        .expect("function-call arguments delta");
    let tool_args_done_idx = events
        .iter()
        .position(|event| {
            event.get("type").and_then(Value::as_str)
                == Some("response.function_call_arguments.done")
                && event["name"] == "exec_command"
        })
        .expect("function-call arguments done");
    let tool_done_idx = events
        .iter()
        .position(|event| {
            event.get("type").and_then(Value::as_str) == Some("response.output_item.done")
                && event["item"]["type"] == "function_call"
                && event["item"]["name"] == "exec_command"
        })
        .expect("function-call item done");

    assert!(
        tool_added_idx < tool_args_delta_idx
            && tool_args_delta_idx < tool_args_done_idx
            && tool_args_done_idx < tool_done_idx,
        "tool item lifecycle should stay intact: {body_text}"
    );
    assert_eq!(
        events[tool_done_idx]["item"]["arguments"],
        "{\"cmd\":\"pwd\"}"
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
        recorded[0]["max_tokens"], 128_000,
        "configured default output limit should propagate to real Anthropic upstream body when the client omits it: {:?}",
        recorded[0]
    );

    server.abort();
}

#[tokio::test]
async fn live_openai_same_format_request_applies_surface_parallel_tool_gate() {
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
        namespace.config.model_aliases.insert(
            "serial-openai".to_string(),
            crate::config::ModelAlias {
                upstream_name: "primary".to_string(),
                upstream_model: "gpt-4o-mini".to_string(),
                limits: None,
                surface: Some(crate::config::ModelSurfacePatch {
                    modalities: None,
                    tools: Some(crate::config::ModelToolSurface {
                        supports_search: None,
                        supports_view_image: None,
                        apply_patch_transport: None,
                        supports_parallel_calls: Some(false),
                    }),
                }),
            },
        );
    }

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/chat/completions".to_string(),
        serde_json::json!({
            "model": "serial-openai",
            "messages": [{ "role": "user", "content": "Hi" }],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "lookup_weather",
                    "parameters": { "type": "object", "properties": {} }
                }
            }],
            "stream": false
        }),
        "serial-openai".to_string(),
        crate::formats::UpstreamFormat::OpenAiCompletion,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);

    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    assert_eq!(recorded[0]["model"], "gpt-4o-mini");
    assert_eq!(
        recorded[0]["parallel_tool_calls"],
        false,
        "same-format request should still inherit ModelSurface parallel-call policy before hitting the upstream: {:?}",
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
        proxy: Some(crate::config::ProxyConfig::Direct),
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
                proxy: None,
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
                proxy: None,
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
        proxy: Some(crate::config::ProxyConfig::Direct),
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
                proxy: None,
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
                proxy: None,
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
