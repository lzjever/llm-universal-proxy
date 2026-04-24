use super::*;
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::task::{Context, Poll};
use tokio::time::{timeout, Duration};

#[test]
fn translate_sse_event_passthrough_openai_sends_done() {
    let event = serde_json::json!({ "_done": true });
    let mut state = StreamState::default();
    let out = translate_sse_event(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiCompletion,
        &event,
        &mut state,
    );
    assert_eq!(out.len(), 1);
    assert!(out[0].starts_with(b"data: [DONE]"));
}

#[test]
fn translate_sse_event_openai_to_non_openai_single_frame_multi_choice_fails_closed() {
    for client_format in [UpstreamFormat::Google, UpstreamFormat::OpenAiResponses] {
        let mut state = StreamState::default();
        let out = translate_sse_event(
            UpstreamFormat::OpenAiCompletion,
            client_format,
            &serde_json::json!({
                "id": "chatcmpl-msg123",
                "model": "gpt-4o",
                "choices": [
                    {
                        "index": 0,
                        "delta": { "content": "candidate-0" },
                        "finish_reason": null
                    },
                    {
                        "index": 1,
                        "delta": { "content": "candidate-1" },
                        "finish_reason": null
                    }
                ]
            }),
            &mut state,
        );

        let joined = out
            .iter()
            .map(|bytes| String::from_utf8_lossy(bytes).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            state.fatal_rejection.is_some(),
            "client_format = {client_format:?}, out = {joined}"
        );
        if client_format == UpstreamFormat::Google {
            assert!(
                out.is_empty(),
                "Gemini sink must not emit ad-hoc error frames"
            );
        } else {
            assert!(
                !out.is_empty(),
                "Responses sink should surface incompatibility as response.failed"
            );
            assert!(
                !joined.contains("candidate-0") && !joined.contains("candidate-1"),
                "fatal reject must not leak mixed-choice content for {client_format:?}: {joined}"
            );
        }
    }
}

#[test]
fn translate_sse_event_openai_to_non_openai_cross_frame_multi_choice_rejects_and_suppresses() {
    for client_format in [UpstreamFormat::Google, UpstreamFormat::OpenAiResponses] {
        let mut state = StreamState::default();
        let first = translate_sse_event(
            UpstreamFormat::OpenAiCompletion,
            client_format,
            &serde_json::json!({
                "id": "chatcmpl-msg123",
                "model": "gpt-4o",
                "choices": [{
                    "index": 0,
                    "delta": { "content": "candidate-0" },
                    "finish_reason": null
                }]
            }),
            &mut state,
        );
        assert!(
            state.fatal_rejection.is_none(),
            "single-choice frame should not be rejected early for {client_format:?}: {first:?}"
        );
        let second = translate_sse_event(
            UpstreamFormat::OpenAiCompletion,
            client_format,
            &serde_json::json!({
                "id": "chatcmpl-msg123",
                "model": "gpt-4o",
                "choices": [{
                    "index": 1,
                    "delta": { "content": "candidate-1" },
                    "finish_reason": null
                }]
            }),
            &mut state,
        );
        let third = translate_sse_event(
            UpstreamFormat::OpenAiCompletion,
            client_format,
            &serde_json::json!({
                "id": "chatcmpl-msg123",
                "model": "gpt-4o",
                "choices": [{
                    "index": 0,
                    "delta": { "content": "after-reject" },
                    "finish_reason": null
                }]
            }),
            &mut state,
        );

        let second_joined = second
            .iter()
            .map(|bytes| String::from_utf8_lossy(bytes).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            state.fatal_rejection.is_some(),
            "second frame should fatal reject mixed choice stream for {client_format:?}: {second_joined}"
        );
        if client_format == UpstreamFormat::Google {
            assert!(
                second.is_empty(),
                "Gemini sink must not emit ad-hoc error frames for fatal rejection"
            );
        } else {
            assert!(
                !second.is_empty(),
                "Responses sink should surface incompatibility as response.failed"
            );
            assert!(
                !second_joined.contains("candidate-1"),
                "fatal reject must not leak mixed-choice content for {client_format:?}: {second_joined}"
            );
        }
        assert!(
            third.is_empty(),
            "follow-up after fatal reject should be suppressed for {client_format:?}"
        );
    }
}

#[test]
fn stream_usage_detail_objects_match_non_stream_translation() {
    let responses_usage = serde_json::json!({
        "input_tokens": 11,
        "output_tokens": 7,
        "total_tokens": 18,
        "input_tokens_details": {
            "cached_tokens": 3,
            "audio_tokens": 2,
            "future_prompt_detail": 4
        },
        "output_tokens_details": {
            "reasoning_tokens": 1,
            "audio_tokens": 5,
            "accepted_prediction_tokens": 6,
            "rejected_prediction_tokens": 2,
            "future_completion_detail": 8
        }
    });
    let openai_usage = serde_json::json!({
        "prompt_tokens": 11,
        "completion_tokens": 7,
        "total_tokens": 18,
        "prompt_tokens_details": {
            "cached_tokens": 3,
            "audio_tokens": 2,
            "future_prompt_detail": 4
        },
        "completion_tokens_details": {
            "reasoning_tokens": 1,
            "audio_tokens": 5,
            "accepted_prediction_tokens": 6,
            "rejected_prediction_tokens": 2,
            "future_completion_detail": 8
        }
    });

    let streamed_openai_usage = responses_usage_to_openai_usage_stream(&responses_usage);
    let non_stream_openai = translate_response(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        &serde_json::json!({
            "id": "resp_usage",
            "object": "response",
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": "Hi" }]
            }],
            "usage": responses_usage
        }),
    )
    .expect("non-stream responses -> openai");
    assert_eq!(
        streamed_openai_usage["prompt_tokens_details"],
        non_stream_openai["usage"]["prompt_tokens_details"]
    );
    assert_eq!(
        streamed_openai_usage["completion_tokens_details"],
        non_stream_openai["usage"]["completion_tokens_details"]
    );

    let streamed_responses_usage = openai_usage_to_responses_usage_stream(&openai_usage);
    let non_stream_responses = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        &serde_json::json!({
            "id": "chatcmpl_usage",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "Hi" },
                "finish_reason": "stop"
            }],
            "usage": openai_usage
        }),
    )
    .expect("non-stream openai -> responses");
    assert_eq!(
        streamed_responses_usage["input_tokens_details"],
        non_stream_responses["usage"]["input_tokens_details"]
    );
    assert_eq!(
        streamed_responses_usage["output_tokens_details"],
        non_stream_responses["usage"]["output_tokens_details"]
    );
}

#[test]
fn openai_chunk_does_not_double_prefix_existing_chatcmpl_ids() {
    let state = StreamState {
        message_id: Some("chatcmpl-msg123".to_string()),
        ..Default::default()
    };
    let chunk = openai_chunk(&state, serde_json::json!({"content":"Hi"}), None);
    assert_eq!(chunk["id"], "chatcmpl-msg123");
}

fn parse_sse_json_frame(bytes: &[u8]) -> Value {
    let mut buf = bytes.to_vec();
    take_one_sse_event(&mut buf).expect("parse sse event")
}

struct PendingAfterFirstChunkStream {
    polls: Arc<AtomicUsize>,
    emitted: bool,
}

impl Stream for PendingAfterFirstChunkStream {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.polls.fetch_add(1, Ordering::SeqCst);
        if !self.emitted {
            self.emitted = true;
            return Poll::Ready(Some(Ok(Bytes::from_static(
                br#"data: {"id":"chatcmpl-msg123","model":"gpt-4o","choices":[{"index":0,"delta":{"content":"candidate-0"},"finish_reason":null},{"index":1,"delta":{"content":"candidate-1"},"finish_reason":null}]}

"#,
            ))));
        }
        Poll::Pending
    }
}

#[tokio::test]
async fn translate_sse_stream_closes_promptly_after_fatal_rejection() {
    let polls = Arc::new(AtomicUsize::new(0));
    let inner = PendingAfterFirstChunkStream {
        polls: polls.clone(),
        emitted: false,
    };
    let mut stream = TranslateSseStream::new(
        inner,
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
    );

    let mut frames = Vec::new();
    loop {
        let next = timeout(Duration::from_millis(50), stream.next())
            .await
            .expect("stream should not hang after fatal rejection");
        match next {
            Some(Ok(bytes)) => frames.push(String::from_utf8(bytes.to_vec()).expect("utf8 frame")),
            Some(Err(err)) => panic!("unexpected stream error: {err}"),
            None => break,
        }
    }

    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("\"type\":\"response.failed\"")),
        "frames = {frames:?}"
    );
    assert_eq!(
        polls.load(Ordering::SeqCst),
        1,
        "upstream should not be polled again after fatal rejection"
    );
}

#[tokio::test]
async fn translate_sse_stream_gemini_to_anthropic_defers_finish_until_stream_end() {
    let inner = futures_util::stream::iter(vec![
        Ok::<Bytes, std::io::Error>(Bytes::from_static(
            br#"data: {"candidates":[{"content":{"parts":[{"text":"think","thought":true}],"role":"model"},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":1,"candidatesTokenCount":1,"thoughtsTokenCount":1,"totalTokenCount":3}}

"#,
        )),
        Ok::<Bytes, std::io::Error>(Bytes::from_static(
            br#"data: {"candidates":[{"content":{"parts":[{"text":"Hi"}],"role":"model"},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":1,"candidatesTokenCount":2,"thoughtsTokenCount":1,"totalTokenCount":4}}

"#,
        )),
    ]);
    let mut stream =
        TranslateSseStream::new(inner, UpstreamFormat::Google, UpstreamFormat::Anthropic);

    let mut events = Vec::new();
    while let Some(frame) = stream.next().await {
        let frame = frame.expect("translated frame");
        events.push(parse_sse_json_frame(&frame));
    }

    assert_eq!(events[0]["type"], "message_start");
    assert_eq!(events[1]["type"], "content_block_start");
    assert_eq!(events[1]["content_block"]["type"], "thinking");
    assert_eq!(events[2]["type"], "content_block_delta");
    assert_eq!(events[2]["delta"]["thinking"], "think");
    assert_eq!(events[3]["type"], "content_block_stop");
    assert_eq!(events[4]["type"], "content_block_start");
    assert_eq!(events[4]["content_block"]["type"], "text");
    assert_eq!(events[5]["type"], "content_block_delta");
    assert_eq!(events[5]["delta"]["text"], "Hi");
    assert_eq!(events[6]["type"], "content_block_stop");
    assert_eq!(events[7]["type"], "message_delta");
    assert_eq!(events[7]["delta"]["stop_reason"], "end_turn");
    assert_eq!(events[8]["type"], "message_stop");
    assert_eq!(
        events
            .iter()
            .filter(|event| event["type"] == "message_stop")
            .count(),
        1
    );
}

#[tokio::test]
async fn translate_sse_stream_gemini_to_responses_bridges_custom_tool_calls_without_prefix_leak() {
    let inner = futures_util::stream::iter(vec![
        Ok::<Bytes, std::io::Error>(Bytes::from_static(
            br#"data: {"responseId":"resp_gemini_custom","modelVersion":"gemini-2.5-flash","candidates":[{"content":{"role":"model","parts":[{"functionCall":{"id":"call_apply_patch","name":"apply_patch","args":{"input":"*** Begin Patch\n*** Add File: hello.txt\n+hello\n*** End Patch\n"}}}]},"finishReason":"STOP"}]}

"#,
        )),
        Ok::<Bytes, std::io::Error>(Bytes::from_static(
            br#"data: {"responseId":"resp_gemini_custom","modelVersion":"gemini-2.5-flash","candidates":[{"content":{"role":"model","parts":[]},"finishReason":"STOP"}]}

"#,
        )),
    ]);
    let mut stream = TranslateSseStream::new(
        inner,
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiResponses,
    )
    .with_request_scoped_tool_bridge_context(Some(typed_tool_bridge_context(
        "apply_patch",
        "custom_grammar",
        "max_compat",
    )));

    let mut frames = Vec::new();
    while let Some(frame) = stream.next().await {
        let frame = frame.expect("translated frame");
        frames.push(String::from_utf8(frame.to_vec()).expect("utf8 frame"));
    }
    let joined = frames.join("\n");

    assert!(
        joined.contains("response.custom_tool_call_input.delta"),
        "{joined}"
    );
    assert!(
        joined.contains("response.custom_tool_call_input.done"),
        "{joined}"
    );
    assert!(joined.contains("\"type\":\"custom_tool_call\""), "{joined}");
    assert!(joined.contains("\"name\":\"apply_patch\""), "{joined}");
    assert!(
        !joined.contains("response.function_call_arguments.delta"),
        "{joined}"
    );
    assert!(!joined.contains("\"type\":\"function_call\""), "{joined}");
    assert!(!joined.contains("__llmup_custom__"), "{joined}");
}

#[test]
fn translate_sse_event_anthropic_to_responses_preserves_commentary_in_completed_output_at_tool_boundary(
) {
    let mut state = StreamState::default();
    let mut out = Vec::new();
    for event in [
        serde_json::json!({
            "type": "message_start",
            "message": { "id": "msg_1", "model": "claude-test" }
        }),
        serde_json::json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": { "type": "text", "text": "" }
        }),
        serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "text_delta", "text": "I will inspect the file first." }
        }),
        serde_json::json!({ "type": "content_block_stop", "index": 0 }),
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
            "type": "message_delta",
            "delta": { "stop_reason": "tool_use" },
            "usage": { "input_tokens": 10, "output_tokens": 5 }
        }),
        serde_json::json!({ "type": "message_stop" }),
    ] {
        out.extend(translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiResponses,
            &event,
            &mut state,
        ));
    }

    let events = out
        .iter()
        .map(|bytes| parse_sse_json_frame(bytes))
        .collect::<Vec<_>>();
    let commentary_done_index = events
        .iter()
        .position(|event| {
            event.get("type").and_then(Value::as_str) == Some("response.output_item.done")
                && event["item"]["type"] == "message"
        })
        .expect("commentary done event");
    let tool_added_index = events
        .iter()
        .position(|event| {
            event.get("type").and_then(Value::as_str) == Some("response.output_item.added")
                && event["item"]["type"] == "function_call"
        })
        .expect("tool call added event");
    assert!(
        commentary_done_index < tool_added_index,
        "events = {events:?}"
    );
    assert_eq!(events[commentary_done_index]["item"]["phase"], "commentary");

    let terminal = events
        .iter()
        .find(|event| event.get("type").and_then(Value::as_str) == Some("response.completed"))
        .expect("response.completed event");
    let output = terminal["response"]["output"]
        .as_array()
        .expect("response output");
    assert_eq!(output.len(), 2, "output = {output:?}");
    assert_eq!(output[0]["type"], "message");
    assert_eq!(output[0]["phase"], "commentary");
    assert_eq!(
        output[0]["content"][0]["text"],
        "I will inspect the file first."
    );
    assert_eq!(output[1]["type"], "function_call");
    assert_eq!(output[1]["call_id"], "call_1");
    assert!(
        output
            .iter()
            .all(|item| item.get("phase").and_then(Value::as_str) != Some("final_answer")),
        "tool-boundary terminal should not mislabel commentary as final_answer: {output:?}"
    );
}
