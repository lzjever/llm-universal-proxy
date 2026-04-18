use super::*;

#[test]
fn openai_chunk_to_responses_sse_allocates_unique_output_indices_per_item_kind() {
    let mut state = StreamState::default();
    let reasoning_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{
            "index": 0,
            "delta": { "reasoning_content": "think" },
            "finish_reason": null
        }]
    });
    let text_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{
            "index": 0,
            "delta": { "content": "Hi" },
            "finish_reason": null
        }]
    });
    let tool_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_1",
                    "function": { "name": "lookup", "arguments": "{\"city\":\"Tokyo\"}" }
                }]
            },
            "finish_reason": null
        }]
    });

    let joined = openai_chunk_to_responses_sse(&reasoning_chunk, &mut state)
        .into_iter()
        .chain(openai_chunk_to_responses_sse(&text_chunk, &mut state))
        .chain(openai_chunk_to_responses_sse(&tool_chunk, &mut state))
        .map(|bytes| parse_sse_json(&bytes))
        .filter(|event| {
            event.get("type").and_then(Value::as_str) == Some("response.output_item.added")
        })
        .collect::<Vec<_>>();

    assert_eq!(joined.len(), 3, "events = {joined:?}");
    let indices = joined
        .iter()
        .map(|event| event["output_index"].as_u64().expect("output index"))
        .collect::<Vec<_>>();
    let unique = indices
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(unique.len(), 3, "indices = {indices:?}");
    assert_eq!(joined[0]["item"]["type"], "reasoning");
    assert_eq!(joined[1]["item"]["type"], "message");
    assert_eq!(joined[2]["item"]["type"], "function_call");
}

#[test]
fn openai_chat_stream_to_responses_allocates_distinct_output_indices_for_reasoning_message_and_tool_calls(
) {
    let mut state = StreamState::default();
    let reasoning_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{
            "index": 0,
            "delta": { "reasoning_content": "think" },
            "finish_reason": null
        }]
    });
    let text_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{
            "index": 0,
            "delta": { "content": "Hi" },
            "finish_reason": null
        }]
    });
    let tool_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_1",
                    "function": { "name": "lookup", "arguments": "{\"city\":\"Tokyo\"}" }
                }]
            },
            "finish_reason": null
        }]
    });

    let output_item_indices = openai_chunk_to_responses_sse(&reasoning_chunk, &mut state)
        .into_iter()
        .chain(openai_chunk_to_responses_sse(&text_chunk, &mut state))
        .chain(openai_chunk_to_responses_sse(&tool_chunk, &mut state))
        .map(|bytes| parse_sse_json(&bytes))
        .filter(|event| {
            event.get("type").and_then(Value::as_str) == Some("response.output_item.added")
        })
        .map(|event| event["output_index"].as_u64().expect("output index"))
        .collect::<Vec<_>>();

    let unique = output_item_indices
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        output_item_indices.len(),
        3,
        "indices = {output_item_indices:?}"
    );
    assert_eq!(unique.len(), 3, "indices = {output_item_indices:?}");
}

#[test]
fn openai_chunk_to_responses_sse_emits_content_part_added_before_delta() {
    let mut state = StreamState::default();
    let role_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{ "index": 0, "delta": { "role": "assistant" }, "finish_reason": null }]
    });
    let text_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{ "index": 0, "delta": { "content": "hello" }, "finish_reason": null }]
    });

    let _ = openai_chunk_to_responses_sse(&role_chunk, &mut state);
    let out = openai_chunk_to_responses_sse(&text_chunk, &mut state);
    let joined = out
        .iter()
        .map(|b| String::from_utf8_lossy(b).to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(joined.contains("event: response.content_part.added"));
    assert!(joined.contains("event: response.output_text.delta"));
    assert!(joined.contains("\"delta\":\"hello\""));
}

#[test]
fn openai_chunk_to_responses_sse_wraps_reasoning_with_item_lifecycle() {
    let mut state = StreamState::default();
    let reasoning_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{
            "index": 0,
            "delta": { "reasoning_content": "think" },
            "finish_reason": null
        }]
    });
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
    });

    let out1 = openai_chunk_to_responses_sse(&reasoning_chunk, &mut state);
    let out2 = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
    let joined = out1
        .into_iter()
        .chain(out2)
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(joined.contains("response.reasoning_summary_part.added"));
    assert!(joined.contains("response.reasoning_summary_text.delta"));
    assert!(joined.contains("response.reasoning_summary_text.done"));
    assert!(joined.contains("\"type\":\"reasoning\""));
}

#[test]
fn openai_chunk_to_responses_sse_includes_response_id_on_child_events() {
    let mut state = StreamState::default();
    let text_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{ "index": 0, "delta": { "content": "Hi" }, "finish_reason": null }]
    });
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
    });

    let out1 = openai_chunk_to_responses_sse(&text_chunk, &mut state);
    let out2 = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
    let joined = out1
        .into_iter()
        .chain(out2)
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(joined.contains("\"type\":\"response.output_item.added\""));
    assert!(joined.contains("\"type\":\"response.content_part.added\""));
    assert!(joined.contains("\"type\":\"response.output_text.delta\""));
    assert!(joined.contains("\"type\":\"response.output_text.done\""));
    assert!(joined.contains("\"response_id\":\"chatcmpl-msg123\""));
}

#[test]
fn openai_chunk_to_responses_sse_includes_null_error_fields_on_response_events() {
    let mut state = StreamState::default();
    let text_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{ "index": 0, "delta": { "content": "Hi" }, "finish_reason": null }]
    });
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
    });

    let out1 = openai_chunk_to_responses_sse(&text_chunk, &mut state);
    let out2 = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
    let joined = out1
        .into_iter()
        .chain(out2)
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(joined.contains("\"type\":\"response.created\""));
    assert!(joined.contains("\"type\":\"response.in_progress\""));
    assert!(joined.contains("\"type\":\"response.completed\""));
    assert!(joined.contains("\"error\":null"));
    assert!(joined.contains("\"incomplete_details\":null"));
}
