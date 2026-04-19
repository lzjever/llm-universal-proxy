use super::*;

#[test]
fn gemini_event_with_text_produces_openai_chunks() {
    let event = serde_json::json!({
        "candidates": [{
            "content": { "parts": [{ "text": "Hello" }] },
            "finishReason": "STOP"
        }],
        "modelVersion": "gemini-1.5"
    });
    let mut state = StreamState::default();
    let chunks = gemini_event_to_openai_chunks(&event, &mut state);
    assert!(!chunks.is_empty());
    assert_eq!(state.model.as_deref(), Some("gemini-1.5"));
    let content_chunk = chunks
        .iter()
        .find(|c| c["choices"][0]["delta"].get("content").is_some());
    assert!(content_chunk.is_some());
    assert_eq!(
        content_chunk.unwrap()["choices"][0]["delta"]["content"],
        "Hello"
    );
}

#[test]
fn gemini_thought_part_produces_openai_reasoning_chunk() {
    let event = serde_json::json!({
        "candidates": [{
            "content": {
                "parts": [{
                    "text": "think",
                    "thought": true,
                    "thoughtSignature": "sig"
                }]
            },
            "finishReason": "STOP"
        }]
    });
    let mut state = StreamState::default();
    let chunks = gemini_event_to_openai_chunks(&event, &mut state);
    assert!(chunks
        .iter()
        .any(|chunk| chunk["choices"][0]["delta"]["reasoning_content"] == "think"));
}

#[test]
fn gemini_event_to_openai_chunks_maps_portable_finish_and_reasoning_usage() {
    let event = serde_json::json!({
        "response": {
            "responseId": "gem_resp_1",
            "modelVersion": "gemini-2.5",
            "candidates": [{
                "content": { "parts": [{ "text": "Hi" }], "role": "model" },
                "finishReason": "SAFETY"
            }],
            "usageMetadata": {
                "promptTokenCount": 11,
                "candidatesTokenCount": 5,
                "thoughtsTokenCount": 2,
                "totalTokenCount": 18,
                "cachedContentTokenCount": 3
            }
        }
    });
    let mut state = StreamState::default();
    let mut chunks = gemini_event_to_openai_chunks(&event, &mut state);
    if let Some(chunk) = flush_pending_gemini_finish_chunk(&mut state) {
        chunks.push(chunk);
    }
    let finish_chunk = chunks
        .iter()
        .find(|chunk| chunk["choices"][0]["finish_reason"].is_string())
        .expect("finish chunk");
    assert_eq!(
        finish_chunk["choices"][0]["finish_reason"],
        "content_filter"
    );
    assert_eq!(finish_chunk["usage"]["total_tokens"], 18);
    assert_eq!(
        finish_chunk["usage"]["prompt_tokens_details"]["cached_tokens"],
        3
    );
    assert_eq!(
        finish_chunk["usage"]["completion_tokens_details"]["reasoning_tokens"],
        2
    );
}

#[test]
fn gemini_event_to_openai_chunks_handles_prompt_feedback_without_candidates() {
    let event = serde_json::json!({
        "response": {
            "responseId": "gem_resp_feedback",
            "modelVersion": "gemini-2.5",
            "promptFeedback": { "blockReason": "SAFETY" },
            "usageMetadata": {
                "promptTokenCount": 3,
                "totalTokenCount": 3
            }
        }
    });
    let mut state = StreamState::default();
    let chunks = gemini_event_to_openai_chunks(&event, &mut state);
    let finish_chunk = chunks
        .iter()
        .find(|chunk| chunk["choices"][0]["finish_reason"].is_string())
        .expect("finish chunk");

    assert_eq!(
        finish_chunk["choices"][0]["finish_reason"],
        "content_filter"
    );
    assert_eq!(finish_chunk["usage"]["prompt_tokens"], 3);
    assert!(
        chunks
            .iter()
            .all(|chunk| chunk["choices"][0]["delta"].get("role").is_none()),
        "prompt block should not fabricate assistant role: {chunks:?}"
    );
}

#[test]
fn gemini_candidate_less_usage_and_metadata_only_partials_are_buffered_until_candidate_arrives() {
    let metadata_only = serde_json::json!({
        "response": {
            "responseId": "gem_resp_partial",
            "modelVersion": "gemini-2.5"
        }
    });
    let usage_only = serde_json::json!({
        "response": {
            "responseId": "gem_resp_partial",
            "modelVersion": "gemini-2.5",
            "usageMetadata": {
                "promptTokenCount": 3,
                "totalTokenCount": 3
            }
        }
    });
    let candidate = serde_json::json!({
        "response": {
            "responseId": "gem_resp_partial",
            "modelVersion": "gemini-2.5",
            "candidates": [{
                "content": { "parts": [{ "text": "Hi" }], "role": "model" },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "candidatesTokenCount": 5,
                "totalTokenCount": 8
            }
        }
    });
    let mut state = StreamState::default();
    let first = gemini_event_to_openai_chunks(&metadata_only, &mut state);
    let second = gemini_event_to_openai_chunks(&usage_only, &mut state);
    let mut third = gemini_event_to_openai_chunks(&candidate, &mut state);
    if let Some(chunk) = flush_pending_gemini_finish_chunk(&mut state) {
        third.push(chunk);
    }

    assert!(first.is_empty(), "metadata-only partial should be buffered");
    assert!(second.is_empty(), "usage-only partial should be buffered");
    assert!(
        third
            .iter()
            .any(|chunk| chunk["choices"][0]["delta"]["content"] == "Hi"),
        "candidate payload should still arrive after buffered partials: {third:?}"
    );
    let finish_chunk = third
        .iter()
        .find(|chunk| chunk["choices"][0]["finish_reason"].is_string())
        .expect("finish chunk");
    assert_eq!(finish_chunk["usage"]["prompt_tokens"], 3);
    assert_eq!(finish_chunk["usage"]["completion_tokens"], 5);
    assert_eq!(finish_chunk["usage"]["total_tokens"], 8);
}

#[test]
fn gemini_candidate_less_unknown_payload_still_rejects() {
    let event = serde_json::json!({
        "response": {
            "responseId": "gem_resp_partial",
            "modelVersion": "gemini-2.5",
            "unexpectedOutput": { "text": "oops" }
        }
    });
    let mut state = StreamState::default();
    let chunks = gemini_event_to_openai_chunks(&event, &mut state);

    assert_eq!(chunks.len(), 1, "chunks = {chunks:?}");
    assert_eq!(chunks[0]["choices"][0]["finish_reason"], "error");
    assert!(chunks[0]["error"]["message"]
        .as_str()
        .unwrap_or("")
        .contains("candidate"));
}

#[test]
fn gemini_multi_candidate_rejects_instead_of_silently_using_candidate_zero() {
    let event = serde_json::json!({
        "response": {
            "responseId": "gem_resp_multi",
            "modelVersion": "gemini-2.5",
            "candidates": [
                {
                    "content": { "parts": [{ "text": "candidate-0" }], "role": "model" },
                    "finishReason": "STOP"
                },
                {
                    "content": { "parts": [{ "text": "candidate-1" }], "role": "model" },
                    "finishReason": "STOP"
                }
            ],
            "usageMetadata": {
                "promptTokenCount": 3,
                "candidatesTokenCount": 5,
                "totalTokenCount": 8
            }
        }
    });
    let mut state = StreamState::default();
    let chunks = gemini_event_to_openai_chunks(&event, &mut state);

    assert_eq!(chunks.len(), 1, "chunks = {chunks:?}");
    assert_eq!(chunks[0]["choices"][0]["finish_reason"], "error");
    assert!(chunks[0]["error"]["message"]
        .as_str()
        .unwrap_or("")
        .contains("multiple candidates"));
    let rendered = serde_json::to_string(&chunks).expect("render chunks");
    assert!(!rendered.contains("candidate-0"), "{rendered}");
    assert!(!rendered.contains("candidate-1"), "{rendered}");
}

#[test]
fn translate_sse_event_gemini_cross_frame_multi_candidate_rejects_and_suppresses_followups() {
    let mut state = StreamState::default();

    let first = translate_sse_event(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiCompletion,
        &serde_json::json!({
            "response": {
                "responseId": "gem_resp_multi",
                "modelVersion": "gemini-2.5",
                "candidates": [{
                    "index": 1,
                    "content": { "parts": [{ "text": "candidate-1" }], "role": "model" }
                }]
            }
        }),
        &mut state,
    );
    let second = translate_sse_event(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiCompletion,
        &serde_json::json!({
            "response": {
                "responseId": "gem_resp_multi",
                "modelVersion": "gemini-2.5",
                "candidates": [{
                    "index": 0,
                    "content": { "parts": [{ "text": "candidate-0" }], "role": "model" }
                }]
            }
        }),
        &mut state,
    );

    let first_joined = first
        .iter()
        .map(|bytes| String::from_utf8_lossy(bytes).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(first_joined.contains("\"finish_reason\":\"error\""));
    assert!(first_joined.contains("multiple candidates"));
    assert!(
        second.is_empty(),
        "follow-up after fatal reject should be suppressed"
    );
}

#[test]
fn gemini_stream_non_success_finish_reasons_do_not_collapse_to_success() {
    let cases = [
        ("MALFORMED_FUNCTION_CALL", "tool_error"),
        ("UNEXPECTED_TOOL_CALL", "tool_error"),
        ("TOO_MANY_TOOL_CALLS", "tool_error"),
        ("MISSING_THOUGHT_SIGNATURE", "tool_error"),
        ("IMAGE_OTHER", "error"),
        ("NO_IMAGE", "error"),
        ("LANGUAGE", "error"),
    ];

    for (reason, expected) in cases {
        let event = serde_json::json!({
            "response": {
                "responseId": format!("gem_{reason}"),
                "modelVersion": "gemini-2.5",
                "candidates": [{
                    "content": {
                        "role": "model",
                        "parts": [{
                            "functionCall": {
                                "id": "call_1",
                                "name": "lookup_weather",
                                "args": { "city": "Tokyo" }
                            }
                        }]
                    },
                    "finishReason": reason
                }]
            }
        });
        let mut state = StreamState::default();
        let mut chunks = gemini_event_to_openai_chunks(&event, &mut state);
        if let Some(chunk) = flush_pending_gemini_finish_chunk(&mut state) {
            chunks.push(chunk);
        }
        let finish_chunk = chunks
            .iter()
            .find(|chunk| chunk["choices"][0]["finish_reason"].is_string())
            .expect("finish chunk");
        assert_eq!(
            finish_chunk["choices"][0]["finish_reason"], expected,
            "reason = {reason}, chunk = {finish_chunk:?}"
        );
    }
}

#[test]
fn gemini_inline_data_output_rejects_instead_of_silent_drop() {
    let mut state = StreamState::default();
    let chunks = gemini_event_to_openai_chunks(
        &serde_json::json!({
            "response": {
                "responseId": "gem_resp_inline",
                "modelVersion": "gemini-2.5",
                "candidates": [{
                    "content": {
                        "role": "model",
                        "parts": [{
                            "inlineData": {
                                "mimeType": "image/png",
                                "data": "AAAA"
                            }
                        }]
                    }
                }]
            }
        }),
        &mut state,
    );

    assert_eq!(chunks.len(), 1, "chunks = {chunks:?}");
    assert_eq!(chunks[0]["choices"][0]["finish_reason"], "error");
    assert!(
        chunks[0]["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("inlineData"),
        "chunks = {chunks:?}"
    );
}

#[test]
fn gemini_function_response_part_rejects_instead_of_silent_drop() {
    let mut state = StreamState::default();
    let chunks = gemini_event_to_openai_chunks(
        &serde_json::json!({
            "response": {
                "responseId": "gem_resp_tool_result",
                "modelVersion": "gemini-2.5",
                "candidates": [{
                    "content": {
                        "role": "model",
                        "parts": [{
                            "functionResponse": {
                                "id": "call_1",
                                "name": "get_weather",
                                "response": {
                                    "parts": [{
                                        "inlineData": {
                                            "mimeType": "image/png",
                                            "data": "AAAA"
                                        }
                                    }]
                                }
                            }
                        }]
                    }
                }]
            }
        }),
        &mut state,
    );

    assert_eq!(chunks.len(), 1, "chunks = {chunks:?}");
    assert_eq!(chunks[0]["choices"][0]["finish_reason"], "error");
    assert!(
        chunks[0]["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("functionResponse"),
        "chunks = {chunks:?}"
    );
}
