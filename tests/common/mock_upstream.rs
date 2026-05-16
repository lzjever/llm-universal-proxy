//! Mock upstream servers that speak each protocol per official API specs.
//! Used by integration tests to validate proxy passthrough and translation.
#![allow(dead_code)]

use axum::{
    body::Body,
    extract::{Json, Path, State},
    http::{Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use serde_json::Value;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::net::TcpListener;

/// Spawns a mock upstream that speaks OpenAI Chat Completions API.
/// Returns (base_url, _handle). Server responds at POST /chat/completions.
/// Non-streaming: returns full ChatCompletion JSON. Streaming: returns SSE chunks then [DONE].
pub async fn spawn_openai_completion_mock() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");

    let app = Router::new()
        .route("/v1/chat/completions", post(openai_completion_handler))
        .route("/chat/completions", post(openai_completion_handler));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle)
}

async fn openai_completion_handler(Json(body): Json<Value>) -> Response {
    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    if stream {
        let id = body.get("model").and_then(Value::as_str).unwrap_or("mock");
        let chunks = [
            format!(
                r#"data: {{"id":"chatcmpl-{id}","object":"chat.completion.chunk","created":1,"model":"{id}","choices":[{{"index":0,"delta":{{"role":"assistant"}},"finish_reason":null}}]}}"#
            ),
            r#"data: {"id":"chatcmpl-mock","object":"chat.completion.chunk","created":1,"model":"mock","choices":[{"index":0,"delta":{"content":"Hi"},"finish_reason":null}]}"#.to_string(),
            r#"data: {"id":"chatcmpl-mock","object":"chat.completion.chunk","created":1,"model":"mock","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#.to_string(),
            "data: [DONE]".to_string(),
        ];
        let body = chunks.join("\n\n") + "\n\n";
        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .body(Body::from(body))
            .unwrap()
    } else {
        let resp = serde_json::json!({
            "id": "chatcmpl-mock",
            "object": "chat.completion",
            "created": 1,
            "model": body.get("model").unwrap_or(&serde_json::json!("mock")),
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": "Hi" }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
        });
        (StatusCode::OK, Json(resp)).into_response()
    }
}

/// Spawns a mock that speaks Anthropic Messages API.
/// Responds at POST /messages. Non-streaming: content array; streaming: message_start, content_block_*, message_delta, message_stop.
pub async fn spawn_anthropic_mock() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");

    let app = Router::new()
        .route("/v1/messages", post(anthropic_handler))
        .route("/messages", post(anthropic_handler));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle)
}

pub async fn spawn_anthropic_thinking_mock() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");

    let app = Router::new()
        .route("/v1/messages", post(anthropic_thinking_handler))
        .route("/messages", post(anthropic_thinking_handler));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle)
}

pub async fn spawn_anthropic_signed_thinking_mock() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");

    let app = Router::new()
        .route("/v1/messages", post(anthropic_signed_thinking_handler))
        .route("/messages", post(anthropic_signed_thinking_handler));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle)
}

pub async fn spawn_anthropic_omitted_thinking_mock() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");

    let app = Router::new()
        .route("/v1/messages", post(anthropic_omitted_thinking_handler))
        .route("/messages", post(anthropic_omitted_thinking_handler));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle)
}

async fn anthropic_thinking_handler(Json(body): Json<Value>) -> Response {
    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    if stream {
        let events = [
            r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"claude-3","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":0,"output_tokens":0}}}"#,
            r#"event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            r#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"think"}}"#,
            r#"event: content_block_stop
data: {"type":"content_block_stop","index":0}"#,
            r#"event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
            r#"event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Hi"}}"#,
            r#"event: content_block_stop
data: {"type":"content_block_stop","index":1}"#,
            r#"event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":1,"output_tokens":2}}"#,
            r#"event: message_stop
data: {"type":"message_stop"}"#,
        ];
        let body = events.join("\n\n") + "\n\n";
        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .body(Body::from(body))
            .unwrap()
    } else {
        let resp = serde_json::json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "content": [
                { "type": "thinking", "thinking": "think" },
                { "type": "text", "text": "Hi" }
            ],
            "model": body.get("model").unwrap_or(&serde_json::json!("claude-3")),
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": { "input_tokens": 1, "output_tokens": 2 }
        });
        (StatusCode::OK, Json(resp)).into_response()
    }
}

async fn anthropic_signed_thinking_handler(Json(body): Json<Value>) -> Response {
    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    if stream {
        let events = [
            r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_signed_thinking","type":"message","role":"assistant","model":"claude-3","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":0,"output_tokens":0}}}"#,
            r#"event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":"sig_123"}}"#,
            r#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"internal reasoning"}}"#,
            r#"event: content_block_stop
data: {"type":"content_block_stop","index":0}"#,
            r#"event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
            r#"event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Visible answer"}}"#,
            r#"event: content_block_stop
data: {"type":"content_block_stop","index":1}"#,
            r#"event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":1,"output_tokens":2}}"#,
            r#"event: message_stop
data: {"type":"message_stop"}"#,
        ];
        let body = events.join("\n\n") + "\n\n";
        return Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .body(Body::from(body))
            .unwrap();
    }

    let resp = serde_json::json!({
        "id": "msg_signed_thinking",
        "type": "message",
        "role": "assistant",
        "content": [
            {
                "type": "thinking",
                "thinking": "internal reasoning",
                "signature": "sig_123"
            },
            { "type": "text", "text": "Visible answer" }
        ],
        "model": body.get("model").unwrap_or(&serde_json::json!("claude-3")),
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": { "input_tokens": 1, "output_tokens": 2 }
    });
    (StatusCode::OK, Json(resp)).into_response()
}

async fn anthropic_omitted_thinking_handler(Json(body): Json<Value>) -> Response {
    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    if stream {
        let events = [
            r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_omitted_thinking","type":"message","role":"assistant","model":"claude-3","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":0,"output_tokens":0}}}"#,
            r#"event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":{"display":"omitted"},"signature":"sig_omitted"}}"#,
            r#"event: content_block_stop
data: {"type":"content_block_stop","index":0}"#,
            r#"event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
            r#"event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Visible answer"}}"#,
            r#"event: content_block_stop
data: {"type":"content_block_stop","index":1}"#,
            r#"event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":1,"output_tokens":2}}"#,
            r#"event: message_stop
data: {"type":"message_stop"}"#,
        ];
        let body = events.join("\n\n") + "\n\n";
        return Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .body(Body::from(body))
            .unwrap();
    }

    let resp = serde_json::json!({
        "id": "msg_omitted_thinking",
        "type": "message",
        "role": "assistant",
        "content": [
            {
                "type": "thinking",
                "thinking": { "display": "omitted" },
                "signature": "sig_omitted"
            },
            { "type": "text", "text": "Visible answer" }
        ],
        "model": body.get("model").unwrap_or(&serde_json::json!("claude-3")),
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": { "input_tokens": 1, "output_tokens": 2 }
    });
    (StatusCode::OK, Json(resp)).into_response()
}

async fn anthropic_handler(Json(body): Json<Value>) -> Response {
    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    if stream {
        let events = [
            r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"claude-3","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":0,"output_tokens":0}}}"#,
            r#"event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#,
            r#"event: content_block_stop
data: {"type":"content_block_stop","index":0}"#,
            r#"event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":1,"output_tokens":1}}"#,
            r#"event: message_stop
data: {"type":"message_stop"}"#,
        ];
        let body = events.join("\n\n") + "\n\n";
        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .body(Body::from(body))
            .unwrap()
    } else {
        let resp = serde_json::json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "text", "text": "Hi" }],
            "model": body.get("model").unwrap_or(&serde_json::json!("claude-3")),
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": { "input_tokens": 1, "output_tokens": 1 }
        });
        (StatusCode::OK, Json(resp)).into_response()
    }
}

/// Spawns a mock that speaks OpenAI Responses API.
/// Responds at POST /responses. Non-streaming: output array; streaming: response.created, response.output_text.delta, response.completed.
pub async fn spawn_openai_responses_mock() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");

    let app = Router::new()
        .route("/v1/responses", post(openai_responses_handler))
        .route(
            "/v1/responses/compact",
            post(openai_responses_compact_handler),
        )
        .route(
            "/v1/responses/:response_id",
            get(openai_responses_get_handler).delete(openai_responses_delete_handler),
        )
        .route(
            "/v1/responses/:response_id/cancel",
            post(openai_responses_cancel_handler),
        )
        .route("/responses", post(openai_responses_handler));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle)
}

async fn openai_responses_get_handler(Path(response_id): Path<String>) -> Response {
    let resp = serde_json::json!({
        "id": response_id,
        "object": "response",
        "created_at": 1,
        "status": "completed",
        "output": [],
        "usage": { "input_tokens": 1, "output_tokens": 1, "total_tokens": 2 }
    });
    (StatusCode::OK, Json(resp)).into_response()
}

async fn openai_responses_delete_handler(Path(response_id): Path<String>) -> Response {
    let resp = serde_json::json!({
        "id": response_id,
        "object": "response.deleted",
        "deleted": true
    });
    (StatusCode::OK, Json(resp)).into_response()
}

async fn openai_responses_cancel_handler(Path(response_id): Path<String>) -> Response {
    let resp = serde_json::json!({
        "id": response_id,
        "object": "response",
        "status": "cancelled",
        "output": []
    });
    (StatusCode::OK, Json(resp)).into_response()
}

async fn openai_responses_compact_handler(Json(_body): Json<Value>) -> Response {
    let resp = serde_json::json!({
        "object": "response",
        "id": "resp_compacted",
        "status": "completed",
        "output": [],
        "usage": { "input_tokens": 1, "output_tokens": 0, "total_tokens": 1 }
    });
    (StatusCode::OK, Json(resp)).into_response()
}

pub async fn spawn_openai_responses_reasoning_mock() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");

    let app = Router::new()
        .route("/v1/responses", post(openai_responses_reasoning_handler))
        .route("/responses", post(openai_responses_reasoning_handler));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle)
}

pub async fn spawn_openai_responses_reasoning_with_encrypted_carrier_mock(
) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");

    let app = Router::new()
        .route(
            "/v1/responses",
            post(openai_responses_reasoning_with_encrypted_carrier_handler),
        )
        .route(
            "/responses",
            post(openai_responses_reasoning_with_encrypted_carrier_handler),
        );
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle)
}

async fn openai_responses_reasoning_handler(Json(body): Json<Value>) -> Response {
    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    if stream {
        let events = [
            r#"event: response.created
data: {"type":"response.created","sequence_number":1,"response":{"id":"resp_1","object":"response","created_at":1,"status":"in_progress","background":false,"error":null,"output":[]}}"#,
            r#"event: response.reasoning_summary_text.delta
data: {"type":"response.reasoning_summary_text.delta","sequence_number":2,"output_index":0,"summary_index":0,"delta":"think"}"#,
            r#"event: response.output_text.delta
data: {"type":"response.output_text.delta","sequence_number":3,"output_index":1,"delta":"Hi"}"#,
            r#"event: response.completed
data: {"type":"response.completed","sequence_number":4,"response":{"id":"resp_1","object":"response","created_at":1,"status":"completed","output":[{"id":"rs_1","type":"reasoning","summary":[{"type":"summary_text","text":"think"}]},{"id":"msg_1","type":"message","role":"assistant","content":[{"type":"output_text","text":"Hi"}]}],"usage":{"input_tokens":1,"output_tokens":2,"output_tokens_details":{"reasoning_tokens":1}}}}"#,
        ];
        let body = events.join("\n\n") + "\n\n";
        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .body(Body::from(body))
            .unwrap()
    } else {
        let resp = serde_json::json!({
            "id": "resp_1",
            "object": "response",
            "created_at": 1,
            "status": "completed",
            "output": [
                { "id": "rs_1", "type": "reasoning", "summary": [{ "type": "summary_text", "text": "think" }] },
                { "id": "msg_1", "type": "message", "role": "assistant", "content": [{ "type": "output_text", "text": "Hi" }] }
            ],
            "usage": { "input_tokens": 1, "output_tokens": 2, "output_tokens_details": { "reasoning_tokens": 1 } }
        });
        (StatusCode::OK, Json(resp)).into_response()
    }
}

async fn openai_responses_reasoning_with_encrypted_carrier_handler(
    Json(body): Json<Value>,
) -> Response {
    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let encrypted_content = "anthropic-thinking-v1:7b22666f726d6174223a22616e7468726f7069632d7468696e6b696e672d7265706c6179222c2276657273696f6e223a312c22626c6f636b73223a5b7b2274797065223a227468696e6b696e67222c227468696e6b696e67223a227468696e6b222c227369676e6174757265223a227369675f73747265616d227d5d7d";
    if stream {
        let events = [
            r#"event: response.created
data: {"type":"response.created","sequence_number":1,"response":{"id":"resp_enc","object":"response","created_at":1,"status":"in_progress","background":false,"error":null,"output":[]}}"#
                .to_string(),
            r#"event: response.reasoning_summary_text.delta
data: {"type":"response.reasoning_summary_text.delta","sequence_number":2,"output_index":0,"summary_index":0,"delta":"think"}"#
                .to_string(),
            r#"event: response.output_text.delta
data: {"type":"response.output_text.delta","sequence_number":3,"output_index":1,"delta":"Hi"}"#
                .to_string(),
            format!(
                "event: response.completed\ndata: {{\"type\":\"response.completed\",\"sequence_number\":4,\"response\":{{\"id\":\"resp_enc\",\"object\":\"response\",\"created_at\":1,\"status\":\"completed\",\"output\":[{{\"id\":\"rs_enc\",\"type\":\"reasoning\",\"summary\":[{{\"type\":\"summary_text\",\"text\":\"think\"}}],\"encrypted_content\":\"{encrypted_content}\"}},{{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[{{\"type\":\"output_text\",\"text\":\"Hi\"}}]}}],\"usage\":{{\"input_tokens\":1,\"output_tokens\":2,\"output_tokens_details\":{{\"reasoning_tokens\":1}}}}}}}}"
            ),
        ];
        let body = events.join("\n\n") + "\n\n";
        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .body(Body::from(body))
            .unwrap()
    } else {
        let resp = serde_json::json!({
            "id": "resp_enc",
            "object": "response",
            "created_at": 1,
            "status": "completed",
            "output": [
                {
                    "id": "rs_enc",
                    "type": "reasoning",
                    "summary": [{ "type": "summary_text", "text": "think" }],
                    "encrypted_content": encrypted_content
                },
                {
                    "id": "msg_1",
                    "type": "message",
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": "Hi" }]
                }
            ],
            "usage": { "input_tokens": 1, "output_tokens": 2, "output_tokens_details": { "reasoning_tokens": 1 } }
        });
        (StatusCode::OK, Json(resp)).into_response()
    }
}

async fn openai_responses_handler(Json(body): Json<Value>) -> Response {
    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    if stream {
        let events = [
            r#"event: response.created
data: {"type":"response.created","sequence_number":1,"response":{"id":"resp_1","object":"response","created_at":1,"status":"in_progress","background":false,"error":null,"output":[]}}"#,
            r#"event: response.in_progress
data: {"type":"response.in_progress","sequence_number":2,"response":{"id":"resp_1","object":"response","created_at":1,"status":"in_progress"}}"#,
            r#"event: response.output_text.delta
data: {"type":"response.output_text.delta","sequence_number":3,"output_index":0,"delta":"Hi"}"#,
            r#"event: response.completed
data: {"type":"response.completed","sequence_number":4,"response":{"id":"resp_1","object":"response","created_at":1,"status":"completed","output":[],"usage":{"input_tokens":1,"output_tokens":1}}}"#,
        ];
        let body = events.join("\n\n") + "\n\n";
        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .body(Body::from(body))
            .unwrap()
    } else {
        let resp = serde_json::json!({
            "id": "resp_1",
            "object": "response",
            "created_at": 1,
            "status": "completed",
            "output": [
                { "type": "message", "content": [{ "type": "output_text", "text": "Hi" }] }
            ],
            "usage": { "input_tokens": 1, "output_tokens": 1 }
        });
        (StatusCode::OK, Json(resp)).into_response()
    }
}

/// Spawns a mock OpenAI Chat Completions upstream that returns `reasoning_content`.
pub async fn spawn_openai_completion_reasoning_mock() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");

    let app = Router::new()
        .route(
            "/v1/chat/completions",
            post(openai_completion_reasoning_handler),
        )
        .route(
            "/chat/completions",
            post(openai_completion_reasoning_handler),
        );
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle)
}

async fn openai_completion_reasoning_handler(Json(body): Json<Value>) -> Response {
    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let model = body.get("model").and_then(Value::as_str).unwrap_or("mock");
    if stream {
        let chunks = [
            format!(
                r#"data: {{"id":"chatcmpl-rs","object":"chat.completion.chunk","created":1,"model":"{model}","choices":[{{"index":0,"delta":{{"role":"assistant","reasoning_content":"think"}},"finish_reason":null}}]}}"#
            ),
            format!(
                r#"data: {{"id":"chatcmpl-rs","object":"chat.completion.chunk","created":1,"model":"{model}","choices":[{{"index":0,"delta":{{"content":"Hi"}},"finish_reason":null}}]}}"#
            ),
            format!(
                r#"data: {{"id":"chatcmpl-rs","object":"chat.completion.chunk","created":1,"model":"{model}","choices":[{{"index":0,"delta":{{}},"finish_reason":"stop"}}],"usage":{{"prompt_tokens":1,"completion_tokens":3,"total_tokens":4,"completion_tokens_details":{{"reasoning_tokens":1}}}}}}"#
            ),
            "data: [DONE]".to_string(),
        ];
        let body = chunks.join("\n\n") + "\n\n";
        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .body(Body::from(body))
            .unwrap()
    } else {
        let resp = serde_json::json!({
            "id": "chatcmpl-rs",
            "object": "chat.completion",
            "created": 1,
            "model": model,
            "choices": [{ "index": 0, "message": { "role": "assistant", "reasoning_content": "think", "content": "Hi" }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 3, "total_tokens": 4, "completion_tokens_details": { "reasoning_tokens": 1 } }
        });
        (StatusCode::OK, Json(resp)).into_response()
    }
}

/// Spawns a mock Anthropic upstream that returns thinking + text + tool_use blocks.
pub async fn spawn_anthropic_thinking_with_tools_mock() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");

    let app = Router::new()
        .route("/v1/messages", post(anthropic_thinking_tools_handler))
        .route("/messages", post(anthropic_thinking_tools_handler));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle)
}

async fn anthropic_thinking_tools_handler(Json(body): Json<Value>) -> Response {
    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    if stream {
        let events = [
            r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_tools","type":"message","role":"assistant","model":"claude-3","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":0,"output_tokens":0}}}"#,
            r#"event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            r#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"need to call tool"}}"#,
            r#"event: content_block_stop
data: {"type":"content_block_stop","index":0}"#,
            r#"event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
            r#"event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Calling tool."}}"#,
            r#"event: content_block_stop
data: {"type":"content_block_stop","index":1}"#,
            r#"event: content_block_start
data: {"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"tool_1","name":"get_weather","input":{}}}"#,
            r#"event: content_block_delta
data: {"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"city\":\"Tokyo\"}"}}"#,
            r#"event: content_block_stop
data: {"type":"content_block_stop","index":2}"#,
            r#"event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"input_tokens":5,"output_tokens":10}}"#,
            r#"event: message_stop
data: {"type":"message_stop"}"#,
        ];
        let body = events.join("\n\n") + "\n\n";
        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .body(Body::from(body))
            .unwrap()
    } else {
        let resp = serde_json::json!({
            "id": "msg_tools",
            "type": "message",
            "role": "assistant",
            "content": [
                { "type": "thinking", "thinking": "need to call tool" },
                { "type": "text", "text": "Calling tool." },
                { "type": "tool_use", "id": "tool_1", "name": "get_weather", "input": { "city": "Tokyo" } }
            ],
            "model": body.get("model").unwrap_or(&serde_json::json!("claude-3")),
            "stop_reason": "tool_use",
            "stop_sequence": null,
            "usage": { "input_tokens": 5, "output_tokens": 10 }
        });
        (StatusCode::OK, Json(resp)).into_response()
    }
}

/// Spawns a mock that captures request body and returns a simple OpenAI Chat Completion response.
/// The captured body can be retrieved via the returned `captured` watch channel.
pub async fn spawn_capture_openai_completion_mock() -> (
    String,
    tokio::task::JoinHandle<()>,
    tokio::sync::watch::Receiver<Option<Value>>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");

    let (tx, rx) = tokio::sync::watch::channel(None);

    let app = Router::new()
        .route(
            "/v1/chat/completions",
            post(capture_openai_completion_handler),
        )
        .route("/chat/completions", post(capture_openai_completion_handler));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app.with_state(tx)).await.ok();
    });
    (base, handle, rx)
}

async fn capture_openai_completion_handler(
    axum::extract::State(tx): axum::extract::State<tokio::sync::watch::Sender<Option<Value>>>,
    Json(body): Json<Value>,
) -> Response {
    let _ = tx.send(Some(body.clone()));
    let resp = serde_json::json!({
        "id": "chatcmpl-captured",
        "object": "chat.completion",
        "created": 1,
        "model": "mock",
        "choices": [{ "index": 0, "message": { "role": "assistant", "content": "OK" }, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
    });
    (StatusCode::OK, Json(resp)).into_response()
}

/// Spawns a mock Anthropic Messages upstream that captures request bodies.
/// The captured body can be retrieved via the returned `captured` watch channel.
pub async fn spawn_capture_anthropic_mock() -> (
    String,
    tokio::task::JoinHandle<()>,
    tokio::sync::watch::Receiver<Option<Value>>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");

    let (tx, rx) = tokio::sync::watch::channel(None);

    let app = Router::new()
        .route("/v1/messages", post(capture_anthropic_handler))
        .route("/messages", post(capture_anthropic_handler));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app.with_state(tx)).await.ok();
    });
    (base, handle, rx)
}

async fn capture_anthropic_handler(
    axum::extract::State(tx): axum::extract::State<tokio::sync::watch::Sender<Option<Value>>>,
    Json(body): Json<Value>,
) -> Response {
    let _ = tx.send(Some(body.clone()));
    let resp = serde_json::json!({
        "id": "msg_captured",
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "text", "text": "OK" }],
        "model": body.get("model").unwrap_or(&serde_json::json!("claude-3")),
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": { "input_tokens": 1, "output_tokens": 1 }
    });
    (StatusCode::OK, Json(resp)).into_response()
}

/// Request captured by a spec-aware asserting mock upstream.
#[derive(Clone, Debug)]
pub struct CapturedMockRequest {
    pub method: String,
    pub path: String,
    pub body: Value,
}

/// Thread-safe request log for asserting mock upstreams.
#[derive(Clone, Default)]
pub struct CapturedMockRequests {
    requests: Arc<Mutex<Vec<CapturedMockRequest>>>,
}

impl CapturedMockRequests {
    fn push(&self, request: CapturedMockRequest) {
        self.requests.lock().unwrap().push(request);
    }

    pub fn snapshot(&self) -> Vec<CapturedMockRequest> {
        self.requests.lock().unwrap().clone()
    }

    pub async fn wait_for_count(
        &self,
        count: usize,
        timeout: Duration,
    ) -> Vec<CapturedMockRequest> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let snapshot = self.snapshot();
            if snapshot.len() >= count || tokio::time::Instant::now() >= deadline {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

type RequestAssertion =
    Arc<dyn Fn(&CapturedMockRequest) -> Result<(), String> + Send + Sync + 'static>;

#[derive(Clone)]
struct AssertingMockState {
    captured: CapturedMockRequests,
    assertion: RequestAssertion,
}

/// Spawns an OpenAI Chat Completions mock that captures and asserts request shape.
pub async fn spawn_asserting_openai_completion_mock(
    assertion: impl Fn(&CapturedMockRequest) -> Result<(), String> + Send + Sync + 'static,
) -> (String, tokio::task::JoinHandle<()>, CapturedMockRequests) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");

    let captured = CapturedMockRequests::default();
    let state = AssertingMockState {
        captured: captured.clone(),
        assertion: Arc::new(assertion),
    };

    let app = Router::new()
        .route(
            "/v1/chat/completions",
            post(asserting_openai_completion_handler),
        )
        .route(
            "/chat/completions",
            post(asserting_openai_completion_handler),
        );
    let handle = tokio::spawn(async move {
        axum::serve(listener, app.with_state(state)).await.ok();
    });
    (base, handle, captured)
}

async fn asserting_openai_completion_handler(
    State(state): State<AssertingMockState>,
    method: Method,
    uri: Uri,
    Json(body): Json<Value>,
) -> Response {
    let request = CapturedMockRequest {
        method: method.to_string(),
        path: uri.path().to_string(),
        body,
    };
    state.captured.push(request.clone());

    if let Err(message) = (state.assertion)(&request) {
        return assertion_failure_response(message);
    }

    let resp = serde_json::json!({
        "id": "chatcmpl-asserting-mock",
        "object": "chat.completion",
        "created": 1,
        "model": request.body.get("model").unwrap_or(&serde_json::json!("mock")),
        "choices": [{ "index": 0, "message": { "role": "assistant", "content": "OK" }, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
    });
    (StatusCode::OK, Json(resp)).into_response()
}

/// Spawns an OpenAI Responses mock that captures and asserts request shape.
pub async fn spawn_asserting_openai_responses_mock(
    assertion: impl Fn(&CapturedMockRequest) -> Result<(), String> + Send + Sync + 'static,
) -> (String, tokio::task::JoinHandle<()>, CapturedMockRequests) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");

    let captured = CapturedMockRequests::default();
    let state = AssertingMockState {
        captured: captured.clone(),
        assertion: Arc::new(assertion),
    };

    let app = Router::new()
        .route("/v1/responses", post(asserting_openai_responses_handler))
        .route("/responses", post(asserting_openai_responses_handler));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app.with_state(state)).await.ok();
    });
    (base, handle, captured)
}

async fn asserting_openai_responses_handler(
    State(state): State<AssertingMockState>,
    method: Method,
    uri: Uri,
    Json(body): Json<Value>,
) -> Response {
    let request = CapturedMockRequest {
        method: method.to_string(),
        path: uri.path().to_string(),
        body,
    };
    state.captured.push(request.clone());

    if let Err(message) = (state.assertion)(&request) {
        return assertion_failure_response(message);
    }

    let resp = serde_json::json!({
        "id": "resp_asserting_mock",
        "object": "response",
        "created_at": 1,
        "status": "completed",
        "model": request.body.get("model").unwrap_or(&serde_json::json!("mock")),
        "output": [
            { "type": "message", "role": "assistant", "content": [{ "type": "output_text", "text": "OK" }] }
        ],
        "usage": { "input_tokens": 1, "output_tokens": 1, "total_tokens": 2 }
    });
    (StatusCode::OK, Json(resp)).into_response()
}

/// Spawns an Anthropic Messages mock that captures and asserts request shape.
///
/// Assertion failures are returned as 400 JSON so e2e tests see the mock-side
/// contract reason through the proxy response path.
pub async fn spawn_asserting_anthropic_mock(
    assertion: impl Fn(&CapturedMockRequest) -> Result<(), String> + Send + Sync + 'static,
) -> (String, tokio::task::JoinHandle<()>, CapturedMockRequests) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");

    let captured = CapturedMockRequests::default();
    let state = AssertingMockState {
        captured: captured.clone(),
        assertion: Arc::new(assertion),
    };

    let app = Router::new()
        .route("/v1/messages", post(asserting_anthropic_handler))
        .route("/messages", post(asserting_anthropic_handler));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app.with_state(state)).await.ok();
    });
    (base, handle, captured)
}

async fn asserting_anthropic_handler(
    State(state): State<AssertingMockState>,
    method: Method,
    uri: Uri,
    Json(body): Json<Value>,
) -> Response {
    let request = CapturedMockRequest {
        method: method.to_string(),
        path: uri.path().to_string(),
        body,
    };
    state.captured.push(request.clone());

    if let Err(message) = (state.assertion)(&request) {
        return assertion_failure_response(message);
    }

    let resp = serde_json::json!({
        "id": "msg_asserting_mock",
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "text", "text": "OK" }],
        "model": request.body.get("model").unwrap_or(&serde_json::json!("claude-3")),
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": { "input_tokens": 1, "output_tokens": 1 }
    });
    (StatusCode::OK, Json(resp)).into_response()
}

fn assertion_failure_response(message: String) -> Response {
    let resp = serde_json::json!({
        "error": {
            "type": "mock_assertion_failed",
            "message": message
        }
    });
    (StatusCode::BAD_REQUEST, Json(resp)).into_response()
}
