use super::*;

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
fn translate_sse_event_openai_to_non_openai_single_frame_multi_choice_fails_closed() {
    for client_format in [UpstreamFormat::Google, UpstreamFormat::OpenAiResponses] {
        let mut state = StreamState::default();
        let out = translate_sse_event(
            UpstreamFormat::OpenAiCompletion,
            client_format,
            &serde_json::json!({
                "id": "chatcmpl-msg123",
                "model": "gpt-4o",
                "choices": [
                    {
                        "index": 0,
                        "delta": { "content": "candidate-0" },
                        "finish_reason": null
                    },
                    {
                        "index": 1,
                        "delta": { "content": "candidate-1" },
                        "finish_reason": null
                    }
                ]
            }),
            &mut state,
        );

        let joined = out
            .iter()
            .map(|bytes| String::from_utf8_lossy(bytes).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            state.fatal_rejection.is_some(),
            "client_format = {client_format:?}, out = {joined}"
        );
        if client_format == UpstreamFormat::Google {
            assert!(
                out.is_empty(),
                "Gemini sink must not emit ad-hoc error frames"
            );
        } else {
            assert!(
                !out.is_empty(),
                "Responses sink should surface incompatibility as response.failed"
            );
            assert!(
                !joined.contains("candidate-0") && !joined.contains("candidate-1"),
                "fatal reject must not leak mixed-choice content for {client_format:?}: {joined}"
            );
        }
    }
}

#[test]
fn translate_sse_event_openai_to_non_openai_cross_frame_multi_choice_rejects_and_suppresses() {
    for client_format in [UpstreamFormat::Google, UpstreamFormat::OpenAiResponses] {
        let mut state = StreamState::default();
        let first = translate_sse_event(
            UpstreamFormat::OpenAiCompletion,
            client_format,
            &serde_json::json!({
                "id": "chatcmpl-msg123",
                "model": "gpt-4o",
                "choices": [{
                    "index": 0,
                    "delta": { "content": "candidate-0" },
                    "finish_reason": null
                }]
            }),
            &mut state,
        );
        assert!(
            state.fatal_rejection.is_none(),
            "single-choice frame should not be rejected early for {client_format:?}: {first:?}"
        );
        let second = translate_sse_event(
            UpstreamFormat::OpenAiCompletion,
            client_format,
            &serde_json::json!({
                "id": "chatcmpl-msg123",
                "model": "gpt-4o",
                "choices": [{
                    "index": 1,
                    "delta": { "content": "candidate-1" },
                    "finish_reason": null
                }]
            }),
            &mut state,
        );
        let third = translate_sse_event(
            UpstreamFormat::OpenAiCompletion,
            client_format,
            &serde_json::json!({
                "id": "chatcmpl-msg123",
                "model": "gpt-4o",
                "choices": [{
                    "index": 0,
                    "delta": { "content": "after-reject" },
                    "finish_reason": null
                }]
            }),
            &mut state,
        );

        let second_joined = second
            .iter()
            .map(|bytes| String::from_utf8_lossy(bytes).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            state.fatal_rejection.is_some(),
            "second frame should fatal reject mixed choice stream for {client_format:?}: {second_joined}"
        );
        if client_format == UpstreamFormat::Google {
            assert!(
                second.is_empty(),
                "Gemini sink must not emit ad-hoc error frames for fatal rejection"
            );
        } else {
            assert!(
                !second.is_empty(),
                "Responses sink should surface incompatibility as response.failed"
            );
            assert!(
                !second_joined.contains("candidate-1"),
                "fatal reject must not leak mixed-choice content for {client_format:?}: {second_joined}"
            );
        }
        assert!(
            third.is_empty(),
            "follow-up after fatal reject should be suppressed for {client_format:?}"
        );
    }
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
