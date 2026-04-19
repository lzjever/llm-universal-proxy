use super::*;
use crate::translate::translate_request;

#[test]
fn openai_chunk_to_responses_sse_maps_pause_turn_to_incomplete() {
    let mut state = StreamState::default();
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "pause_turn" }]
    });
    let out = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
    let joined = out
        .into_iter()
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("\"type\":\"response.incomplete\""));
    assert!(joined.contains("\"reason\":\"pause_turn\""));
}

#[test]
fn openai_chunk_to_responses_sse_maps_error_finish_to_failed() {
    let mut state = StreamState::default();
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "error" }]
    });
    let out = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
    let joined = out
        .into_iter()
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("\"type\":\"response.failed\""));
    assert!(joined.contains("\"code\":\"error\""));
}

#[test]
fn openai_chunk_to_responses_sse_maps_tool_error_finish_to_failed() {
    let mut state = StreamState::default();
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_error" }]
    });
    let out = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
    let joined = out
        .into_iter()
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("\"type\":\"response.failed\""));
    assert!(joined.contains("\"code\":\"tool_error\""));
}

#[test]
fn openai_chunk_to_responses_sse_preserves_specific_error_on_incompatibility_failure() {
    let mut state = StreamState::default();
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "error": {
            "type": "invalid_request_error",
            "code": "unsupported_openai_stream_event",
            "message": "OpenAI streaming response with multiple choices cannot be translated losslessly."
        },
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "error" }]
    });

    let out = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
    let joined = out
        .iter()
        .map(|b| String::from_utf8_lossy(b).to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(joined.contains("\"type\":\"response.failed\""));
    assert!(joined.contains("\"type\":\"invalid_request_error\""));
    assert!(joined.contains("\"code\":\"unsupported_openai_stream_event\""));
    assert!(joined.contains("multiple choices"));
    assert!(!joined.contains("\"type\":\"server_error\",\"code\":\"error\""));
}

#[test]
fn claude_context_window_exceeded_maps_to_responses_failed_event() {
    let mut state = StreamState::default();
    let start = serde_json::json!({
        "type": "message_start",
        "message": {
            "id": "msg_1",
            "model": "glm-5"
        }
    });
    let delta = serde_json::json!({
        "type": "message_delta",
        "delta": { "stop_reason": "model_context_window_exceeded" },
        "usage": { "input_tokens": 0, "output_tokens": 0 }
    });
    let stop = serde_json::json!({
        "type": "message_stop"
    });

    let mut out = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &start,
        &mut state,
    );
    out.extend(translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &delta,
        &mut state,
    ));
    out.extend(translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &stop,
        &mut state,
    ));

    let joined = out
        .into_iter()
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(joined.contains("\"type\":\"response.failed\""));
    assert!(joined.contains("\"code\":\"context_length_exceeded\""));
    assert!(!joined.contains("\"type\":\"response.completed\""));
}

#[test]
fn openai_chunk_to_responses_sse_maps_length_finish_to_incomplete() {
    let mut state = StreamState::default();
    let text_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{ "index": 0, "delta": { "content": "Hi" }, "finish_reason": null }]
    });
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "length" }]
    });

    let out1 = openai_chunk_to_responses_sse(&text_chunk, &mut state);
    let out2 = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
    let joined = out1
        .into_iter()
        .chain(out2)
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(joined.contains("\"type\":\"response.incomplete\""));
    assert!(joined.contains("\"reason\":\"max_output_tokens\""));
    assert!(!joined.contains("\"type\":\"response.completed\""));
}

#[test]
fn openai_chunk_to_responses_sse_marks_incomplete_partial_function_call_for_safe_anthropic_replay()
{
    let mut state = StreamState::default();
    let tool_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_1",
                    "function": {
                        "name": "exec_command",
                        "arguments": "{\"cmd\":\"cat > /tmp/spec.rs << 'EOF'\\nfn main() {\\n"
                    }
                }]
            },
            "finish_reason": null
        }]
    });
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "length" }]
    });

    let _ = openai_chunk_to_responses_sse(&tool_chunk, &mut state);
    let out = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
    let terminal = out
        .iter()
        .map(|bytes| parse_sse_json(bytes))
        .find(|event| event.get("type").and_then(Value::as_str) == Some("response.incomplete"))
        .expect("response.incomplete event");

    let output = terminal["response"]["output"]
        .as_array()
        .expect("response output");
    assert_eq!(output[0]["type"], "function_call");
    assert!(
        output[0]
            .get("_llmup_non_replayable_tool_call")
            .and_then(Value::as_object)
            .is_some(),
        "response output should carry internal non-replayable marker: {terminal:?}"
    );

    let mut replay_body = serde_json::json!({
        "model": "claude-3",
        "input": output.clone()
    });
    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        "claude-3",
        &mut replay_body,
        false,
    )
    .expect("marked incomplete streamed tool calls should degrade instead of failing Anthropic JSON validation");

    let blocks = replay_body["messages"][0]["content"]
        .as_array()
        .expect("assistant blocks");
    assert_eq!(blocks[0]["type"], "text");
    let text = blocks[0]["text"].as_str().expect("assistant text");
    assert!(text.contains("exec_command"), "text = {text}");
    assert!(text.contains("cat > /tmp/spec.rs"), "text = {text}");
}

#[test]
fn translate_sse_event_openai_done_marks_eof_truncated_function_call_for_safe_anthropic_replay() {
    let mut state = StreamState::default();
    let tool_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_1",
                    "function": {
                        "name": "exec_command",
                        "arguments": "{\"cmd\":\"cat > /tmp/spec.rs << 'EOF'\\nfn main() {\\n"
                    }
                }]
            },
            "finish_reason": null
        }]
    });

    let _ = translate_sse_event(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        &tool_chunk,
        &mut state,
    );
    let out = translate_sse_event(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        &serde_json::json!({ "_done": true }),
        &mut state,
    );

    let terminal = out
        .iter()
        .map(|bytes| parse_sse_json(bytes))
        .find(|event| event.get("type").and_then(Value::as_str) == Some("response.completed"))
        .expect("response.completed event");

    let output = terminal["response"]["output"]
        .as_array()
        .expect("response output");
    assert_eq!(output[0]["type"], "function_call");
    assert!(
        output[0]
            .get("_llmup_non_replayable_tool_call")
            .and_then(Value::as_object)
            .is_some(),
        "response output should carry internal non-replayable marker on EOF completion: {terminal:?}"
    );

    let mut replay_body = serde_json::json!({
        "model": "claude-3",
        "input": output.clone()
    });
    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        "claude-3",
        &mut replay_body,
        false,
    )
    .expect("EOF-truncated streamed tool calls should degrade instead of failing Anthropic JSON validation");

    let blocks = replay_body["messages"][0]["content"]
        .as_array()
        .expect("assistant blocks");
    assert_eq!(blocks[0]["type"], "text");
    let text = blocks[0]["text"].as_str().expect("assistant text");
    assert!(text.contains("exec_command"), "text = {text}");
    assert!(text.contains("cat > /tmp/spec.rs"), "text = {text}");
}

#[test]
fn translate_sse_event_openai_done_keeps_complete_function_call_replayable() {
    let mut state = StreamState::default();
    let tool_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_1",
                    "function": {
                        "name": "exec_command",
                        "arguments": "{\"cmd\":\"pwd\"}"
                    }
                }]
            },
            "finish_reason": null
        }]
    });

    let _ = translate_sse_event(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        &tool_chunk,
        &mut state,
    );
    let out = translate_sse_event(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        &serde_json::json!({ "_done": true }),
        &mut state,
    );

    let terminal = out
        .iter()
        .map(|bytes| parse_sse_json(bytes))
        .find(|event| event.get("type").and_then(Value::as_str) == Some("response.completed"))
        .expect("response.completed event");

    let output = terminal["response"]["output"]
        .as_array()
        .expect("response output");
    assert_eq!(output[0]["type"], "function_call");
    assert!(
        output[0].get("_llmup_non_replayable_tool_call").is_none(),
        "valid JSON object arguments should remain replayable on EOF completion: {terminal:?}"
    );
}

#[test]
fn anthropic_error_event_maps_to_responses_failed() {
    let mut state = StreamState::default();
    let out = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &serde_json::json!({
            "type": "error",
            "error": {
                "type": "overloaded_error",
                "message": "Overloaded"
            }
        }),
        &mut state,
    );

    let joined = out
        .into_iter()
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("\"type\":\"response.failed\""));
    assert!(joined.contains("\"type\":\"server_error\""));
    assert!(joined.contains("\"code\":\"server_is_overloaded\""));
}
