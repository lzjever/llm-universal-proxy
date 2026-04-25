use super::*;

fn assert_no_gemini_thought_signature(value: &serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            assert!(
                !map.contains_key("thoughtSignature"),
                "unexpected thoughtSignature in {value}"
            );
            assert!(
                !map.contains_key("thought_signature"),
                "unexpected thought_signature in {value}"
            );
            for child in map.values() {
                assert_no_gemini_thought_signature(child);
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                assert_no_gemini_thought_signature(child);
            }
        }
        _ => {}
    }
}

#[test]
fn openai_chunk_to_gemini_sse_emits_thought_parts() {
    let chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "model": "gpt-4o",
        "choices": [{ "index": 0, "delta": { "reasoning_content": "think" }, "finish_reason": null }]
    });
    let mut state = StreamState::default();
    let out = openai_chunk_to_gemini_sse(&chunk, &mut state);
    let joined = out
        .iter()
        .map(|b| String::from_utf8_lossy(b).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("\"thought\":true"));
    assert!(joined.contains("\"text\":\"think\""));
}

#[test]
fn openai_chunk_to_gemini_sse_maps_portable_finish_reason_names() {
    let chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "model": "gpt-4o",
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "length" }]
    });
    let mut state = StreamState::default();
    let out = openai_chunk_to_gemini_sse(&chunk, &mut state);
    let joined = out
        .iter()
        .map(|b| String::from_utf8_lossy(b).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("\"finishReason\":\"MAX_TOKENS\""));
    assert!(!joined.contains("\"finishReason\":\"LENGTH\""));
}

#[test]
fn openai_chunk_to_gemini_sse_does_not_add_fake_signature_to_tool_calls() {
    let chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "model": "gpt-4o",
        "usage": {
            "prompt_tokens": 11,
            "completion_tokens": 7,
            "total_tokens": 18
        },
        "choices": [{
            "index": 0,
            "delta": {
                "content": "Weather: ",
                "tool_calls": [
                    {
                        "index": 0,
                        "id": "call_1",
                        "function": { "name": "lookup_weather", "arguments": "{\"city\":\"Tokyo\"}" }
                    },
                    {
                        "index": 1,
                        "id": "call_2",
                        "function": { "name": "lookup_time", "arguments": "{\"city\":\"Tokyo\"}" }
                    }
                ]
            },
            "finish_reason": "tool_calls"
        }]
    });
    let mut state = StreamState::default();
    let out = openai_chunk_to_gemini_sse(&chunk, &mut state);
    assert_eq!(out.len(), 1);
    let payload = parse_sse_json(&out[0]);
    assert_no_gemini_thought_signature(&payload);
    let candidate = &payload["candidates"][0];
    assert_eq!(candidate["finishReason"], "STOP");
    assert_eq!(payload["usageMetadata"]["promptTokenCount"], 11);
    assert_eq!(payload["usageMetadata"]["candidatesTokenCount"], 7);
    assert_eq!(payload["usageMetadata"]["totalTokenCount"], 18);
    let parts = candidate["content"]["parts"]
        .as_array()
        .expect("gemini parts");
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[0]["text"], "Weather: ");
    assert_eq!(parts[1]["functionCall"]["id"], "call_1");
    assert_eq!(parts[1]["functionCall"]["name"], "lookup_weather");
    assert_eq!(parts[1]["functionCall"]["args"]["city"], "Tokyo");
    assert_eq!(parts[2]["functionCall"]["id"], "call_2");
    assert_eq!(parts[2]["functionCall"]["name"], "lookup_time");
    assert_eq!(parts[2]["functionCall"]["args"]["city"], "Tokyo");
}

#[test]
fn openai_chunk_to_gemini_sse_emits_tool_calls_in_call_index_order() {
    let mut state = StreamState::default();
    let higher_index_first = serde_json::json!({
        "id": "chatcmpl-msg123",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 1,
                    "id": "call_1",
                    "function": {
                        "name": "lookup_time",
                        "arguments": "{\"city\":\"Tokyo\"}"
                    }
                }]
            },
            "finish_reason": null
        }]
    });
    let lower_index_later = serde_json::json!({
        "id": "chatcmpl-msg123",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_0",
                    "function": {
                        "name": "lookup_weather",
                        "arguments": "{\"city\":\"Tokyo\"}"
                    }
                }]
            },
            "finish_reason": null
        }]
    });

    let out1 = openai_chunk_to_gemini_sse(&higher_index_first, &mut state);
    assert!(out1.is_empty(), "higher index should wait for lower index");

    let out2 = openai_chunk_to_gemini_sse(&lower_index_later, &mut state);
    assert_eq!(out2.len(), 1);
    let payload = parse_sse_json(&out2[0]);
    let parts = payload["candidates"][0]["content"]["parts"]
        .as_array()
        .expect("parts");
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0]["functionCall"]["id"], "call_0");
    assert_eq!(parts[1]["functionCall"]["id"], "call_1");
    assert_no_gemini_thought_signature(&payload);
}

#[test]
fn openai_chunk_to_gemini_sse_waits_for_earlier_incomplete_tool_before_later_parseable_tool() {
    let mut state = StreamState::default();
    let first_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_0",
                    "function": {
                        "name": "lookup_weather",
                        "arguments": "{\"city\":\"To"
                    }
                }]
            },
            "finish_reason": null
        }]
    });
    let second_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 1,
                    "id": "call_1",
                    "function": {
                        "name": "lookup_time",
                        "arguments": "{\"city\":\"Tokyo\"}"
                    }
                }]
            },
            "finish_reason": null
        }]
    });
    let third_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "function": {
                        "arguments": "kyo\"}"
                    }
                }]
            },
            "finish_reason": null
        }]
    });

    assert!(openai_chunk_to_gemini_sse(&first_chunk, &mut state).is_empty());
    let out2 = openai_chunk_to_gemini_sse(&second_chunk, &mut state);
    assert!(
        out2.is_empty(),
        "later parseable tool must wait for earlier incomplete one: {out2:?}"
    );

    let out3 = openai_chunk_to_gemini_sse(&third_chunk, &mut state);
    assert_eq!(out3.len(), 1);
    let payload = parse_sse_json(&out3[0]);
    let parts = payload["candidates"][0]["content"]["parts"]
        .as_array()
        .expect("gemini parts");
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0]["functionCall"]["id"], "call_0");
    assert_eq!(parts[1]["functionCall"]["id"], "call_1");
    assert_no_gemini_thought_signature(&payload);
}

#[test]
fn openai_chunk_to_gemini_sse_waits_for_complete_tool_call_arguments() {
    let mut state = StreamState::default();
    let first_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_1",
                    "function": {
                        "name": "lookup_weather",
                        "arguments": "{\"city\":\"To"
                    }
                }]
            },
            "finish_reason": null
        }]
    });
    let second_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "function": {
                        "arguments": "kyo\"}"
                    }
                }]
            },
            "finish_reason": null
        }]
    });

    let out1 = openai_chunk_to_gemini_sse(&first_chunk, &mut state);
    assert!(
        out1.is_empty(),
        "first fragment should not emit partial args"
    );

    let out2 = openai_chunk_to_gemini_sse(&second_chunk, &mut state);
    assert_eq!(out2.len(), 1);
    let payload = parse_sse_json(&out2[0]);
    let parts = payload["candidates"][0]["content"]["parts"]
        .as_array()
        .expect("gemini parts");
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0]["functionCall"]["id"], "call_1");
    assert_eq!(parts[0]["functionCall"]["name"], "lookup_weather");
    assert_eq!(parts[0]["functionCall"]["args"]["city"], "Tokyo");
    assert_no_gemini_thought_signature(&payload);
}

#[test]
fn openai_refusal_stream_maps_to_gemini_text_part_and_safety_terminal() {
    let mut state = StreamState::default();
    let refusal_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "delta": { "refusal": "Cannot comply" },
            "finish_reason": null
        }]
    });
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": "content_filter"
        }]
    });

    let out1 = openai_chunk_to_gemini_sse(&refusal_chunk, &mut state);
    let out2 = openai_chunk_to_gemini_sse(&finish_chunk, &mut state);

    assert_eq!(out1.len(), 1, "out1 = {out1:?}");
    let refusal_payload = parse_sse_json(&out1[0]);
    assert_eq!(
        refusal_payload["candidates"][0]["content"]["parts"][0]["text"],
        "Cannot comply"
    );
    assert_eq!(out2.len(), 1, "out2 = {out2:?}");
    let finish_payload = parse_sse_json(&out2[0]);
    assert_eq!(finish_payload["candidates"][0]["finishReason"], "SAFETY");
}

#[test]
fn openai_chunk_to_gemini_sse_suppresses_error_chunk_instead_of_emitting_illegal_error_frame() {
    let mut state = StreamState::default();
    let error_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "model": "gpt-4o",
        "error": {
            "type": "invalid_request_error",
            "code": "unsupported_openai_stream_event",
            "message": "OpenAI streaming response with multiple choices cannot be translated losslessly."
        },
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "error" }]
    });

    let out = openai_chunk_to_gemini_sse(&error_chunk, &mut state);

    assert!(
        out.is_empty(),
        "Gemini sink must not emit non-Gemini error frames"
    );
    assert!(state.fatal_rejection.is_some());
}

#[test]
fn openai_chunk_to_gemini_sse_rejects_nonportable_tool_kinds_without_emitting_function_calls() {
    let mut state = StreamState::default();
    let chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_1",
                    "type": "custom",
                    "proxied_tool_kind": "anthropic_server_tool_use",
                    "function": {
                        "name": "web_search",
                        "arguments": "{\"q\":\"tokyo\"}"
                    }
                }]
            },
            "finish_reason": null
        }]
    });

    let out = openai_chunk_to_gemini_sse(&chunk, &mut state);

    assert!(
        out.is_empty(),
        "non-portable tool kinds must not be emitted as Gemini functionCall"
    );
    assert!(state.fatal_rejection.is_some());
}

#[test]
fn openai_chunk_to_gemini_sse_does_not_add_fake_signature_to_first_parseable_tool_call() {
    let mut state = StreamState::default();
    let first_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_0",
                    "function": {
                        "name": "lookup_weather",
                        "arguments": "{\"city\":\"To"
                    }
                }]
            },
            "finish_reason": null
        }]
    });
    let second_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 1,
                    "id": "call_1",
                    "function": {
                        "name": "lookup_time",
                        "arguments": "{\"city\":\"Tokyo\"}"
                    }
                }]
            },
            "finish_reason": null
        }]
    });
    let third_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "function": {
                        "arguments": "kyo\"}"
                    }
                }]
            },
            "finish_reason": null
        }]
    });

    let out1 = openai_chunk_to_gemini_sse(&first_chunk, &mut state);
    assert!(out1.is_empty());

    let out2 = openai_chunk_to_gemini_sse(&second_chunk, &mut state);
    assert!(
        out2.is_empty(),
        "later tool calls must wait for earlier incomplete indices"
    );

    let out3 = openai_chunk_to_gemini_sse(&third_chunk, &mut state);
    assert_eq!(out3.len(), 1);
    let payload = parse_sse_json(&out3[0]);
    let parts = payload["candidates"][0]["content"]["parts"]
        .as_array()
        .expect("gemini parts");
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0]["functionCall"]["id"], "call_0");
    assert_eq!(parts[1]["functionCall"]["id"], "call_1");
    assert_no_gemini_thought_signature(&payload);
}

#[test]
fn translate_sse_event_openai_to_gemini_suppresses_usage_only_chunk_instead_of_leaking_openai_json()
{
    let mut state = StreamState::default();
    let out = translate_sse_event(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        &serde_json::json!({
            "id": "chatcmpl-msg123",
            "model": "gpt-4o",
            "usage": {
                "prompt_tokens": 11,
                "completion_tokens": 7,
                "total_tokens": 18
            },
            "choices": []
        }),
        &mut state,
    );

    assert!(
        out.is_empty(),
        "usage-only chunk should be buffered/suppressed"
    );
}

#[test]
fn translate_sse_event_openai_to_gemini_suppresses_incomplete_tool_args_chunk_instead_of_leaking_openai_json(
) {
    let mut state = StreamState::default();
    let out = translate_sse_event(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        &serde_json::json!({
            "id": "chatcmpl-msg123",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": {
                            "name": "lookup_weather",
                            "arguments": "{\"city\":\"To"
                        }
                    }]
                },
                "finish_reason": null
            }]
        }),
        &mut state,
    );

    assert!(
        out.is_empty(),
        "incomplete tool args should not leak raw OpenAI chunk"
    );
}
