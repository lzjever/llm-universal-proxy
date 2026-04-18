use super::*;

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
