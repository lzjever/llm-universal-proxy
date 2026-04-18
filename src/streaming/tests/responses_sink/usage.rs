use super::*;

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
