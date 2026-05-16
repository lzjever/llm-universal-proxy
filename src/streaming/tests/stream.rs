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
fn translate_sse_event_same_format_openai_rejects_reserved_tool_name() {
    let event = serde_json::json!({
        "id": "chatcmpl-reserved",
        "object": "chat.completion.chunk",
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
    });
    let mut state = StreamState::default();
    let out = translate_sse_event(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiCompletion,
        &event,
        &mut state,
    );
    let joined = out
        .iter()
        .map(|bytes| String::from_utf8_lossy(bytes).to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(joined.contains("\"code\":\"reserved_openai_custom_bridge_prefix\""));
    assert!(!joined.contains("\"name\":\"__llmup_custom__apply_patch\""));
    assert!(!joined.contains("__llmup_custom__"), "{joined}");
    assert!(
        state.fatal_rejection.is_some(),
        "reserved-prefix stream should be rejected"
    );
}

#[test]
fn translate_sse_event_same_format_responses_rejects_reserved_public_tool_identity() {
    let cases = [
        serde_json::json!({
            "type": "response.output_item.added",
            "item": {
                "type": "function_call",
                "call_id": "call_safe_name_reserved_namespace",
                "name": "lookup_weather",
                "namespace": "__llmup_custom__internal",
                "arguments": "{}"
            }
        }),
        serde_json::json!({
            "type": "response.output_item.done",
            "output": [{
                "type": "custom_tool_call",
                "call_id": "call_safe_name_reserved_namespace",
                "name": "code_exec",
                "namespace": "__llmup_custom__internal",
                "input": "print('hi')"
            }]
        }),
        serde_json::json!({
            "type": "response.completed",
            "response": {
                "id": "resp_output_namespace",
                "object": "response",
                "output": [{
                    "type": "function_call",
                    "call_id": "call_safe_name_reserved_namespace",
                    "name": "lookup_weather",
                    "namespace": "__llmup_custom__internal",
                    "arguments": "{}"
                }]
            }
        }),
        serde_json::json!({
            "type": "response.created",
            "response": {
                "id": "resp_tools_name",
                "object": "response",
                "output": [],
                "tools": [{
                    "type": "function",
                    "name": "__llmup_custom__lookup_weather"
                }]
            }
        }),
        serde_json::json!({
            "type": "response.created",
            "response": {
                "id": "resp_tool_choice_namespace",
                "object": "response",
                "output": [],
                "tool_choice": {
                    "type": "function",
                    "name": "lookup_weather",
                    "namespace": "__llmup_custom__internal"
                }
            }
        }),
    ];

    for event in cases {
        let mut state = StreamState::default();
        let out = translate_sse_event(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiResponses,
            &event,
            &mut state,
        );
        let joined = out
            .iter()
            .map(|bytes| String::from_utf8_lossy(bytes).to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            joined.contains("\"code\":\"reserved_openai_custom_bridge_prefix\""),
            "event = {event}, out = {joined}"
        );
        assert!(
            !joined.contains("\"namespace\":\"__llmup_custom__internal\"")
                && !joined.contains("\"name\":\"__llmup_custom__lookup_weather\""),
            "reserved tool artifact should not leak: {joined}"
        );
        assert!(
            state.fatal_rejection.is_some(),
            "reserved-prefix Responses stream should be rejected"
        );
    }
}

#[tokio::test]
async fn guarded_sse_stream_responses_passthrough_rejects_reserved_top_level_call_event_identity() {
    let event_types = [
        (
            "response.function_call_arguments.delta",
            "delta",
            "{\"city\"",
            "lookup_weather",
        ),
        (
            "response.function_call_arguments.done",
            "arguments",
            "{}",
            "lookup_weather",
        ),
        (
            "response.custom_tool_call_input.delta",
            "delta",
            "print(",
            "run_code",
        ),
        (
            "response.custom_tool_call_input.done",
            "input",
            "print('hi')",
            "run_code",
        ),
    ];

    for (event_type, payload_field, payload_value, safe_name) in event_types {
        for (identity_field, reserved_value, leaked_fragment) in [
            (
                "name",
                "__llmup_custom__reserved_tool",
                "\"name\":\"__llmup_custom__reserved_tool\"",
            ),
            (
                "namespace",
                "__llmup_custom__internal",
                "\"namespace\":\"__llmup_custom__internal\"",
            ),
        ] {
            let mut event = serde_json::json!({
                "type": event_type,
                "sequence_number": 1,
                "response_id": "resp_reserved_top_level_identity",
                "item_id": "fc_call_reserved",
                "output_index": 0,
                "call_id": "call_reserved",
                "name": safe_name
            });
            event[payload_field] = Value::String(payload_value.to_string());
            event[identity_field] = Value::String(reserved_value.to_string());

            let inner = futures_util::stream::iter(vec![Ok::<Bytes, std::io::Error>(Bytes::from(
                format_sse_event(event_type, &event),
            ))]);
            let mut stream = GuardedSseStream::new(inner, UpstreamFormat::OpenAiResponses);
            let mut frames = Vec::new();
            while let Some(frame) = stream.next().await {
                let frame = frame.expect("guarded frame");
                frames.push(String::from_utf8(frame.to_vec()).expect("utf8 frame"));
            }
            let joined = frames.join("\n");

            assert!(
                joined.contains("\"code\":\"reserved_openai_custom_bridge_prefix\""),
                "{event_type} {identity_field} should fail closed, body = {joined}"
            );
            assert!(
                !joined.contains(leaked_fragment),
                "{event_type} {identity_field} leaked reserved tool identity: {joined}"
            );
        }
    }
}

#[tokio::test]
async fn guarded_sse_stream_parse_failed_raw_artifact_frame_fails_closed() {
    let inner = futures_util::stream::iter(vec![Ok::<Bytes, std::io::Error>(Bytes::from_static(
        b": heartbeat __llmup_custom__secret _llmup_tool_bridge_context\n\n",
    ))]);
    let mut stream = GuardedSseStream::new(inner, UpstreamFormat::OpenAiResponses);
    let mut frames = Vec::new();
    while let Some(frame) = stream.next().await {
        let frame = frame.expect("guarded frame");
        frames.push(String::from_utf8(frame.to_vec()).expect("utf8 frame"));
    }
    let joined = frames.join("\n");

    assert!(joined.contains("response.failed"), "{joined}");
    assert!(!joined.contains("__llmup_custom__"), "{joined}");
    assert!(!joined.contains("_llmup_tool_bridge_context"), "{joined}");
    assert!(!joined.contains("secret"), "{joined}");
}

#[tokio::test]
async fn guarded_sse_stream_sanitizes_parsed_error_message_artifacts() {
    let event = serde_json::json!({
        "id": "chatcmpl_error",
        "object": "chat.completion.chunk",
        "error": {
            "type": "server_error",
            "message": "provider leaked __llmup_custom__secret and _llmup_tool_bridge_context"
        }
    });
    let inner = futures_util::stream::iter(vec![Ok::<Bytes, std::io::Error>(Bytes::from(
        format_sse_data(&event),
    ))]);
    let mut stream = GuardedSseStream::new(inner, UpstreamFormat::OpenAiCompletion);
    let mut frames = Vec::new();
    while let Some(frame) = stream.next().await {
        let frame = frame.expect("guarded frame");
        frames.push(String::from_utf8(frame.to_vec()).expect("utf8 frame"));
    }
    let joined = frames.join("\n");

    assert!(joined.contains(crate::internal_artifacts::GENERIC_UPSTREAM_ERROR_MESSAGE));
    assert!(!joined.contains("__llmup_custom__"), "{joined}");
    assert!(!joined.contains("_llmup_tool_bridge_context"), "{joined}");
    assert!(!joined.contains("secret"), "{joined}");
}

#[tokio::test]
async fn guarded_sse_stream_canonicalizes_parsed_duplicate_key_frame() {
    let inner = futures_util::stream::iter(vec![Ok::<Bytes, std::io::Error>(Bytes::from_static(
        br#"event: response.completed
data: {"type":"response.completed","response":{"id":"resp_duplicate","object":"response","output":[],"metadata":{"note":"__llmup_custom__secret"},"metadata":{}}}

"#,
    ))]);
    let mut stream = GuardedSseStream::new(inner, UpstreamFormat::OpenAiResponses);
    let mut frames = Vec::new();
    while let Some(frame) = stream.next().await {
        let frame = frame.expect("guarded frame");
        frames.push(String::from_utf8(frame.to_vec()).expect("utf8 frame"));
    }
    let joined = frames.join("\n");

    assert!(joined.contains("event: response.completed"), "{joined}");
    assert!(joined.contains("\"metadata\":{}"), "{joined}");
    assert!(!joined.contains("__llmup_custom__"), "{joined}");
    assert!(!joined.contains("secret"), "{joined}");
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

fn translated_sse_matrix_formats() -> [UpstreamFormat; 3] {
    [
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
    ]
}

fn translated_sse_matrix_valid_frame(upstream_format: UpstreamFormat, sentinel: &str) -> Vec<u8> {
    match upstream_format {
        UpstreamFormat::OpenAiCompletion => format_sse_data(&serde_json::json!({
            "id": "chatcmpl-translated-guard",
            "model": "gpt-test",
            "choices": [{
                "index": 0,
                "delta": {
                    "role": "assistant",
                    "content": sentinel
                },
                "finish_reason": null
            }]
        })),
        UpstreamFormat::Anthropic => format_sse_event(
            "content_block_delta",
            &serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {
                    "type": "text_delta",
                    "text": sentinel
                }
            }),
        ),
        UpstreamFormat::OpenAiResponses => format_sse_event(
            "response.output_text.delta",
            &serde_json::json!({
                "type": "response.output_text.delta",
                "sequence_number": 1,
                "response_id": "resp_translated_guard",
                "output_index": 0,
                "content_index": 0,
                "delta": sentinel
            }),
        ),
    }
}

async fn collect_translated_sse_strings(
    upstream_format: UpstreamFormat,
    client_format: UpstreamFormat,
    chunks: Vec<Vec<u8>>,
) -> Vec<String> {
    let inner = futures_util::stream::iter(
        chunks
            .into_iter()
            .map(|chunk| Ok::<Bytes, std::io::Error>(Bytes::from(chunk))),
    );
    let mut stream = TranslateSseStream::new(inner, upstream_format, client_format);
    let mut frames = Vec::new();
    while let Some(frame) = stream.next().await {
        let frame = frame.expect("translated frame");
        frames.push(String::from_utf8(frame.to_vec()).expect("utf8 frame"));
    }
    frames
}

fn oversized_unterminated_sse_frame() -> Vec<u8> {
    vec![b'x'; crate::streaming::stream::DEFAULT_MAX_SSE_FRAME_BYTES + 1]
}

#[tokio::test]
async fn guarded_sse_stream_rejects_oversized_unterminated_frame_and_stops() {
    const SENTINEL: &str = "SAFE_AFTER_OVERSIZED_GUARDED_FRAME";

    let inner = futures_util::stream::iter(vec![
        Ok::<Bytes, std::io::Error>(Bytes::from(oversized_unterminated_sse_frame())),
        Ok::<Bytes, std::io::Error>(Bytes::from(format_sse_event(
            "response.output_text.delta",
            &serde_json::json!({
                "type": "response.output_text.delta",
                "sequence_number": 1,
                "response_id": "resp_after_oversized_guarded",
                "output_index": 0,
                "content_index": 0,
                "delta": SENTINEL
            }),
        ))),
    ]);
    let mut stream = GuardedSseStream::new(inner, UpstreamFormat::OpenAiResponses);
    let mut frames = Vec::new();
    while let Some(frame) = stream.next().await {
        let frame = frame.expect("guarded frame");
        frames.push(String::from_utf8(frame.to_vec()).expect("utf8 frame"));
    }
    let joined = frames.join("\n");

    assert!(joined.contains("\"type\":\"response.failed\""), "{joined}");
    assert!(
        joined.contains("\"type\":\"invalid_request_error\""),
        "{joined}"
    );
    assert!(
        joined.contains("\"code\":\"upstream_sse_frame_too_large\""),
        "{joined}"
    );
    assert!(!joined.contains("\"type\":\"server_error\""), "{joined}");
    assert!(!joined.contains(SENTINEL), "{joined}");
}

#[tokio::test]
async fn translated_sse_stream_rejects_oversized_unterminated_frame_and_stops() {
    const SENTINEL: &str = "SAFE_AFTER_OVERSIZED_TRANSLATED_FRAME";

    let frames = collect_translated_sse_strings(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        vec![
            oversized_unterminated_sse_frame(),
            translated_sse_matrix_valid_frame(UpstreamFormat::OpenAiCompletion, SENTINEL),
        ],
    )
    .await;
    let joined = frames.join("\n");

    assert!(joined.contains("\"type\":\"response.failed\""), "{joined}");
    assert!(
        joined.contains("\"type\":\"invalid_request_error\""),
        "{joined}"
    );
    assert!(
        joined.contains("\"code\":\"upstream_sse_frame_too_large\""),
        "{joined}"
    );
    assert!(!joined.contains("\"type\":\"server_error\""), "{joined}");
    assert!(!joined.contains(SENTINEL), "{joined}");
}

#[tokio::test]
async fn translated_sse_stream_uses_configured_sse_frame_limit() {
    const SENTINEL: &str = "SAFE_AFTER_CONFIGURED_SMALL_FRAME_LIMIT";

    let configured_oversized_frame = vec![b'x'; 65];
    let inner = futures_util::stream::iter(vec![
        Ok::<Bytes, std::io::Error>(Bytes::from(configured_oversized_frame)),
        Ok::<Bytes, std::io::Error>(Bytes::from(translated_sse_matrix_valid_frame(
            UpstreamFormat::OpenAiCompletion,
            SENTINEL,
        ))),
    ]);
    let mut stream = TranslateSseStream::new(
        inner,
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
    )
    .with_resource_limits(crate::config::ResourceLimits {
        max_sse_frame_bytes: 64,
        ..Default::default()
    });
    let mut frames = Vec::new();
    while let Some(frame) = stream.next().await {
        let frame = frame.expect("translated frame");
        frames.push(String::from_utf8(frame.to_vec()).expect("utf8 frame"));
    }
    let joined = frames.join("\n");

    assert!(
        joined.contains("\"code\":\"upstream_sse_frame_too_large\""),
        "{joined}"
    );
    assert!(!joined.contains(SENTINEL), "{joined}");
}

#[tokio::test]
async fn guarded_sse_stream_stops_after_configured_max_events() {
    const FIRST: &str = "SAFE_FIRST_EVENT_BEFORE_EVENT_LIMIT";
    const SECOND: &str = "SAFE_SECOND_EVENT_AFTER_EVENT_LIMIT";

    let first = serde_json::json!({
        "type": "response.output_text.delta",
        "sequence_number": 1,
        "response_id": "resp_event_limit",
        "output_index": 0,
        "content_index": 0,
        "delta": FIRST
    });
    let second = serde_json::json!({
        "type": "response.output_text.delta",
        "sequence_number": 2,
        "response_id": "resp_event_limit",
        "output_index": 0,
        "content_index": 0,
        "delta": SECOND
    });
    let inner = futures_util::stream::iter(vec![Ok::<Bytes, std::io::Error>(Bytes::from(
        [
            format_sse_event("response.output_text.delta", &first),
            format_sse_event("response.output_text.delta", &second),
        ]
        .concat(),
    ))]);
    let mut stream = GuardedSseStream::new(inner, UpstreamFormat::OpenAiResponses)
        .with_resource_limits(crate::config::ResourceLimits {
            stream_max_events: 1,
            ..Default::default()
        });

    let mut frames = Vec::new();
    while let Some(frame) = stream.next().await {
        let frame = frame.expect("guarded frame");
        frames.push(String::from_utf8(frame.to_vec()).expect("utf8 frame"));
    }
    let joined = frames.join("\n");

    assert!(joined.contains(FIRST), "{joined}");
    assert!(
        joined.contains("\"code\":\"upstream_stream_event_limit_exceeded\""),
        "{joined}"
    );
    assert!(!joined.contains(SECOND), "{joined}");
}

#[tokio::test]
async fn guarded_sse_stream_emits_configured_idle_timeout_error() {
    let inner = futures_util::stream::pending::<Result<Bytes, std::io::Error>>();
    let mut stream = GuardedSseStream::new(inner, UpstreamFormat::OpenAiResponses)
        .with_resource_limits(crate::config::ResourceLimits {
            stream_idle_timeout_secs: 1,
            stream_max_duration_secs: 60,
            ..Default::default()
        });

    let frame = timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("idle timeout should fire")
        .expect("idle timeout should emit one frame")
        .expect("idle timeout frame");
    let frame = String::from_utf8(frame.to_vec()).expect("utf8 frame");

    assert!(
        frame.contains("\"code\":\"upstream_stream_idle_timeout\""),
        "{frame}"
    );
    let end = timeout(Duration::from_millis(50), stream.next())
        .await
        .expect("stream should close after idle timeout");
    assert!(end.is_none(), "next = {end:?}");
}

#[tokio::test]
async fn guarded_sse_stream_emits_configured_max_duration_error() {
    let inner = futures_util::stream::pending::<Result<Bytes, std::io::Error>>();
    let mut stream = GuardedSseStream::new(inner, UpstreamFormat::OpenAiResponses)
        .with_resource_limits(crate::config::ResourceLimits {
            stream_idle_timeout_secs: 60,
            stream_max_duration_secs: 1,
            ..Default::default()
        });

    let frame = timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("max duration should fire")
        .expect("max duration should emit one frame")
        .expect("max duration frame");
    let frame = String::from_utf8(frame.to_vec()).expect("utf8 frame");

    assert!(
        frame.contains("\"code\":\"upstream_stream_max_duration_exceeded\""),
        "{frame}"
    );
    let end = timeout(Duration::from_millis(50), stream.next())
        .await
        .expect("stream should close after max duration");
    assert!(end.is_none(), "next = {end:?}");
}

#[tokio::test]
async fn translated_sse_stream_fails_closed_on_accumulated_state_cap() {
    const SENTINEL: &str = "STREAM_STATE_SENTINEL_SHOULD_NOT_LEAK";

    let event = serde_json::json!({
        "id": "chatcmpl-state-limit",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "delta": { "content": format!("{SENTINEL}{}", "x".repeat(256)) },
            "finish_reason": null
        }]
    });
    let inner = futures_util::stream::iter(vec![Ok::<Bytes, std::io::Error>(Bytes::from(
        format_sse_data(&event),
    ))]);
    let mut stream = TranslateSseStream::new(
        inner,
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
    )
    .with_resource_limits(crate::config::ResourceLimits {
        max_accumulated_stream_state_bytes: 64,
        ..Default::default()
    });

    let mut frames = Vec::new();
    while let Some(frame) = stream.next().await {
        let frame = frame.expect("translated frame");
        frames.push(String::from_utf8(frame.to_vec()).expect("utf8 frame"));
    }
    let joined = frames.join("\n");

    assert!(
        joined.contains("\"code\":\"upstream_stream_state_too_large\""),
        "{joined}"
    );
    assert!(!joined.contains(SENTINEL), "{joined}");
}

#[tokio::test]
async fn translated_sse_stream_malformed_raw_artifact_frames_fail_closed_for_all_cross_format_pairs(
) {
    const SENTINEL: &str = "SAFE_AFTER_MALFORMED_RAW_ARTIFACT_FRAME";

    for upstream_format in translated_sse_matrix_formats() {
        for client_format in translated_sse_matrix_formats() {
            if upstream_format == client_format {
                continue;
            }

            let mut chunk =
                b"data: not-json __llmup_custom__secret _llmup_tool_bridge_context\n\n".to_vec();
            chunk.extend(translated_sse_matrix_valid_frame(upstream_format, SENTINEL));

            let frames =
                collect_translated_sse_strings(upstream_format, client_format, vec![chunk]).await;
            let joined = frames.join("\n");

            assert!(
                !joined.contains("__llmup_custom__"),
                "{upstream_format}->{client_format} leaked custom bridge artifact: {joined}"
            );
            assert!(
                !joined.contains("_llmup_tool_bridge_context"),
                "{upstream_format}->{client_format} leaked bridge context artifact: {joined}"
            );
            assert!(
                !joined.contains("secret"),
                "{upstream_format}->{client_format} leaked raw artifact payload: {joined}"
            );
            assert!(
                !joined.contains(SENTINEL),
                "{upstream_format}->{client_format} processed the valid frame after fatal raw artifact rejection: {joined}"
            );
        }
    }
}

#[tokio::test]
async fn translated_sse_stream_artifact_event_types_fail_closed_for_all_cross_format_pairs() {
    const SENTINEL: &str = "SAFE_AFTER_ARTIFACT_EVENT_TYPE";

    for upstream_format in translated_sse_matrix_formats() {
        for client_format in translated_sse_matrix_formats() {
            if upstream_format == client_format {
                continue;
            }

            let mut chunk = b"event: __llmup_custom__internal_event\n".to_vec();
            let valid_frame = translated_sse_matrix_valid_frame(upstream_format, SENTINEL);
            let data_start = valid_frame
                .windows(5)
                .position(|window| window == b"data:")
                .expect("valid frame contains data line");
            chunk.extend_from_slice(&valid_frame[data_start..]);

            let frames =
                collect_translated_sse_strings(upstream_format, client_format, vec![chunk]).await;
            let joined = frames.join("\n");

            assert!(
                !joined.contains("__llmup_custom__"),
                "{upstream_format}->{client_format} leaked artifact event type: {joined}"
            );
            assert!(
                !joined.contains(SENTINEL),
                "{upstream_format}->{client_format} translated data from artifact event type: {joined}"
            );
        }
    }
}

#[tokio::test]
async fn translated_sse_stream_ignores_clean_unparsed_frames_before_valid_events() {
    const SENTINEL: &str = "SAFE_AFTER_CLEAN_UNPARSED_FRAMES";

    for upstream_format in translated_sse_matrix_formats() {
        for client_format in translated_sse_matrix_formats() {
            if upstream_format == client_format {
                continue;
            }

            let mut chunk = b": clean heartbeat\n\nevent: ping\n\ndata:\n\n".to_vec();
            chunk.extend(translated_sse_matrix_valid_frame(upstream_format, SENTINEL));

            let frames =
                collect_translated_sse_strings(upstream_format, client_format, vec![chunk]).await;
            let joined = frames.join("\n");

            assert!(
                joined.contains(SENTINEL),
                "{upstream_format}->{client_format} should translate the valid frame after clean unparsed frames: {joined}"
            );
        }
    }
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
