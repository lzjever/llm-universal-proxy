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

#[test]
fn openai_chunk_to_responses_sse_closes_commentary_segment_before_tool_call_and_marks_final_answer()
{
    let mut state = StreamState::default();
    let commentary_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{
            "index": 0,
            "delta": { "content": "I will inspect the file first." },
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
                    "function": { "name": "exec_command", "arguments": "{\"cmd\":\"pwd\"}" }
                }]
            },
            "finish_reason": null
        }]
    });
    let final_answer_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{
            "index": 0,
            "delta": { "content": "The working directory is ready." },
            "finish_reason": null
        }]
    });
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": "stop"
        }]
    });

    let _ = openai_chunk_to_responses_sse(&commentary_chunk, &mut state);
    let tool_out = openai_chunk_to_responses_sse(&tool_chunk, &mut state);
    let final_out = openai_chunk_to_responses_sse(&final_answer_chunk, &mut state);
    let terminal_out = openai_chunk_to_responses_sse(&finish_chunk, &mut state);

    let tool_events = tool_out
        .iter()
        .map(|bytes| parse_sse_json(bytes))
        .collect::<Vec<_>>();
    let terminal_events = terminal_out
        .iter()
        .map(|bytes| parse_sse_json(bytes))
        .collect::<Vec<_>>();
    let final_joined = final_out
        .iter()
        .map(|bytes| String::from_utf8_lossy(bytes).to_string())
        .collect::<Vec<_>>()
        .join("\n");

    let commentary_done_index = tool_events
        .iter()
        .position(|event| {
            event.get("type").and_then(Value::as_str) == Some("response.output_item.done")
                && event["item"]["type"] == "message"
        })
        .expect("commentary message done event");
    let commentary_done = &tool_events[commentary_done_index];
    assert_eq!(commentary_done["item"]["phase"], "commentary");
    assert_eq!(
        commentary_done["item"]["content"][0]["text"],
        "I will inspect the file first."
    );

    let tool_added_index = tool_events
        .iter()
        .position(|event| {
            event.get("type").and_then(Value::as_str) == Some("response.output_item.added")
                && event["item"]["type"] == "function_call"
        })
        .expect("tool call added event");
    assert!(
        commentary_done_index < tool_added_index,
        "tool events = {tool_events:?}"
    );

    let final_message_done = terminal_events
        .iter()
        .find(|event| {
            event.get("type").and_then(Value::as_str) == Some("response.output_item.done")
                && event["item"]["type"] == "message"
        })
        .expect("final answer message done event");
    assert_eq!(final_message_done["item"]["phase"], "final_answer");
    assert_eq!(
        final_message_done["item"]["content"][0]["text"],
        "The working directory is ready."
    );
    assert!(
        !final_joined.contains("response.output_item.done"),
        "final answer delta chunk should not eagerly close the terminal segment: {final_joined}"
    );
}

#[test]
fn openai_chunk_to_responses_sse_does_not_emit_empty_completed_message_before_or_at_tool_terminal()
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
                    "function": { "name": "exec_command", "arguments": "{\"cmd\":\"pwd\"}" }
                }]
            },
            "finish_reason": null
        }]
    });
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": "stop"
        }]
    });

    let tool_out = openai_chunk_to_responses_sse(&tool_chunk, &mut state);
    let terminal_out = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
    let events = tool_out
        .into_iter()
        .chain(terminal_out)
        .map(|bytes| parse_sse_json(&bytes))
        .collect::<Vec<_>>();

    assert!(
        !events.iter().any(|event| {
            event.get("type").and_then(Value::as_str) == Some("response.output_item.done")
                && event["item"]["type"] == "message"
        }),
        "events = {events:?}"
    );
}

#[test]
fn openai_chunk_to_responses_sse_completed_output_keeps_commentary_tool_and_final_answer_in_order()
{
    let mut state = StreamState::default();
    let commentary_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{
            "index": 0,
            "delta": { "content": "I will inspect the file first." },
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
                    "function": { "name": "exec_command", "arguments": "{\"cmd\":\"pwd\"}" }
                }]
            },
            "finish_reason": null
        }]
    });
    let final_answer_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{
            "index": 0,
            "delta": { "content": "The working directory is ready." },
            "finish_reason": null
        }]
    });
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": "stop"
        }]
    });

    let _ = openai_chunk_to_responses_sse(&commentary_chunk, &mut state);
    let _ = openai_chunk_to_responses_sse(&tool_chunk, &mut state);
    let _ = openai_chunk_to_responses_sse(&final_answer_chunk, &mut state);
    let terminal_out = openai_chunk_to_responses_sse(&finish_chunk, &mut state);

    let terminal = terminal_out
        .iter()
        .map(|bytes| parse_sse_json(bytes))
        .find(|event| event.get("type").and_then(Value::as_str) == Some("response.completed"))
        .expect("response.completed event");
    let output = terminal["response"]["output"]
        .as_array()
        .expect("response output array");

    assert_eq!(output.len(), 3, "output = {output:?}");
    assert_eq!(output[0]["type"], "message");
    assert_eq!(output[0]["phase"], "commentary");
    assert_eq!(
        output[0]["content"][0]["text"],
        "I will inspect the file first."
    );
    assert_eq!(output[1]["type"], "function_call");
    assert_eq!(output[1]["call_id"], "call_1");
    assert_eq!(output[2]["type"], "message");
    assert_eq!(output[2]["phase"], "final_answer");
    assert_eq!(
        output[2]["content"][0]["text"],
        "The working directory is ready."
    );
}

#[test]
fn openai_chunk_to_responses_sse_tool_calls_terminal_marks_remaining_text_as_commentary() {
    let mut state = StreamState::default();
    let text_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{
            "index": 0,
            "delta": { "content": "Let me call a tool for that." },
            "finish_reason": null
        }]
    });
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": "tool_calls"
        }]
    });

    let _ = openai_chunk_to_responses_sse(&text_chunk, &mut state);
    let terminal_out = openai_chunk_to_responses_sse(&finish_chunk, &mut state);

    let terminal = terminal_out
        .iter()
        .map(|bytes| parse_sse_json(bytes))
        .find(|event| event.get("type").and_then(Value::as_str) == Some("response.completed"))
        .expect("response.completed event");
    let output = terminal["response"]["output"]
        .as_array()
        .expect("response output array");
    let message = output
        .iter()
        .find(|item| item["type"] == "message")
        .expect("commentary message item");

    assert_eq!(message["phase"], "commentary");
    assert_ne!(message["phase"], "final_answer");
    assert_eq!(
        message["content"][0]["text"],
        "Let me call a tool for that."
    );
}
