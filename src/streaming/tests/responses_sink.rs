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
fn openai_chunk_to_responses_sse_emits_refusal_events() {
    let mut state = StreamState::default();
    let role_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{ "index": 0, "delta": { "role": "assistant" }, "finish_reason": null }]
    });
    let refusal_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{ "index": 0, "delta": { "refusal": "Cannot comply" }, "finish_reason": null }]
    });
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "content_filter" }]
    });

    let _ = openai_chunk_to_responses_sse(&role_chunk, &mut state);
    let out1 = openai_chunk_to_responses_sse(&refusal_chunk, &mut state);
    let out2 = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
    let joined = out1
        .into_iter()
        .chain(out2)
        .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(joined.contains("response.refusal.delta"));
    assert!(joined.contains("response.refusal.done"));
    assert!(joined.contains("\"type\":\"refusal\""));
    assert!(joined.contains("\"refusal\":\"Cannot comply\""));
}

#[test]
fn openai_chunk_to_responses_sse_preserves_text_and_refusal_parts_in_terminal_output() {
    let mut state = StreamState::default();
    let role_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{ "index": 0, "delta": { "role": "assistant" }, "finish_reason": null }]
    });
    let text_chunk_1 = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{ "index": 0, "delta": { "content": "Visible" }, "finish_reason": null }]
    });
    let refusal_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{ "index": 0, "delta": { "refusal": "Denied" }, "finish_reason": null }]
    });
    let text_chunk_2 = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{ "index": 0, "delta": { "content": " answer" }, "finish_reason": null }]
    });
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "content_filter" }]
    });

    let _ = openai_chunk_to_responses_sse(&role_chunk, &mut state);
    let _ = openai_chunk_to_responses_sse(&text_chunk_1, &mut state);
    let _ = openai_chunk_to_responses_sse(&refusal_chunk, &mut state);
    let _ = openai_chunk_to_responses_sse(&text_chunk_2, &mut state);
    let out = openai_chunk_to_responses_sse(&finish_chunk, &mut state);

    let terminal = out
        .iter()
        .map(|bytes| parse_sse_json(bytes))
        .find(|event| {
            matches!(
                event.get("type").and_then(Value::as_str),
                Some("response.completed") | Some("response.incomplete")
            )
        })
        .expect("terminal response event");
    let content = terminal["response"]["output"][0]["content"]
        .as_array()
        .expect("message content array");

    assert_eq!(content.len(), 2, "content = {content:?}");
    assert_eq!(content[0]["type"], "output_text");
    assert_eq!(content[0]["text"], "Visible answer");
    assert_eq!(content[1]["type"], "refusal");
    assert_eq!(content[1]["refusal"], "Denied");
}

#[test]
fn claude_citations_delta_preserves_annotations_through_responses_terminal() {
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
    let _ = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &serde_json::json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": { "type": "text", "text": "" }
        }),
        &mut state,
    );
    let _ = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "text_delta", "text": "Fact" }
        }),
        &mut state,
    );
    let _ = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {
                "type": "citations_delta",
                "citation": { "type": "url_citation", "url": "https://example.com/fact" }
            }
        }),
        &mut state,
    );
    let _ = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &serde_json::json!({
            "type": "message_delta",
            "delta": { "stop_reason": "end_turn" },
            "usage": { "input_tokens": 2, "output_tokens": 1, "service_tier": "priority" }
        }),
        &mut state,
    );
    let out = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &serde_json::json!({ "type": "message_stop" }),
        &mut state,
    );

    let terminal = out
        .iter()
        .map(|bytes| parse_sse_json(bytes))
        .find(|event| event.get("type").and_then(Value::as_str) == Some("response.completed"))
        .expect("response.completed");
    let content = terminal["response"]["output"][0]["content"]
        .as_array()
        .expect("message content");

    assert_eq!(
        content[0]["annotations"][0]["url"],
        "https://example.com/fact"
    );
}

#[test]
fn anthropic_extra_usage_fields_survive_to_responses_completed_usage() {
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
    let _ = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
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

    let out = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &serde_json::json!({ "type": "message_stop" }),
        &mut state,
    );
    let terminal = out
        .iter()
        .map(|bytes| parse_sse_json(bytes))
        .find(|event| event.get("type").and_then(Value::as_str) == Some("response.completed"))
        .expect("response.completed");

    assert_eq!(terminal["response"]["usage"]["service_tier"], "priority");
    assert_eq!(
        terminal["response"]["usage"]["server_tool_use"]["web_search_requests"],
        2
    );
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
fn openai_chunk_to_responses_sse_includes_accumulated_text_in_done_events() {
    let mut state = StreamState::default();
    let role_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{ "index": 0, "delta": { "role": "assistant" }, "finish_reason": null }]
    });
    let text_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{ "index": 0, "delta": { "content": "done-text" }, "finish_reason": null }]
    });
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "usage": { "prompt_tokens": 1, "completion_tokens": 2 },
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
    });

    let _ = openai_chunk_to_responses_sse(&role_chunk, &mut state);
    let _ = openai_chunk_to_responses_sse(&text_chunk, &mut state);
    let out = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
    let joined = out
        .iter()
        .map(|b| String::from_utf8_lossy(b).to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(joined.contains("\"text\":\"done-text\""));
    assert!(joined.contains("\"output\":[{"));
}

#[test]
fn openai_chunk_to_responses_sse_closes_function_calls() {
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
                    "function": { "name": "lookup", "arguments": "{\"x\":1}" }
                }]
            },
            "finish_reason": null
        }]
    });
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }]
    });

    let _ = openai_chunk_to_responses_sse(&tool_chunk, &mut state);
    let out = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
    let joined = out
        .iter()
        .map(|b| String::from_utf8_lossy(b).to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(joined.contains("response.function_call_arguments.done"));
    assert!(joined.contains("response.output_item.done"));
    assert!(joined.contains("\"type\":\"function_call\""));
}

#[test]
fn openai_chunk_to_responses_sse_preserves_custom_and_proxied_tool_kinds() {
    let mut state = StreamState::default();
    let custom_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_custom",
                    "type": "custom",
                    "function": { "name": "code_exec", "arguments": "print('hi')" }
                }]
            },
            "finish_reason": null
        }]
    });
    let proxied_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 1,
                    "id": "call_server",
                    "proxied_tool_kind": "anthropic_server_tool_use",
                    "function": { "name": "web_search", "arguments": "{\"query\":\"rust\"}" }
                }]
            },
            "finish_reason": null
        }]
    });
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }]
    });

    let out1 = openai_chunk_to_responses_sse(&custom_chunk, &mut state);
    let out2 = openai_chunk_to_responses_sse(&proxied_chunk, &mut state);
    let out3 = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
    let joined = out1
        .into_iter()
        .chain(out2)
        .chain(out3)
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(joined.contains("response.custom_tool_call_input.delta"));
    assert!(joined.contains("response.custom_tool_call_input.done"));
    assert!(joined.contains("\"type\":\"custom_tool_call\""));
    assert!(joined.contains("\"proxied_tool_kind\":\"anthropic_server_tool_use\""));
}

#[test]
fn anthropic_tool_use_does_not_duplicate_function_call_in_responses_completed() {
    let mut state = StreamState::default();
    let _ = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &serde_json::json!({
            "type": "message_start",
            "message": { "id": "msg_1", "model": "claude-test" }
        }),
        &mut state,
    );
    let _ = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &serde_json::json!({
            "type": "content_block_start",
            "index": 1,
            "content_block": {
                "type": "tool_use",
                "id": "call_1",
                "name": "exec_command",
                "input": {}
            }
        }),
        &mut state,
    );
    let _ = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &serde_json::json!({
            "type": "content_block_delta",
            "index": 1,
            "delta": { "type": "input_json_delta", "partial_json": "{\"cmd\":\"pwd\"}" }
        }),
        &mut state,
    );
    let _ = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &serde_json::json!({
            "type": "message_delta",
            "delta": { "stop_reason": "tool_use" },
            "usage": { "input_tokens": 10, "output_tokens": 5 }
        }),
        &mut state,
    );
    let out = translate_sse_event(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &serde_json::json!({ "type": "message_stop" }),
        &mut state,
    );

    let completed = out
        .iter()
        .map(|bytes| parse_sse_json(bytes))
        .find(|event| event.get("type").and_then(Value::as_str) == Some("response.completed"))
        .expect("response.completed event");
    let output = completed["response"]["output"]
        .as_array()
        .expect("response output array");
    let function_calls = output
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("function_call"))
        .collect::<Vec<_>>();
    assert_eq!(function_calls.len(), 1);
    assert_eq!(function_calls[0]["call_id"], "call_1");
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
fn openai_chunk_to_responses_sse_maps_minimax_reasoning_details() {
    let mut state = StreamState::default();
    let reasoning_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "model": "MiniMax-M2.7-highspeed",
        "created": 123,
        "choices": [{
            "index": 0,
            "delta": { "reasoning_details": [{ "text": "internal thinking" }] },
            "finish_reason": null
        }]
    });
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "model": "MiniMax-M2.7-highspeed",
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

    assert!(joined.contains("\"type\":\"response.reasoning_summary_text.delta\""));
    assert!(joined.contains("\"delta\":\"internal thinking\""));
    assert!(!joined.contains("\"type\":\"response.output_text.delta\""));
}

#[test]
fn openai_chunk_to_responses_sse_dedupes_minimax_cumulative_text() {
    let mut state = StreamState::default();
    let chunk1 = serde_json::json!({
        "id": "chatcmpl-msg123",
        "model": "MiniMax-M2.7-highspeed",
        "created": 123,
        "choices": [{ "index": 0, "delta": { "content": "Hello" }, "finish_reason": null }]
    });
    let chunk2 = serde_json::json!({
        "id": "chatcmpl-msg123",
        "model": "MiniMax-M2.7-highspeed",
        "created": 123,
        "choices": [{ "index": 0, "delta": { "content": "Hello world" }, "finish_reason": null }]
    });
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "model": "MiniMax-M2.7-highspeed",
        "created": 123,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
    });
    let usage_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "model": "MiniMax-M2.7-highspeed",
        "created": 123,
        "choices": [],
        "usage": { "prompt_tokens": 10, "completion_tokens": 2, "total_tokens": 12 }
    });

    let out1 = openai_chunk_to_responses_sse(&chunk1, &mut state);
    let out2 = openai_chunk_to_responses_sse(&chunk2, &mut state);
    let _ = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
    let out3 = openai_chunk_to_responses_sse(&usage_chunk, &mut state);
    let joined = out1
        .into_iter()
        .chain(out2)
        .chain(out3)
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(joined.contains("\"delta\":\"Hello\""));
    assert!(joined.contains("\"delta\":\" world\""));
    assert!(joined.contains("\"text\":\"Hello world\""));
}

#[test]
fn openai_chunk_to_responses_sse_waits_for_usage_only_chunk_before_completed() {
    let mut state = StreamState::default();
    let text_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "model": "MiniMax-M2.7-highspeed",
        "created": 123,
        "choices": [{ "index": 0, "delta": { "content": "Hello" }, "finish_reason": null }]
    });
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "model": "MiniMax-M2.7-highspeed",
        "created": 123,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
    });
    let usage_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "model": "MiniMax-M2.7-highspeed",
        "created": 123,
        "choices": [],
        "usage": {
            "prompt_tokens": 42,
            "completion_tokens": 172,
            "total_tokens": 214,
            "completion_tokens_details": { "reasoning_tokens": 162 }
        }
    });

    let _ = openai_chunk_to_responses_sse(&text_chunk, &mut state);
    let finish_out = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
    let usage_out = openai_chunk_to_responses_sse(&usage_chunk, &mut state);
    let finish_joined = finish_out
        .iter()
        .map(|b| String::from_utf8_lossy(b).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let usage_joined = usage_out
        .iter()
        .map(|b| String::from_utf8_lossy(b).to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!finish_joined.contains("\"type\":\"response.completed\""));
    assert!(usage_joined.contains("\"type\":\"response.completed\""));
    assert!(usage_joined.contains("\"total_tokens\":214"));
    assert!(usage_joined.contains("\"output_tokens_details\":{\"reasoning_tokens\":162}"));
}

#[test]
fn openai_chunk_to_responses_sse_preserves_usage_details_and_total_tokens() {
    let mut state = StreamState::default();
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "usage": {
            "prompt_tokens": 11,
            "completion_tokens": 7,
            "total_tokens": 25,
            "prompt_tokens_details": { "cached_tokens": 3 },
            "completion_tokens_details": { "reasoning_tokens": 2 }
        },
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
    });

    let out = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
    let joined = out
        .iter()
        .map(|b| String::from_utf8_lossy(b).to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(joined.contains("\"total_tokens\":25"));
    assert!(joined.contains("\"input_tokens_details\":{\"cached_tokens\":3}"));
    assert!(joined.contains("\"output_tokens_details\":{\"reasoning_tokens\":2}"));
}

#[test]
fn openai_stream_usage_to_responses_preserves_audio_prediction_and_unknown_detail_fields() {
    let mut state = StreamState::default();
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "usage": {
            "prompt_tokens": 11,
            "completion_tokens": 7,
            "total_tokens": 18,
            "service_tier": "priority",
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
        },
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
    });

    let out = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
    let terminal = out
        .iter()
        .map(|bytes| parse_sse_json(bytes))
        .find(|event| event["type"] == "response.completed")
        .expect("responses terminal");

    assert_eq!(terminal["response"]["usage"]["service_tier"], "priority");
    assert_eq!(
        terminal["response"]["usage"]["input_tokens_details"]["cached_tokens"],
        3
    );
    assert_eq!(
        terminal["response"]["usage"]["input_tokens_details"]["audio_tokens"],
        2
    );
    assert_eq!(
        terminal["response"]["usage"]["input_tokens_details"]["future_prompt_detail"],
        4
    );
    assert_eq!(
        terminal["response"]["usage"]["output_tokens_details"]["reasoning_tokens"],
        1
    );
    assert_eq!(
        terminal["response"]["usage"]["output_tokens_details"]["audio_tokens"],
        5
    );
    assert_eq!(
        terminal["response"]["usage"]["output_tokens_details"]["accepted_prediction_tokens"],
        6
    );
    assert_eq!(
        terminal["response"]["usage"]["output_tokens_details"]["rejected_prediction_tokens"],
        2
    );
    assert_eq!(
        terminal["response"]["usage"]["output_tokens_details"]["future_completion_detail"],
        8
    );
}

#[test]
fn openai_chunk_to_responses_sse_preserves_usage_on_context_failure() {
    let mut state = StreamState::default();
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "usage": {
            "prompt_tokens": 11,
            "completion_tokens": 7,
            "total_tokens": 25,
            "prompt_tokens_details": { "cached_tokens": 3 },
            "completion_tokens_details": { "reasoning_tokens": 2 }
        },
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "context_length_exceeded" }]
    });

    let out = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
    let joined = out
        .iter()
        .map(|b| String::from_utf8_lossy(b).to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(joined.contains("\"type\":\"response.failed\""));
    assert!(joined.contains("\"code\":\"context_length_exceeded\""));
    assert!(joined.contains("\"total_tokens\":25"));
    assert!(joined.contains("\"input_tokens_details\":{\"cached_tokens\":3}"));
    assert!(joined.contains("\"output_tokens_details\":{\"reasoning_tokens\":2}"));
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
fn openai_chunk_to_responses_sse_includes_call_metadata_on_function_events() {
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
                    "function": { "name": "lookup", "arguments": "{\"x\":1}" }
                }]
            },
            "finish_reason": null
        }]
    });
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }]
    });

    let out1 = openai_chunk_to_responses_sse(&tool_chunk, &mut state);
    let out2 = openai_chunk_to_responses_sse(&finish_chunk, &mut state);
    let joined = out1
        .into_iter()
        .chain(out2)
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(joined.contains("\"type\":\"response.function_call_arguments.delta\""));
    assert!(joined.contains("\"type\":\"response.function_call_arguments.done\""));
    assert!(joined.contains("\"call_id\":\"call_1\""));
    assert!(joined.contains("\"name\":\"lookup\""));
}

#[test]
fn openai_chunk_to_responses_sse_adds_empty_annotations_to_text_parts() {
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

    assert!(joined.contains("\"type\":\"response.content_part.added\""));
    assert!(joined.contains("\"type\":\"response.content_part.done\""));
    assert!(joined.contains("\"annotations\":[]"));
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
