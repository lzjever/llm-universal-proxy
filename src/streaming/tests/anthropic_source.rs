use super::*;

#[test]
fn claude_message_start_produces_openai_chunk() {
    let event = serde_json::json!({
        "type": "message_start",
        "message": { "id": "msg_1", "model": "claude-3" }
    });
    let mut state = StreamState::default();
    let chunks = claude_event_to_openai_chunks(&event, &mut state);
    assert_eq!(chunks.len(), 1);
    assert_eq!(state.message_id.as_deref(), Some("msg_1"));
    assert!(chunks[0].get("choices").is_some());
    assert_eq!(chunks[0]["choices"][0]["delta"]["role"], "assistant");
}

#[test]
fn claude_plain_thinking_is_buffered_until_block_stop() {
    let mut state = StreamState::default();
    let _ = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "message_start",
            "message": { "id": "msg_1", "model": "claude-3" }
        }),
        &mut state,
    );
    let _ = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": { "type": "thinking", "thinking": "" }
        }),
        &mut state,
    );
    let delta_chunks = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "thinking_delta", "thinking": "think" }
        }),
        &mut state,
    );
    assert!(delta_chunks.is_empty(), "delta_chunks = {delta_chunks:?}");

    let stop_chunks = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "content_block_stop",
            "index": 0
        }),
        &mut state,
    );
    assert!(stop_chunks
        .iter()
        .any(|chunk| chunk["choices"][0]["delta"]["reasoning_content"] == "think"));
}

#[test]
fn claude_thinking_boundaries_do_not_emit_synthetic_reasoning_markers() {
    let mut state = StreamState::default();
    let _ = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "message_start",
            "message": { "id": "msg_1", "model": "claude-3" }
        }),
        &mut state,
    );
    let start_chunks = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": { "type": "thinking", "thinking": "" }
        }),
        &mut state,
    );
    let stop_chunks = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "content_block_stop",
            "index": 0
        }),
        &mut state,
    );

    assert!(start_chunks.is_empty());
    assert!(stop_chunks.is_empty());
}

#[test]
fn claude_signature_delta_updates_block_state_without_reasoning_chunk() {
    let mut state = StreamState::default();
    let _ = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "message_start",
            "message": { "id": "msg_1", "model": "claude-3" }
        }),
        &mut state,
    );
    let _ = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": { "type": "thinking", "thinking": "" }
        }),
        &mut state,
    );

    let chunks = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "signature_delta", "signature": "sig_123" }
        }),
        &mut state,
    );

    assert!(chunks.is_empty(), "chunks = {chunks:?}");
    assert_eq!(
        state
            .claude_blocks
            .get(&0)
            .and_then(|block| block.signature.as_deref()),
        Some("sig_123")
    );
}

#[test]
fn claude_unknown_typed_delta_still_fails_closed() {
    let mut state = StreamState::default();
    let _ = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "message_start",
            "message": { "id": "msg_1", "model": "claude-3" }
        }),
        &mut state,
    );

    let chunks = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "unknown_future_delta", "payload": true }
        }),
        &mut state,
    );

    assert_eq!(chunks.len(), 1, "chunks = {chunks:?}");
    assert_eq!(chunks[0]["choices"][0]["finish_reason"], "error");
    assert!(chunks[0]["error"]["message"]
        .as_str()
        .unwrap_or("")
        .contains("unknown_future_delta"));
}

#[test]
fn translate_sse_event_anthropic_plain_thinking_to_openai_buffers_until_stop_and_continues() {
    let mut state = StreamState::default();
    let first = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        &serde_json::json!({
            "type": "message_start",
            "message": { "id": "msg_1", "model": "claude-3" }
        }),
        &mut state,
    );
    let second = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        &serde_json::json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {
                "type": "thinking",
                "thinking": "ponder"
            }
        }),
        &mut state,
    );
    let third = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        &serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "thinking_delta", "thinking": "hidden" }
        }),
        &mut state,
    );
    let fourth = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        &serde_json::json!({
            "type": "content_block_stop",
            "index": 0
        }),
        &mut state,
    );
    let fifth = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        &serde_json::json!({
            "type": "content_block_start",
            "index": 1,
            "content_block": {
                "type": "text",
                "text": ""
            }
        }),
        &mut state,
    );
    let sixth = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        &serde_json::json!({
            "type": "content_block_delta",
            "index": 1,
            "delta": { "type": "text_delta", "text": "Hi" }
        }),
        &mut state,
    );
    let fourth_joined = fourth
        .iter()
        .map(|bytes| String::from_utf8_lossy(bytes).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let sixth_joined = sixth
        .iter()
        .map(|bytes| String::from_utf8_lossy(bytes).to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        !first.is_empty(),
        "message_start should still initialize stream"
    );
    assert!(second.is_empty(), "second = {second:?}");
    assert!(third.is_empty(), "third = {third:?}");
    assert!(fifth.is_empty(), "fifth = {fifth:?}");
    assert!(
        fourth_joined.contains("reasoning_content"),
        "fourth_joined = {fourth_joined}"
    );
    assert!(sixth_joined.contains("\"content\":\"Hi\""), "{sixth_joined}");
    assert!(state.fatal_rejection.is_none(), "state = {state:?}");
}

#[test]
fn translate_sse_event_anthropic_plain_thinking_to_responses_buffers_until_stop_and_continues() {
    let mut state = StreamState::default();
    let _ = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &serde_json::json!({
            "type": "message_start",
            "message": { "id": "msg_1", "model": "claude-3" }
        }),
        &mut state,
    );
    let started = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &serde_json::json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {
                "type": "thinking",
                "thinking": "ponder"
            }
        }),
        &mut state,
    );
    let buffered_reasoning = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "thinking_delta", "thinking": "hidden" }
        }),
        &mut state,
    );
    let reasoning_done = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "thinking_delta", "thinking": "hidden" }
        }),
        &mut state,
    );
    let flushed_reasoning = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &serde_json::json!({
            "type": "content_block_stop",
            "index": 0
        }),
        &mut state,
    );
    let joined = flushed_reasoning
        .iter()
        .map(|bytes| String::from_utf8_lossy(bytes).to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        joined.contains("response.reasoning_summary_text.delta"),
        "joined = {joined}"
    );
    assert!(
        joined.contains("\"delta\":\"ponderhiddenhidden\""),
        "joined = {joined}"
    );
    assert!(started.is_empty(), "started = {started:?}");
    assert!(buffered_reasoning.is_empty(), "buffered_reasoning = {buffered_reasoning:?}");
    assert!(reasoning_done.is_empty(), "reasoning_done = {reasoning_done:?}");
    assert!(state.fatal_rejection.is_none(), "state = {state:?}");
}

#[test]
fn translate_sse_event_anthropic_plain_thinking_to_gemini_buffers_until_stop_and_continues() {
    let mut state = StreamState::default();
    let first = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::Google,
        &serde_json::json!({
            "type": "message_start",
            "message": { "id": "msg_1", "model": "claude-3" }
        }),
        &mut state,
    );
    let started = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::Google,
        &serde_json::json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {
                "type": "thinking",
                "thinking": "ponder"
            }
        }),
        &mut state,
    );
    let buffered = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::Google,
        &serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "thinking_delta", "thinking": "hidden" }
        }),
        &mut state,
    );
    let flushed = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::Google,
        &serde_json::json!({
            "type": "content_block_stop",
            "index": 0
        }),
        &mut state,
    );
    let joined = flushed
        .iter()
        .map(|bytes| String::from_utf8_lossy(bytes).to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        first.is_empty(),
        "message_start alone should not emit Gemini content"
    );
    assert!(started.is_empty(), "started = {started:?}");
    assert!(buffered.is_empty(), "buffered = {buffered:?}");
    assert!(!flushed.is_empty(), "{joined}");
    assert!(joined.contains("\"thought\":true"), "{joined}");
    assert!(state.fatal_rejection.is_none(), "state = {state:?}");
}

#[test]
fn translate_sse_event_anthropic_signature_delta_to_openai_fails_closed_before_releasing_reasoning() {
    let mut state = StreamState::default();
    let _ = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        &serde_json::json!({
            "type": "message_start",
            "message": { "id": "msg_1", "model": "claude-3" }
        }),
        &mut state,
    );
    let _ = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        &serde_json::json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {
                "type": "thinking",
                "thinking": ""
            }
        }),
        &mut state,
    );
    let buffered = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        &serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "thinking_delta", "thinking": "hidden" }
        }),
        &mut state,
    );
    let rejected = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        &serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "signature_delta", "signature": "sig_123" }
        }),
        &mut state,
    );
    let joined = rejected
        .iter()
        .map(|bytes| String::from_utf8_lossy(bytes).to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(buffered.is_empty(), "buffered = {buffered:?}");
    assert!(joined.contains("\"finish_reason\":\"error\""), "{joined}");
    assert!(joined.contains("signature provenance"), "{joined}");
    assert!(!joined.contains("reasoning_content"), "{joined}");
}

#[test]
fn translate_sse_event_anthropic_omitted_thinking_still_fails_closed_at_start() {
    let mut state = StreamState::default();
    let _ = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        &serde_json::json!({
            "type": "message_start",
            "message": { "id": "msg_1", "model": "claude-3" }
        }),
        &mut state,
    );
    let rejected = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        &serde_json::json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {
                "type": "thinking",
                "thinking": { "display": "omitted" }
            }
        }),
        &mut state,
    );

    let joined = rejected
        .iter()
        .map(|bytes| String::from_utf8_lossy(bytes).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("\"finish_reason\":\"error\""), "{joined}");
    assert!(
        joined.contains("thinking blocks cannot be translated losslessly")
            || joined.contains("omitted thinking"),
        "{joined}"
    );
}

#[test]
fn claude_server_tool_result_block_rejects_instead_of_succeeding() {
    let mut state = StreamState::default();
    let _ = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "message_start",
            "message": { "id": "msg_1", "model": "claude-3" }
        }),
        &mut state,
    );

    let chunks = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {
                "type": "server_tool_result",
                "tool_use_id": "server_1",
                "content": [{ "type": "text", "text": "result" }]
            }
        }),
        &mut state,
    );

    assert_eq!(chunks.len(), 1, "chunks = {chunks:?}");
    assert_eq!(chunks[0]["choices"][0]["finish_reason"], "error");
    assert!(chunks[0]["error"]["message"]
        .as_str()
        .unwrap_or("")
        .contains("server_tool_result"));
}

#[test]
fn claude_message_stop_preserves_extra_usage_fields() {
    let mut state = StreamState::default();
    let _ = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "message_start",
            "message": { "id": "msg_1", "model": "claude-3" }
        }),
        &mut state,
    );
    let _ = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "message_delta",
            "delta": { "stop_reason": "end_turn" },
            "usage": {
                "input_tokens": 10,
                "output_tokens": 5,
                "service_tier": "priority",
                "server_tool_use": { "web_search_requests": 2 }
            }
        }),
        &mut state,
    );

    let chunks =
        claude_event_to_openai_chunks(&serde_json::json!({ "type": "message_stop" }), &mut state);

    assert_eq!(chunks.len(), 1, "chunks = {chunks:?}");
    assert_eq!(chunks[0]["usage"]["prompt_tokens"], 10);
    assert_eq!(chunks[0]["usage"]["completion_tokens"], 5);
    assert_eq!(chunks[0]["usage"]["service_tier"], "priority");
    assert_eq!(
        chunks[0]["usage"]["server_tool_use"]["web_search_requests"],
        2
    );
}

#[test]
fn claude_pause_turn_stream_maps_to_openai_pause_turn_finish() {
    let mut state = StreamState::default();
    let _ = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "message_start",
            "message": { "id": "msg_1", "model": "claude-3" }
        }),
        &mut state,
    );
    let _ = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "message_delta",
            "delta": { "stop_reason": "pause_turn" }
        }),
        &mut state,
    );
    let chunks =
        claude_event_to_openai_chunks(&serde_json::json!({ "type": "message_stop" }), &mut state);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0]["choices"][0]["finish_reason"], "pause_turn");
}

#[test]
fn claude_refusal_stream_maps_to_openai_content_filter_finish() {
    let mut state = StreamState::default();
    let _ = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "message_start",
            "message": { "id": "msg_1", "model": "claude-3" }
        }),
        &mut state,
    );
    let _ = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "message_delta",
            "delta": { "stop_reason": "refusal" }
        }),
        &mut state,
    );
    let chunks =
        claude_event_to_openai_chunks(&serde_json::json!({ "type": "message_stop" }), &mut state);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0]["choices"][0]["finish_reason"], "content_filter");
}

#[test]
fn claude_tool_use_start_with_input_seeds_openai_arguments() {
    let mut state = StreamState::default();
    let _ = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "message_start",
            "message": { "id": "msg_1", "model": "claude-test" }
        }),
        &mut state,
    );
    let chunks = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "content_block_start",
            "index": 2,
            "content_block": {
                "type": "tool_use",
                "id": "call_1",
                "name": "exec_command",
                "input": { "cmd": "pwd" }
            }
        }),
        &mut state,
    );

    assert_eq!(chunks.len(), 1);
    let tool_calls = chunks[0]["choices"][0]["delta"]["tool_calls"]
        .as_array()
        .expect("tool_calls array");
    assert_eq!(tool_calls[0]["function"]["arguments"], "{\"cmd\":\"pwd\"}");
}

#[test]
fn claude_tool_use_seeded_input_and_json_delta_are_both_preserved() {
    let mut state = StreamState::default();
    let _ = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "message_start",
            "message": { "id": "msg_1", "model": "claude-test" }
        }),
        &mut state,
    );
    let _ = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "content_block_start",
            "index": 2,
            "content_block": {
                "type": "tool_use",
                "id": "call_1",
                "name": "exec_command",
                "input": { "cmd": "pw" }
            }
        }),
        &mut state,
    );
    let delta_chunks = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "content_block_delta",
            "index": 2,
            "delta": { "type": "input_json_delta", "partial_json": "d\"}" }
        }),
        &mut state,
    );

    assert_eq!(
        state
            .claude_tool_uses
            .get(&2)
            .expect("tool state")
            .arguments,
        "{\"cmd\":\"pwd\"}"
    );
    assert!(
        !delta_chunks.is_empty(),
        "delta should remain visible when start input was seeded"
    );
    let delta_tool_calls = delta_chunks[0]["choices"][0]["delta"]["tool_calls"]
        .as_array()
        .expect("delta tool_calls");
    assert_eq!(delta_tool_calls[0]["function"]["arguments"], "d\"}");
}

#[test]
fn claude_server_tool_use_is_preserved_in_stream_as_marked_tool_call() {
    let mut state = StreamState::default();
    let _ = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "message_start",
            "message": { "id": "msg_1", "model": "claude-test" }
        }),
        &mut state,
    );
    let chunks = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "content_block_start",
            "index": 1,
            "content_block": {
                "type": "server_tool_use",
                "id": "server_1",
                "name": "web_search",
                "input": { "query": "rust" }
            }
        }),
        &mut state,
    );

    assert_eq!(chunks.len(), 1);
    let tool_calls = chunks[0]["choices"][0]["delta"]["tool_calls"]
        .as_array()
        .expect("tool calls");
    assert_eq!(tool_calls[0]["function"]["name"], "web_search");
    assert_eq!(
        tool_calls[0]["proxied_tool_kind"],
        "anthropic_server_tool_use"
    );
}

#[test]
fn claude_empty_tool_input_waits_for_delta_arguments() {
    let mut state = StreamState::default();
    let _ = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "message_start",
            "message": { "id": "msg_1", "model": "claude-test" }
        }),
        &mut state,
    );
    let start_chunks = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "content_block_start",
            "index": 2,
            "content_block": {
                "type": "tool_use",
                "id": "call_1",
                "name": "exec_command",
                "input": {}
            }
        }),
        &mut state,
    );
    let delta_chunks = claude_event_to_openai_chunks(
        &serde_json::json!({
            "type": "content_block_delta",
            "index": 2,
            "delta": { "type": "input_json_delta", "partial_json": "{\"cmd\":\"pwd\"}" }
        }),
        &mut state,
    );

    let start_tool_calls = start_chunks[0]["choices"][0]["delta"]["tool_calls"]
        .as_array()
        .expect("start tool_calls");
    assert_eq!(start_tool_calls[0]["function"]["arguments"], "");
    let delta_tool_calls = delta_chunks[0]["choices"][0]["delta"]["tool_calls"]
        .as_array()
        .expect("delta tool_calls");
    assert_eq!(
        delta_tool_calls[0]["function"]["arguments"],
        "{\"cmd\":\"pwd\"}"
    );
}

#[test]
fn anthropic_error_event_maps_context_to_openai_context_finish() {
    let mut state = StreamState::default();
    let out = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        &serde_json::json!({
            "type": "error",
            "error": {
                "type": "invalid_request_error",
                "message": "maximum context length exceeded"
            }
        }),
        &mut state,
    );

    let joined = out
        .into_iter()
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("\"finish_reason\":\"context_length_exceeded\""));
    assert!(joined.contains("\"code\":\"context_length_exceeded\""));
    assert!(joined.contains("[DONE]"));
}

#[test]
fn anthropic_error_event_maps_non_specialized_failures_to_openai_error_finish() {
    for (error_type, message) in [
        ("overloaded_error", "Overloaded"),
        ("api_error", "Internal server error"),
        ("rate_limit_error", "Rate limited"),
        ("fallback_error", "Unknown Anthropic failure"),
    ] {
        let mut state = StreamState::default();
        let out = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiCompletion,
            &serde_json::json!({
                "type": "error",
                "error": {
                    "type": error_type,
                    "message": message
                }
            }),
            &mut state,
        );

        let joined = out
            .into_iter()
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains("\"finish_reason\":\"error\""),
            "expected error finish for {error_type}: {joined}"
        );
        assert!(!joined.contains("\"finish_reason\":\"stop\""));
        assert!(joined.contains("[DONE]"));
    }
}

#[test]
fn anthropic_error_event_preserves_specialized_openai_error_finishes() {
    for (message, finish_reason, code) in [
        (
            "maximum context length exceeded",
            "context_length_exceeded",
            "context_length_exceeded",
        ),
        (
            "Request blocked by content filter refusal",
            "content_filter",
            "content_filter",
        ),
    ] {
        let mut state = StreamState::default();
        let out = translate_sse_event(
            UpstreamFormat::Anthropic,
            UpstreamFormat::OpenAiCompletion,
            &serde_json::json!({
                "type": "error",
                "error": {
                    "type": "invalid_request_error",
                    "message": message
                }
            }),
            &mut state,
        );

        let joined = out
            .into_iter()
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains(&format!("\"finish_reason\":\"{finish_reason}\"")),
            "expected specialized finish for {message}: {joined}"
        );
        assert!(
            joined.contains(&format!("\"code\":\"{code}\"")),
            "expected specialized code for {message}: {joined}"
        );
        assert!(joined.contains("[DONE]"));
    }
}
