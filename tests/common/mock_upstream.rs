//! Mock upstream servers that speak each protocol per official API specs.
//! Used by integration tests to validate proxy passthrough and translation.

use axum::{
    body::Body,
    extract::{Json, Path},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use serde_json::Value;
use tokio::net::TcpListener;

/// Spawns a mock upstream that speaks OpenAI Chat Completions API.
/// Returns (base_url, _handle). Server responds at POST /chat/completions.
/// Non-streaming: returns full ChatCompletion JSON. Streaming: returns SSE chunks then [DONE].
pub async fn spawn_openai_completion_mock() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{}", port);

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
                r#"data: {{"id":"chatcmpl-{}","object":"chat.completion.chunk","created":1,"model":"{}","choices":[{{"index":0,"delta":{{"role":"assistant"}},"finish_reason":null}}]}}"#,
                id, id
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
    let base = format!("http://127.0.0.1:{}", port);

    let app = Router::new()
        .route("/v1/messages", post(anthropic_handler))
        .route("/messages", post(anthropic_handler));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle)
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

/// Spawns a mock that speaks Google Gemini generateContent API.
/// Responds at POST /models/{model}:generateContent (per official API). Also accepts /generateContent for backward compat.
pub async fn spawn_google_mock() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{}", port);

    let app = Router::new()
        .route("/v1beta/models/:model_action", post(google_handler))
        .route("/models/:model_action", post(google_handler))
        .route("/generateContent", post(google_handler));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle)
}

async fn google_handler(path: Option<Path<String>>, Json(body): Json<Value>) -> Response {
    let stream = path
        .as_ref()
        .map(|Path(model_action)| model_action.contains(":streamGenerateContent"))
        .unwrap_or(false)
        || body
            .get("generationConfig")
            .and_then(|g| g.get("stream"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
    if stream {
        let chunks = [
            r#"data: {"candidates":[{"content":{"parts":[{"text":"Hi"}],"role":"model"},"finishReason":"STOP"}],"modelVersion":"gemini-mock"}"#,
        ];
        let body = chunks.join("\n\n") + "\n\n";
        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .body(Body::from(body))
            .unwrap()
    } else {
        let resp = serde_json::json!({
            "candidates": [{ "content": { "parts": [{ "text": "Hi" }], "role": "model" }, "finishReason": "STOP" }],
            "usageMetadata": { "promptTokenCount": 1, "candidatesTokenCount": 1, "totalTokenCount": 2 }
        });
        (StatusCode::OK, Json(resp)).into_response()
    }
}

/// Spawns a mock that speaks OpenAI Responses API.
/// Responds at POST /responses. Non-streaming: output array; streaming: response.created, response.output_text.delta, response.completed.
pub async fn spawn_openai_responses_mock() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{}", port);

    let app = Router::new()
        .route("/v1/responses", post(openai_responses_handler))
        .route("/responses", post(openai_responses_handler));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle)
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
