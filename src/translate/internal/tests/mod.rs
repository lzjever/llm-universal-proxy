use super::*;
use serde_json::json;

fn assess_request_translation(
    client_format: UpstreamFormat,
    upstream_format: UpstreamFormat,
    body: &serde_json::Value,
) -> super::models::TranslationAssessment {
    super::assessment::assess_request_translation(
        client_format,
        upstream_format,
        body,
        crate::config::CompatibilityMode::Balanced,
    )
}

fn request_translation_policy(
    compatibility_mode: crate::config::CompatibilityMode,
    max_output_tokens: Option<u64>,
) -> RequestTranslationPolicy {
    RequestTranslationPolicy {
        compatibility_mode,
        surface: crate::config::ModelSurface {
            limits: max_output_tokens.map(|max_output_tokens| crate::config::ModelLimits {
                context_window: None,
                max_output_tokens: Some(max_output_tokens),
            }),
            modalities: None,
            tools: None,
        },
    }
}

#[test]
fn request_translation_policy_default_uses_default_compatibility_mode() {
    assert_eq!(
        crate::config::CompatibilityMode::default(),
        crate::config::CompatibilityMode::MaxCompat
    );

    let default_policy = RequestTranslationPolicy::default();
    assert_eq!(
        default_policy.compatibility_mode,
        crate::config::CompatibilityMode::MaxCompat
    );
    assert!(default_policy.is_empty());

    let balanced_policy = RequestTranslationPolicy {
        compatibility_mode: crate::config::CompatibilityMode::Balanced,
        surface: crate::config::ModelSurface::default(),
    };
    assert!(!balanced_policy.is_empty());
}

#[test]
fn responses_to_messages_via_translate() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": [
            { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "Hi" }] }
        ],
        "instructions": "You are helpful."
    });
    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        true,
    )
    .unwrap();
    assert!(body.get("messages").is_some());
    assert!(body.get("input").is_none());
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[0]["content"], "You are helpful.");
    assert_eq!(messages[1]["role"], "user");
}

#[test]
fn messages_to_responses_via_translate() {
    let mut body = json!({
        "model": "gpt-4o",
        "messages": [
            { "role": "system", "content": "Helper" },
            { "role": "user", "content": "Hi" }
        ]
    });
    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        "gpt-4o",
        &mut body,
        true,
    )
    .unwrap();
    assert!(body.get("input").is_some());
    assert_eq!(body["instructions"], "Helper");
    let input = body["input"].as_array().unwrap();
    assert_eq!(input.len(), 1);
    assert_eq!(input[0]["type"], "message");
    assert_eq!(input[0]["role"], "user");
}

#[test]
fn messages_to_responses_preserves_reasoning_items() {
    let mut body = json!({
        "model": "gpt-4o",
        "messages": [
            {
                "role": "assistant",
                "reasoning_content": "thinking",
                "content": "Hi"
            }
        ]
    });
    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        "gpt-4o",
        &mut body,
        true,
    )
    .unwrap();
    let input = body["input"].as_array().unwrap();
    assert_eq!(input[0]["type"], "reasoning");
    assert_eq!(input[0]["summary"][0]["text"], "thinking");
    assert_eq!(input[1]["type"], "message");
}

#[test]
fn openai_responses_round_trip_preserves_role_order_and_multimodal_parts() {
    let original = json!({
        "model": "gpt-4o",
        "messages": [
            { "role": "system", "content": "System A" },
            {
                "role": "user",
                "content": [
                    { "type": "text", "text": "Look at this" },
                    { "type": "image_url", "image_url": { "url": "https://example.com/cat.png", "detail": "high" } },
                    { "type": "input_audio", "input_audio": { "data": "AAAA", "format": "wav" } },
                    { "type": "file", "file": { "file_id": "file_123" } }
                ]
            },
            { "role": "developer", "content": "Developer B" },
            { "role": "user", "content": "Continue" }
        ]
    });
    let mut body = original.clone();
    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        "gpt-4o",
        &mut body,
        false,
    )
    .unwrap();

    assert!(body.get("instructions").is_none(), "body = {body}");
    let input = body["input"].as_array().expect("responses input");
    assert_eq!(input.len(), 4);
    assert_eq!(input[0]["role"], "system");
    assert_eq!(input[1]["role"], "user");
    assert_eq!(input[1]["content"][0]["type"], "input_text");
    assert_eq!(input[1]["content"][1]["type"], "input_image");
    assert_eq!(input[1]["content"][2]["type"], "input_audio");
    assert_eq!(input[1]["content"][3]["type"], "input_file");
    assert_eq!(input[2]["role"], "developer");
    assert_eq!(input[3]["role"], "user");

    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        false,
    )
    .unwrap();

    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 4, "messages = {messages:?}");
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(messages[2]["role"], "developer");
    assert_eq!(messages[3]["role"], "user");
    assert_eq!(messages[1]["content"][0]["type"], "text");
    assert_eq!(messages[1]["content"][1]["type"], "image_url");
    assert_eq!(messages[1]["content"][2]["type"], "input_audio");
    assert_eq!(messages[1]["content"][3]["type"], "file");
}

#[test]
fn translate_request_chat_to_responses_maps_user_image_audio_and_file_legally() {
    let mut body = json!({
        "model": "gpt-4o",
        "messages": [{
            "role": "user",
            "content": [
                { "type": "text", "text": "Describe these inputs" },
                { "type": "image_url", "image_url": { "url": "https://example.com/cat.png", "detail": "high" } },
                { "type": "input_audio", "input_audio": { "data": "AAAA", "format": "wav" } },
                { "type": "file", "file": { "file_id": "file_123" } }
            ]
        }]
    });

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        "gpt-4o",
        &mut body,
        false,
    )
    .unwrap();

    let input = body["input"].as_array().expect("responses input");
    assert_eq!(input.len(), 1);
    let content = input[0]["content"].as_array().expect("content");
    assert_eq!(content[0]["type"], "input_text");
    assert_eq!(content[1]["type"], "input_image");
    assert_eq!(content[1]["image_url"], "https://example.com/cat.png");
    assert_eq!(content[1]["detail"], "high");
    assert_eq!(content[2]["type"], "input_audio");
    assert_eq!(content[2]["input_audio"]["data"], "AAAA");
    assert_eq!(content[2]["input_audio"]["format"], "wav");
    assert_eq!(content[3]["type"], "input_file");
    assert_eq!(content[3]["file_id"], "file_123");
}

#[test]
fn translate_request_chat_to_responses_maps_user_image_to_input_image_legally() {
    let mut body = json!({
        "model": "gpt-4o",
        "messages": [{
            "role": "user",
            "content": [
                { "type": "text", "text": "Describe this image" },
                { "type": "image_url", "image_url": { "url": "https://example.com/cat.png", "detail": "high" } }
            ]
        }]
    });

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        "gpt-4o",
        &mut body,
        false,
    )
    .unwrap();

    let input = body["input"].as_array().expect("responses input");
    let content = input[0]["content"].as_array().expect("content");
    assert_eq!(content[0]["type"], "input_text");
    assert_eq!(content[1]["type"], "input_image");
    assert_eq!(content[1]["image_url"], "https://example.com/cat.png");
    assert_eq!(content[1]["detail"], "high");
}

#[test]
fn translate_request_chat_to_responses_uses_custom_tool_call_output_for_custom_tool_results() {
    let mut body = json!({
        "model": "gpt-4o",
        "messages": [
            {
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_custom",
                    "type": "custom",
                    "custom": {
                        "name": "code_exec",
                        "input": "print('hi')"
                    }
                }]
            },
            {
                "role": "tool",
                "tool_call_id": "call_custom",
                "content": "exit 0"
            }
        ]
    });

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        "gpt-4o",
        &mut body,
        false,
    )
    .unwrap();

    let input = body["input"].as_array().expect("responses input");
    assert_eq!(input[0]["type"], "custom_tool_call");
    assert_eq!(input[1]["type"], "custom_tool_call_output");
    assert_eq!(input[1]["call_id"], "call_custom");
    assert_eq!(input[1]["output"], "exit 0");
}

#[test]
fn translate_request_responses_to_openai_bridges_custom_tool_definition_choice_and_history() {
    let mut body = json!({
        "model": "gpt-4o",
        "tools": [{
            "type": "custom",
            "name": "code_exec",
            "description": "Executes code",
            "format": { "type": "text" }
        }],
        "tool_choice": {
            "type": "custom",
            "name": "code_exec"
        },
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "run this" }]
            },
            {
                "type": "custom_tool_call",
                "call_id": "call_custom",
                "name": "code_exec",
                "input": "print('hi')"
            },
            {
                "type": "custom_tool_call_output",
                "call_id": "call_custom",
                "output": "exit 0"
            }
        ]
    });

    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        false,
    )
    .unwrap();

    let tools = body["tools"].as_array().expect("chat tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(
        tools[0],
        json!({
            "type": "function",
            "function": {
                "name": "code_exec",
                "description": "Executes code",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "input": { "type": "string" }
                    },
                    "required": ["input"],
                    "additionalProperties": false
                }
            }
        })
    );
    assert_eq!(
        body["tool_choice"],
        json!({
            "type": "function",
            "function": {
                "name": "code_exec"
            }
        })
    );
    assert_eq!(
        body["_llmup_tool_bridge_context"],
        json!({
            "compatibility_mode": "max_compat",
            "entries": {
                "code_exec": {
                    "source_kind": "custom_text",
                    "transport_kind": "function_object_wrapper",
                    "wrapper_field": "input",
                    "expected_canonical_shape": "single_required_string"
                }
            }
        })
    );

    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 3, "messages = {messages:?}");
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(
        messages[1]["tool_calls"][0],
        json!({
            "id": "call_custom",
            "type": "function",
            "function": {
                "name": "code_exec",
                "arguments": "{\"input\":\"print('hi')\"}"
            }
        })
    );
    assert_eq!(messages[2]["role"], "tool");
    assert_eq!(messages[2]["tool_call_id"], "call_custom");
    assert_eq!(messages[2]["content"], "exit 0");
}

#[test]
fn translate_request_responses_to_openai_rejects_reserved_bridge_prefix_for_function_names() {
    let cases = [
        json!({
            "model": "gpt-4o",
            "input": "run this",
            "tools": [{
                "type": "function",
                "name": "__llmup_custom__lookup_weather",
                "parameters": { "type": "object", "properties": {} }
            }]
        }),
        json!({
            "model": "gpt-4o",
            "input": "run this",
            "tool_choice": {
                "type": "function",
                "name": "__llmup_custom__lookup_weather"
            }
        }),
        json!({
            "model": "gpt-4o",
            "input": [{
                "type": "function_call",
                "call_id": "call_prefixed",
                "name": "__llmup_custom__lookup_weather",
                "arguments": "{\"city\":\"SF\"}"
            }]
        }),
    ];

    for mut body in cases {
        let err = translate_request(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            "gpt-4o",
            &mut body,
            false,
        )
        .expect_err("reserved bridge namespace should be rejected");

        assert!(err.contains("__llmup_custom__"), "err = {err}");
        assert!(err.contains("reserved bridge prefix"), "err = {err}");
    }
}

#[test]
fn translate_request_responses_standalone_custom_tool_output_to_non_openai_rejects() {
    for upstream_format in [UpstreamFormat::Anthropic, UpstreamFormat::Google] {
        let mut body = json!({
            "model": "gpt-4o",
            "input": [{
                "type": "custom_tool_call_output",
                "call_id": "call_custom",
                "output": "exit 0"
            }]
        });

        let err = translate_request(
            UpstreamFormat::OpenAiResponses,
            upstream_format,
            "target-model",
            &mut body,
            false,
        )
        .expect_err("standalone custom tool outputs should fail closed");

        assert!(err.contains("custom tools"), "err = {err}");
    }
}

#[test]
fn translate_request_responses_tool_output_text_arrays_to_openai_text_parts() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": [
            {
                "type": "function_call",
                "call_id": "call_1",
                "name": "lookup_weather",
                "arguments": "{\"city\":\"Tokyo\"}"
            },
            {
                "type": "function_call_output",
                "call_id": "call_1",
                "output": [
                    { "type": "input_text", "text": "Sunny" },
                    { "type": "input_text", "text": " 24C" }
                ]
            }
        ]
    });

    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        false,
    )
    .unwrap();

    let messages = body["messages"].as_array().expect("messages");
    let content = messages[1]["content"].as_array().expect("tool content");
    assert_eq!(messages[1]["role"], "tool");
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[0]["text"], "Sunny");
    assert_eq!(content[1]["type"], "text");
    assert_eq!(content[1]["text"], " 24C");
}

#[test]
fn translate_request_responses_tool_output_media_arrays_to_openai_rejects() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": [
            {
                "type": "function_call",
                "call_id": "call_1",
                "name": "inspect_media",
                "arguments": "{}"
            },
            {
                "type": "function_call_output",
                "call_id": "call_1",
                "output": [
                    {
                        "type": "input_image",
                        "image_url": "https://example.com/cat.png"
                    }
                ]
            }
        ]
    });

    let err = translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        false,
    )
    .expect_err("Responses tool output media arrays should fail closed on Chat targets");

    assert!(err.contains("tool output"), "err = {err}");
    assert!(err.contains("input_image"), "err = {err}");
}

#[test]
fn translate_request_responses_to_openai_bridges_custom_tools_and_tool_choice_variants() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": "run this",
        "tools": [{
            "type": "custom",
            "name": "code_exec",
            "description": "Executes code",
            "format": { "type": "text" }
        }],
        "tool_choice": {
            "type": "custom",
            "name": "code_exec"
        }
    });

    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        false,
    )
    .unwrap();

    let tools = body["tools"].as_array().expect("chat tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["type"], "function");
    assert_eq!(tools[0]["function"]["name"], "code_exec");
    assert_eq!(tools[0]["function"]["description"], "Executes code");
    assert_eq!(
        tools[0]["function"]["parameters"],
        json!({
            "type": "object",
            "properties": {
                "input": { "type": "string" }
            },
            "required": ["input"],
            "additionalProperties": false
        })
    );
    assert_eq!(body["tool_choice"]["type"], "function");
    assert_eq!(body["tool_choice"]["function"]["name"], "code_exec");

    let mut allowed_tools_body = json!({
        "model": "gpt-4o",
        "input": "run this",
        "tools": [
            {
                "type": "custom",
                "name": "code_exec",
                "description": "Executes code"
            },
            {
                "type": "function",
                "name": "lookup_weather",
                "parameters": { "type": "object" }
            }
        ],
        "tool_choice": {
            "type": "allowed_tools",
            "mode": "required",
            "tools": [
                { "type": "custom", "name": "code_exec" },
                { "type": "function", "name": "lookup_weather" }
            ]
        }
    });

    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut allowed_tools_body,
        false,
    )
    .unwrap();

    let tools = allowed_tools_body["tools"].as_array().expect("chat tools");
    assert_eq!(tools[0]["type"], "function");
    assert_eq!(tools[0]["function"]["name"], "code_exec");
    assert_eq!(tools[1]["type"], "function");
    assert_eq!(allowed_tools_body["tool_choice"]["type"], "allowed_tools");
    let allowed_tools = allowed_tools_body["tool_choice"]["allowed_tools"]["tools"]
        .as_array()
        .expect("allowed tools");
    assert_eq!(allowed_tools[0]["type"], "function");
    assert_eq!(allowed_tools[0]["function"]["name"], "code_exec");
    assert_eq!(allowed_tools[1]["type"], "function");
    assert_eq!(allowed_tools[1]["function"]["name"], "lookup_weather");
}

#[test]
fn translate_request_responses_to_openai_rejects_same_name_function_and_custom_bridge_conflict() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": "run this",
        "tools": [
            {
                "type": "function",
                "name": "apply_patch",
                "parameters": { "type": "object" }
            },
            {
                "type": "custom",
                "name": "apply_patch",
                "format": { "type": "text" }
            }
        ]
    });

    let err = translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        false,
    )
    .expect_err("same stable function/custom tool names should reject");

    assert!(err.contains("apply_patch"), "err = {err}");
    assert!(err.contains("same stable name"), "err = {err}");
}

#[test]
fn translate_request_responses_to_non_responses_rejects_namespace_tool_groups() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": "run this",
        "tools": [{
            "type": "namespace",
            "name": "crm",
            "description": "CRM tools",
            "tools": [{
                "type": "custom",
                "name": "lookup_account"
            }]
        }]
    });

    let err = translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        false,
    )
    .expect_err("Responses namespace tools should fail closed");

    assert!(err.contains("namespace"), "err = {err}");
}

#[test]
fn translate_request_responses_to_non_responses_rejects_namespaced_tool_calls() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": [{
            "type": "custom_tool_call",
            "call_id": "call_custom",
            "name": "lookup_account",
            "namespace": "crm",
            "input": "account_id=123"
        }]
    });

    let err = translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        false,
    )
    .expect_err("Responses namespaced tool calls should fail closed");

    assert!(err.contains("namespace"), "err = {err}");
}

#[test]
fn translate_request_chat_to_responses_maps_top_level_refusal_to_refusal_part() {
    let mut body = json!({
        "model": "gpt-4o",
        "messages": [{
            "role": "assistant",
            "content": null,
            "refusal": "I can't help with that."
        }]
    });

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        "gpt-4o",
        &mut body,
        false,
    )
    .unwrap();

    let input = body["input"].as_array().expect("responses input");
    assert_eq!(input.len(), 1);
    assert_eq!(input[0]["type"], "message");
    let content = input[0]["content"].as_array().expect("responses content");
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "refusal");
    assert_eq!(content[0]["refusal"], "I can't help with that.");
}

#[test]
fn translate_request_responses_to_openai_maps_refusal_to_top_level_message_field() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": [{
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "refusal", "refusal": "No." }]
        }]
    });

    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        false,
    )
    .unwrap();

    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], "assistant");
    assert_eq!(messages[0]["refusal"], "No.");
    assert!(messages[0]["content"].is_null() || messages[0]["content"] == "");
    assert_ne!(messages[0]["content"][0]["type"], "refusal");
}

#[test]
fn translate_request_gemini_to_openai_maps_audio_inline_data_and_file_parts_without_image_spoofing()
{
    let mut body = json!({
        "model": "gemini-2.5-flash",
        "contents": [{
            "role": "user",
            "parts": [
                { "text": "Inspect these" },
                { "inlineData": { "mimeType": "audio/wav", "data": "AAAA" } },
                { "inlineData": { "mimeType": "application/pdf", "data": "JVBERi0x" } },
                { "fileData": { "mimeType": "application/pdf", "fileUri": "gs://bucket/doc.pdf" } }
            ]
        }]
    });

    translate_request(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiCompletion,
        "gemini-2.5-flash",
        &mut body,
        false,
    )
    .unwrap();

    let messages = body["messages"].as_array().expect("messages");
    let content = messages[0]["content"].as_array().expect("content");
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[1]["type"], "input_audio");
    assert_eq!(content[1]["input_audio"]["data"], "AAAA");
    assert_eq!(content[1]["input_audio"]["format"], "wav");
    assert_eq!(content[2]["type"], "file");
    assert_eq!(
        content[2]["file"]["file_data"],
        "data:application/pdf;base64,JVBERi0x"
    );
    assert_eq!(content[3]["type"], "file");
    assert_eq!(content[3]["file"]["file_data"], "gs://bucket/doc.pdf");
    assert!(content.iter().all(|part| part["type"] != "image_url"));
}

#[test]
fn translate_request_openai_to_gemini_maps_input_audio_and_file_parts() {
    let mut body = json!({
        "model": "gemini-2.5-flash",
        "messages": [{
            "role": "user",
            "content": [
                { "type": "text", "text": "Inspect these" },
                { "type": "input_audio", "input_audio": { "data": "AAAA", "format": "wav" } },
                { "type": "file", "file": { "file_data": "data:application/pdf;base64,JVBERi0x", "filename": "doc.pdf" } }
            ]
        }]
    });

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        "gemini-2.5-flash",
        &mut body,
        false,
    )
    .unwrap();

    let parts = body["contents"][0]["parts"].as_array().expect("parts");
    assert_eq!(parts[0]["text"], "Inspect these");
    assert_eq!(parts[1]["inlineData"]["mimeType"], "audio/wav");
    assert_eq!(parts[1]["inlineData"]["data"], "AAAA");
    assert_eq!(parts[2]["inlineData"]["mimeType"], "application/pdf");
    assert_eq!(parts[2]["inlineData"]["data"], "JVBERi0x");
}

#[test]
fn translate_request_openai_to_gemini_maps_file_uris_to_file_data() {
    let mut body = json!({
        "model": "gemini-2.5-flash",
        "messages": [{
            "role": "user",
            "content": [{ "type": "file", "file": { "file_data": "gs://bucket/doc.pdf", "filename": "doc.pdf" } }]
        }]
    });

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        "gemini-2.5-flash",
        &mut body,
        false,
    )
    .unwrap();

    let parts = body["contents"][0]["parts"].as_array().expect("parts");
    assert_eq!(parts[0]["fileData"]["fileUri"], "gs://bucket/doc.pdf");
    assert_eq!(parts[0]["fileData"]["displayName"], "doc.pdf");
}

#[test]
fn translate_request_openai_to_gemini_maps_plain_base64_file_data_with_filename() {
    let mut body = json!({
        "model": "gemini-2.5-flash",
        "messages": [{
            "role": "user",
            "content": [{ "type": "file", "file": { "file_data": "JVBERi0x", "filename": "doc.pdf" } }]
        }]
    });

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        "gemini-2.5-flash",
        &mut body,
        false,
    )
    .unwrap();

    let parts = body["contents"][0]["parts"].as_array().expect("parts");
    assert_eq!(parts[0]["inlineData"]["mimeType"], "application/pdf");
    assert_eq!(parts[0]["inlineData"]["data"], "JVBERi0x");
}

#[test]
fn translate_request_openai_to_gemini_rejects_plain_base64_file_data_without_mime_or_provenance() {
    let mut body = json!({
        "model": "gemini-2.5-flash",
        "messages": [{
            "role": "user",
            "content": [{ "type": "file", "file": { "file_data": "JVBERi0x" } }]
        }]
    });

    let err = translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        "gemini-2.5-flash",
        &mut body,
        false,
    )
    .expect_err("plain base64 file_data without MIME should fail closed");

    assert!(err.contains("file_data"), "err = {err}");
    assert!(err.contains("Gemini"), "err = {err}");
}

#[test]
fn translate_request_openai_to_gemini_rejects_unmappable_file_references() {
    let mut body = json!({
        "model": "gemini-2.5-flash",
        "messages": [{
            "role": "user",
            "content": [{ "type": "file", "file": { "file_id": "file_123" } }]
        }]
    });

    let err = translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        "gemini-2.5-flash",
        &mut body,
        false,
    )
    .expect_err("unmappable file references should fail closed");

    assert!(err.contains("file"), "err = {err}");
}

#[test]
fn translate_request_openai_to_responses_preserves_multiple_instruction_segments_without_merge() {
    let mut body = json!({
        "model": "gpt-4o",
        "messages": [
            { "role": "system", "content": "System A" },
            { "role": "user", "content": "User 1" },
            { "role": "developer", "content": "Developer B" },
            { "role": "user", "content": "User 2" }
        ]
    });

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        "gpt-4o",
        &mut body,
        false,
    )
    .unwrap();

    assert!(body.get("instructions").is_none(), "body = {body}");
    let input = body["input"].as_array().expect("responses input");
    assert_eq!(input.len(), 4, "input = {input:?}");
    assert_eq!(input[0]["role"], "system");
    assert_eq!(input[0]["content"][0]["text"], "System A");
    assert_eq!(input[1]["role"], "user");
    assert_eq!(input[2]["role"], "developer");
    assert_eq!(input[2]["content"][0]["text"], "Developer B");
    assert_eq!(input[3]["role"], "user");
}

#[test]
fn translate_request_same_format_passthrough() {
    let mut body = json!({ "model": "gpt-4o", "messages": [{ "role": "user", "content": "Hi" }] });
    let orig = body.clone();
    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        true,
    )
    .unwrap();
    assert_eq!(body, orig);
}

#[test]
fn translate_request_openai_same_format_normalizes_developer_role() {
    let mut body = json!({
        "model": "gpt-4o",
        "messages": [
            { "role": "developer", "content": "Follow repo rules." },
            { "role": "user", "content": "Hi" }
        ]
    });
    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        true,
    )
    .unwrap();
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[1]["role"], "user");
}

#[test]
fn translate_request_openai_same_format_minimax_normalizes_developer_role_and_keeps_compat_overrides(
) {
    let mut body = json!({
        "model": "MiniMax-M2.7-highspeed",
        "messages": [
            { "role": "developer", "content": "Follow repo rules." },
            { "role": "assistant", "content": "Hello", "reasoning_content": "internal thinking" }
        ]
    });
    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiCompletion,
        "MiniMax-M2.7-highspeed",
        &mut body,
        true,
    )
    .unwrap();
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(
        messages[1]["reasoning_details"][0]["text"],
        "internal thinking"
    );
    assert_eq!(body["reasoning_split"], true);
    assert_eq!(body["stream_options"]["include_usage"], true);
}

#[test]
fn translate_request_openai_same_format_coalesces_adjacent_string_messages() {
    let mut body = json!({
        "model": "gpt-4o",
        "messages": [
            { "role": "system", "content": "System A" },
            { "role": "developer", "content": "System B" },
            { "role": "user", "content": "User A" },
            { "role": "user", "content": "User B" }
        ]
    });
    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        true,
    )
    .unwrap();
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[0]["content"], "System A\n\nSystem B");
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(messages[1]["content"], "User A\n\nUser B");
}

#[test]
fn translate_request_responses_same_format_normalizes_developer_role() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": [
            { "type": "message", "role": "developer", "content": [{ "type": "input_text", "text": "Follow repo rules." }] },
            { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "Hi" }] }
        ]
    });
    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiResponses,
        "gpt-4o",
        &mut body,
        true,
    )
    .unwrap();
    let input = body["input"].as_array().unwrap();
    assert_eq!(input[0]["role"], "system");
    assert_eq!(input[1]["role"], "user");
}

#[test]
fn translate_request_responses_to_openai() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": [{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "Hello" }] }]
    });
    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        true,
    )
    .unwrap();
    assert!(body.get("messages").is_some());
    assert!(body.get("input").is_none());
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages[0]["content"], "Hello");
}

#[test]
fn translate_request_responses_to_minimax_openai_enables_reasoning_split() {
    let mut body = json!({
        "model": "claude-openai",
        "input": [
            {
                "type": "reasoning",
                "summary": [{ "type": "summary_text", "text": "internal thinking" }]
            },
            {
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": "Hello" }]
            }
        ]
    });
    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "MiniMax-M2.7-highspeed",
        &mut body,
        true,
    )
    .unwrap();
    assert_eq!(body["reasoning_split"], true);
    assert_eq!(body["stream_options"]["include_usage"], true);
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(
        messages[0]["reasoning_details"][0]["text"],
        "internal thinking"
    );
}

#[test]
fn translate_request_responses_to_openai_coalesces_adjacent_string_messages() {
    let mut body = json!({
        "model": "gpt-4o",
        "instructions": "System A",
        "input": [
            { "type": "message", "role": "developer", "content": [{ "type": "input_text", "text": "System B" }] },
            { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "User A" }] },
            { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "User B" }] }
        ]
    });
    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        true,
    )
    .unwrap();
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[0]["content"], "System A\n\nSystem B");
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(messages[1]["content"], "User A\n\nUser B");
}

#[test]
fn translate_request_responses_to_openai_flattens_text_only_content_arrays() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [
                    { "type": "input_text", "text": "Hello " },
                    { "type": "input_text", "text": "world" }
                ]
            }
        ]
    });
    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        true,
    )
    .unwrap();
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages[0]["content"], "Hello world");
}

#[test]
fn translate_request_responses_to_openai_maps_developer_role_to_system() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": [
            { "type": "message", "role": "developer", "content": [{ "type": "input_text", "text": "Follow repo rules." }] },
            { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "Hello" }] }
        ]
    });
    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        true,
    )
    .unwrap();
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[1]["role"], "user");
}

#[test]
fn translate_request_responses_to_openai_preserves_reasoning_items() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": [
            { "type": "reasoning", "summary": [{ "type": "summary_text", "text": "thinking" }] },
            { "type": "message", "role": "assistant", "content": [{ "type": "output_text", "text": "Hi" }] }
        ]
    });
    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        true,
    )
    .unwrap();
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages[0]["role"], "assistant");
    assert_eq!(messages[0]["reasoning_content"], "thinking");
    assert_eq!(messages[0]["content"], "Hi");
}

#[test]
fn translate_request_responses_to_openai_maps_tool_choice_and_parallel_calls() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": "Hello",
        "tool_choice": { "type": "function", "name": "lookup" },
        "parallel_tool_calls": false
    });
    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        true,
    )
    .unwrap();
    assert_eq!(body["tool_choice"]["type"], "function");
    assert_eq!(body["tool_choice"]["function"]["name"], "lookup");
    assert_eq!(body["parallel_tool_calls"], false);
}

#[test]
fn translate_request_responses_to_openai_maps_shared_controls_and_drops_responses_only_fields() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": "Hello",
        "stream": true,
        "max_output_tokens": 123,
        "metadata": { "trace_id": "abc" },
        "user": "user-123",
        "temperature": 0.2,
        "top_p": 0.8,
        "top_logprobs": 5,
        "service_tier": "priority",
        "stream_options": { "include_obfuscation": false },
        "include": ["reasoning.encrypted_content"],
        "text": {
            "format": { "type": "text" },
            "verbosity": "high"
        },
        "reasoning": { "effort": "medium" },
        "max_tool_calls": 2,
        "prompt_cache_key": "cache-key",
        "prompt_cache_retention": "24h",
        "safety_identifier": "safe-user",
        "truncation": "auto"
    });
    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        true,
    )
    .unwrap();
    assert_eq!(body["max_completion_tokens"], 123);
    assert!(body.get("max_output_tokens").is_none());
    assert_eq!(body["metadata"]["trace_id"], "abc");
    assert_eq!(body["user"], "user-123");
    assert_eq!(body["temperature"], 0.2);
    assert_eq!(body["top_p"], 0.8);
    assert_eq!(body["logprobs"], true);
    assert_eq!(body["top_logprobs"], 5);
    assert_eq!(body["service_tier"], "priority");
    assert_eq!(body["stream_options"]["include_obfuscation"], false);
    assert_eq!(body["verbosity"], "high");
    assert_eq!(body["reasoning_effort"], "medium");
    assert_eq!(body["prompt_cache_key"], "cache-key");
    assert_eq!(body["prompt_cache_retention"], "24h");
    assert_eq!(body["safety_identifier"], "safe-user");
    assert_eq!(body["response_format"]["type"], "text");
    assert!(body.get("include").is_none());
    assert!(body.get("text").is_none());
    assert!(body.get("reasoning").is_none());
    assert!(body.get("max_tool_calls").is_none());
    assert!(body.get("truncation").is_none());
}

#[test]
fn translate_request_responses_to_openai_drops_stop_request_extension() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": "Hello",
        "stop": ["END"]
    });

    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        false,
    )
    .unwrap();

    assert!(body.get("stop").is_none(), "body = {body:?}");
}

#[test]
fn translate_request_responses_to_openai_drops_undocumented_sampling_controls() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": "Hello",
        "seed": 42,
        "presence_penalty": 0.7,
        "frequency_penalty": 0.3
    });

    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        false,
    )
    .unwrap();

    for field in ["seed", "presence_penalty", "frequency_penalty"] {
        assert!(
            body.get(field).is_none(),
            "field = {field}, body = {body:?}"
        );
    }
}

#[test]
fn translate_request_responses_to_non_responses_rejects_hosted_tool_choice_selectors() {
    for upstream_format in [UpstreamFormat::OpenAiCompletion, UpstreamFormat::Anthropic] {
        let mut body = json!({
            "model": "gpt-4o",
            "input": "Hello",
            "tool_choice": { "type": "file_search" }
        });

        let err = translate_request(
            UpstreamFormat::OpenAiResponses,
            upstream_format,
            "target-model",
            &mut body,
            false,
        )
        .expect_err("Responses hosted tool_choice selectors should fail closed");

        assert!(err.contains("tool_choice"), "err = {err}");
        assert!(err.contains("file_search"), "err = {err}");
    }
}

#[test]
fn translate_request_responses_to_non_responses_rejects_hosted_tool_items() {
    let item_cases = [
        (
            "file_search_call",
            json!({ "type": "file_search_call", "id": "fsc_1", "queries": ["weather"] }),
        ),
        (
            "computer_call_output",
            json!({
                "type": "computer_call_output",
                "call_id": "comp_1",
                "output": { "type": "image", "image_url": "https://example.com/screenshot.png" }
            }),
        ),
    ];

    for (label, item) in item_cases {
        for upstream_format in [UpstreamFormat::OpenAiCompletion, UpstreamFormat::Anthropic] {
            let mut body = json!({
                "model": "gpt-4o",
                "input": [item.clone()]
            });

            let err = translate_request(
                UpstreamFormat::OpenAiResponses,
                upstream_format,
                "target-model",
                &mut body,
                false,
            )
            .expect_err("Responses hosted input items should fail closed");

            assert!(err.contains(label), "label = {label}, err = {err}");
        }
    }
}

#[test]
fn translate_request_responses_to_non_responses_rejects_item_reference_items() {
    for upstream_format in [UpstreamFormat::OpenAiCompletion, UpstreamFormat::Anthropic] {
        let mut body = json!({
            "model": "gpt-4o",
            "input": [{ "type": "item_reference", "id": "msg_123" }]
        });

        let err = translate_request(
            UpstreamFormat::OpenAiResponses,
            upstream_format,
            "target-model",
            &mut body,
            false,
        )
        .expect_err("Responses item_reference should fail closed cross-protocol");

        assert!(err.contains("item_reference"), "err = {err}");
    }
}

#[test]
fn translate_request_responses_to_openai_downgrades_reasoning_encrypted_content_items() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": [{
            "type": "reasoning",
            "summary": [{ "type": "summary_text", "text": "thinking" }],
            "encrypted_content": "opaque_state"
        }]
    });

    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "target-model",
        &mut body,
        false,
    )
    .expect("Responses reasoning encrypted_content should downgrade on Chat-compatible upstreams");

    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 1, "messages = {messages:?}");
    assert_eq!(messages[0]["role"], "assistant");
    assert_eq!(messages[0]["reasoning_content"], "thinking");
    assert!(
        messages[0].get("_anthropic_reasoning_replay").is_none(),
        "messages = {messages:?}"
    );
}

#[test]
fn translate_request_responses_to_google_rejects_reasoning_encrypted_content_items() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": [{
            "type": "reasoning",
            "summary": [{ "type": "summary_text", "text": "thinking" }],
            "encrypted_content": "enc_123"
        }]
    });

    let err = translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Google,
        "target-model",
        &mut body,
        false,
    )
    .expect_err("Responses reasoning encrypted_content should still fail closed for Gemini");

    assert!(err.contains("encrypted_content"), "err = {err}");
}

#[test]
fn translate_request_responses_to_claude_drops_malformed_reasoning_carrier_but_preserves_summary() {
    let mut body = json!({
        "model": "claude-3",
        "input": [{
            "type": "reasoning",
            "summary": [{ "type": "summary_text", "text": "thinking" }],
            "encrypted_content": { "opaque": "state" }
        }, {
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": "Visible answer" }]
        }]
    });

    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        "claude-3",
        &mut body,
        false,
    )
    .expect("malformed Responses reasoning carrier should not fail closed for Anthropic");

    let messages = body["messages"].as_array().expect("anthropic messages");
    assert_eq!(messages.len(), 1, "messages = {messages:?}");
    assert_eq!(messages[0]["role"], "assistant");
    assert!(messages[0].get("reasoning_content").is_none());
    let assistant_content = messages[0]["content"]
        .as_array()
        .expect("assistant content");
    assert_eq!(assistant_content[0]["type"], "thinking");
    assert_eq!(assistant_content[0]["thinking"], "thinking");
    assert_eq!(assistant_content[1]["type"], "text");
    assert_eq!(assistant_content[1]["text"], "Visible answer");
}

#[test]
fn translate_request_responses_to_openai_preserves_function_tool_strict() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": "Hello",
        "tools": [{
            "type": "function",
            "name": "lookup_weather",
            "description": "Weather lookup",
            "parameters": {
                "type": "object",
                "properties": {
                    "city": { "type": "string" }
                }
            },
            "strict": true
        }]
    });

    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        false,
    )
    .unwrap();

    let tools = body["tools"].as_array().expect("chat tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["function"]["name"], "lookup_weather");
    assert_eq!(tools[0]["function"]["strict"], true);
}

#[test]
fn translate_request_responses_to_openai_rejects_stateful_responses_controls() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": "Hello",
        "previous_response_id": "resp_123"
    });

    let err = translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        true,
    )
    .expect_err("stateful responses controls should fail closed");

    assert_eq!(
            err,
            "Responses request controls `previous_response_id` require a native OpenAI Responses upstream and cannot be translated to openai-completion; the proxy does not reconstruct provider state"
        );
    assert_eq!(body["previous_response_id"], "resp_123");
}

#[test]
fn translate_request_responses_to_openai_preserves_empty_input_and_uses_max_completion_tokens() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": "",
        "max_output_tokens": 123
    });
    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        false,
    )
    .unwrap();

    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"], "");
    assert_eq!(body["max_completion_tokens"], 123);
    assert!(body.get("max_output_tokens").is_none());
}

#[test]
fn translate_request_responses_to_openai_keeps_empty_input_empty() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": ""
    });

    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        false,
    )
    .unwrap();

    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"], "");
}

#[test]
fn translate_request_responses_to_openai_preserves_mid_thread_instruction_segments_in_order() {
    let mut body = json!({
        "model": "MiniMax-M2.7-highspeed",
        "input": [
            { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "Earlier user message" }] },
            { "type": "message", "role": "developer", "content": [{ "type": "input_text", "text": "Compacted thread summary" }] },
            { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "Continue" }] }
        ]
    });
    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "MiniMax-M2.7-highspeed",
        &mut body,
        true,
    )
    .unwrap();
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"], "Earlier user message");
    assert_eq!(messages[1]["role"], "system");
    assert_eq!(messages[1]["content"], "Compacted thread summary");
    assert_eq!(messages[2]["role"], "user");
    assert_eq!(messages[2]["content"], "Continue");
}

#[test]
fn translate_response_openai_reasoning_details_maps_to_responses_reasoning() {
    let body = json!({
        "id": "chatcmpl_1",
        "model": "MiniMax-M2.7-highspeed",
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "Hello",
                "reasoning_details": [{ "text": "internal thinking" }]
            },
            "finish_reason": "stop"
        }]
    });
    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        &body,
    )
    .unwrap();
    let output = out["output"].as_array().unwrap();
    assert_eq!(output[0]["type"], "reasoning");
    assert_eq!(output[0]["summary"][0]["text"], "internal thinking");
    assert_eq!(output[1]["type"], "message");
    assert_eq!(output[1]["content"][0]["text"], "Hello");
}

#[test]
fn translate_response_openai_tool_only_turn_does_not_emit_empty_responses_message() {
    let body = json!({
        "id": "chatcmpl_tool_only",
        "model": "gpt-4o",
        "choices": [{
            "message": {
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "lookup",
                        "arguments": "{\"city\":\"Tokyo\"}"
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });
    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        &body,
    )
    .unwrap();
    let output = out["output"].as_array().expect("responses output");
    assert_eq!(output.len(), 1, "output = {output:?}");
    assert_eq!(output[0]["type"], "function_call");
}

#[test]
fn translate_response_openai_tool_only_turn_to_responses_does_not_create_empty_message_item() {
    let body = json!({
        "id": "chatcmpl_tool_only",
        "model": "gpt-4o",
        "choices": [{
            "message": {
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "lookup",
                        "arguments": "{\"city\":\"Tokyo\"}"
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });

    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        &body,
    )
    .unwrap();

    let output = out["output"].as_array().expect("responses output");
    assert_eq!(output.len(), 1, "output = {output:?}");
    assert_eq!(output[0]["type"], "function_call");
}

#[test]
fn translate_request_openai_to_claude_has_system_and_messages() {
    let mut body = json!({
        "model": "claude-3",
        "messages": [
            { "role": "system", "content": "Sys" },
            { "role": "user", "content": "Hi" }
        ]
    });
    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        "claude-3",
        &mut body,
        true,
    )
    .unwrap();
    // System should be array with cache_control on last block
    let system = body
        .get("system")
        .and_then(Value::as_array)
        .expect("system should be array");
    assert!(!system.is_empty());
    assert_eq!(system[0]["text"], "Sys");
    assert!(body.get("messages").is_some());
    assert!(!body["messages"].as_array().unwrap().is_empty());
}

#[test]
fn translate_request_openai_to_claude_preserves_unsigned_thinking_without_replay_provenance() {
    let mut body = json!({
        "model": "claude-3",
        "messages": [
            {
                "role": "assistant",
                "reasoning_content": "private reasoning",
                "content": "Visible answer"
            }
        ]
    });
    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        "claude-3",
        &mut body,
        false,
    )
    .expect("reasoning text should degrade to unsigned Anthropic thinking");

    let messages = body["messages"].as_array().expect("anthropic messages");
    assert_eq!(messages.len(), 1, "messages = {messages:?}");
    let assistant_content = messages[0]["content"]
        .as_array()
        .expect("assistant content");
    assert_eq!(assistant_content[0]["type"], "thinking");
    assert_eq!(assistant_content[0]["thinking"], "private reasoning");
    assert!(assistant_content[0].get("signature").is_none());
    assert_eq!(assistant_content[1]["type"], "text");
    assert_eq!(assistant_content[1]["text"], "Visible answer");
    assert!(assistant_content[0].get("cache_control").is_none());
}

#[test]
fn translate_request_openai_reasoning_and_tool_calls_to_claude_preserves_unsigned_thinking_without_replay_provenance(
) {
    let mut body = json!({
        "model": "claude-3",
        "messages": [{
            "role": "assistant",
            "reasoning_content": "internal chain of thought",
            "content": "Calling tool.",
            "tool_calls": [{
                "id": "call_lookup",
                "type": "function",
                "function": {
                    "name": "lookup",
                    "arguments": "{\"city\":\"Tokyo\"}"
                }
            }]
        }]
    });

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        "claude-3",
        &mut body,
        false,
    )
    .expect("reasoning text should preserve tool semantics on Anthropic");

    let messages = body["messages"].as_array().expect("anthropic messages");
    let assistant_content = messages[0]["content"]
        .as_array()
        .expect("assistant content");
    assert_eq!(assistant_content[0]["type"], "thinking");
    assert_eq!(
        assistant_content[0]["thinking"],
        "internal chain of thought"
    );
    assert!(assistant_content[0].get("signature").is_none());
    assert_eq!(assistant_content[1]["type"], "text");
    assert_eq!(assistant_content[1]["text"], "Calling tool.");
    assert_eq!(assistant_content[2]["type"], "tool_use");
    assert_eq!(assistant_content[2]["id"], "call_lookup");
    assert_eq!(assistant_content[2]["name"], "lookup");
    assert_eq!(assistant_content[2]["input"]["city"], "Tokyo");
}

#[test]
fn translate_request_gemini_to_claude_preserves_thought_history_as_unsigned_thinking() {
    let mut body = json!({
        "model": "claude-3",
        "contents": [
            { "role": "user", "parts": [{ "text": "Think about 2+2" }] },
            {
                "role": "model",
                "parts": [
                    { "thought": true, "text": "2+2 equals 4" },
                    { "text": "The answer is 4" }
                ]
            },
            { "role": "user", "parts": [{ "text": "Now what about 3+3?" }] }
        ]
    });

    translate_request(
        UpstreamFormat::Google,
        UpstreamFormat::Anthropic,
        "claude-3",
        &mut body,
        false,
    )
    .expect("Gemini thought history should preserve unsigned Anthropic thinking");

    let messages = body["messages"].as_array().expect("anthropic messages");
    assert_eq!(messages.len(), 3, "messages = {messages:?}");
    assert_eq!(messages[1]["role"], "assistant");
    let assistant_content = messages[1]["content"]
        .as_array()
        .expect("assistant content");
    assert_eq!(assistant_content[0]["type"], "thinking");
    assert_eq!(assistant_content[0]["thinking"], "2+2 equals 4");
    assert!(assistant_content[0].get("signature").is_none());
    assert_eq!(assistant_content[1]["type"], "text");
    assert_eq!(assistant_content[1]["text"], "The answer is 4");
}

#[test]
fn translate_request_openai_to_claude_maps_tool_choice_and_parallel_calls() {
    let mut body = json!({
        "model": "claude-3",
        "messages": [{ "role": "user", "content": "Hi" }],
        "tool_choice": { "type": "function", "function": { "name": "lookup" } },
        "parallel_tool_calls": false
    });
    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        "claude-3",
        &mut body,
        true,
    )
    .unwrap();
    assert_eq!(body["tool_choice"]["type"], "tool");
    assert_eq!(body["tool_choice"]["name"], "lookup");
    assert_eq!(body["tool_choice"]["disable_parallel_tool_use"], true);
}

#[test]
fn translate_request_openai_allowed_tools_to_claude_filters_function_subset() {
    let mut body = json!({
        "model": "claude-3",
        "messages": [{ "role": "user", "content": "Hi" }],
        "tools": [
            {
                "type": "function",
                "function": {
                    "name": "lookup_weather",
                    "parameters": { "type": "object", "properties": {} }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "lookup_time",
                    "parameters": { "type": "object", "properties": {} }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "lookup_news",
                    "parameters": { "type": "object", "properties": {} }
                }
            }
        ],
        "tool_choice": {
            "type": "allowed_tools",
            "allowed_tools": {
                "mode": "required",
                "tools": [
                    { "type": "function", "function": { "name": "lookup_weather" } },
                    { "type": "function", "function": { "name": "lookup_time" } }
                ]
            }
        }
    });

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        "claude-3",
        &mut body,
        false,
    )
    .unwrap();

    assert_eq!(body["tool_choice"]["type"], "any");
    let tools = body["tools"].as_array().expect("claude tools");
    assert_eq!(tools.len(), 2, "body = {body:?}");
    assert_eq!(tools[0]["name"], "lookup_weather");
    assert_eq!(tools[1]["name"], "lookup_time");
}

#[test]
fn translate_request_responses_allowed_tools_to_claude_filters_function_subset() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": "Hi",
        "tools": [
            {
                "type": "function",
                "name": "lookup_weather",
                "parameters": { "type": "object", "properties": {} }
            },
            {
                "type": "function",
                "name": "lookup_time",
                "parameters": { "type": "object", "properties": {} }
            },
            {
                "type": "function",
                "name": "lookup_news",
                "parameters": { "type": "object", "properties": {} }
            }
        ],
        "tool_choice": {
            "type": "allowed_tools",
            "mode": "auto",
            "tools": [
                { "type": "function", "name": "lookup_weather" },
                { "type": "function", "name": "lookup_time" }
            ]
        }
    });

    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        "claude-3",
        &mut body,
        false,
    )
    .unwrap();

    assert_eq!(body["tool_choice"]["type"], "auto");
    let tools = body["tools"].as_array().expect("claude tools");
    assert_eq!(tools.len(), 2, "body = {body:?}");
    assert_eq!(tools[0]["name"], "lookup_weather");
    assert_eq!(tools[1]["name"], "lookup_time");
}

#[test]
fn translate_request_openai_to_claude_preserves_top_p_stop_and_metadata() {
    let mut body = json!({
        "model": "claude-3",
        "messages": [{ "role": "user", "content": "Hi" }],
        "top_p": 0.7,
        "stop": ["END"],
        "metadata": { "trace_id": "abc" }
    });
    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        "claude-3",
        &mut body,
        true,
    )
    .unwrap();
    assert_eq!(body["top_p"], 0.7);
    assert_eq!(body["stop_sequences"][0], "END");
    assert_eq!(body["metadata"]["trace_id"], "abc");
}

#[test]
fn translate_request_openai_to_gemini_has_contents() {
    let mut body = json!({
        "model": "gemini-1.5",
        "messages": [
            { "role": "system", "content": "Helper" },
            { "role": "user", "content": "Hi" }
        ]
    });
    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        "gemini-1.5",
        &mut body,
        true,
    )
    .unwrap();
    assert!(body.get("contents").is_some());
    assert!(body.get("systemInstruction").is_some());
}

#[test]
fn translate_request_openai_to_gemini_preserves_function_response_name() {
    let mut body = json!({
        "model": "gemini-1.5",
        "messages": [
            { "role": "user", "content": "Hi" },
            {
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": { "name": "lookup_weather", "arguments": "{\"city\":\"Tokyo\"}" }
                }]
            },
            {
                "role": "tool",
                "tool_call_id": "call_1",
                "content": "{\"temperature\":22}"
            }
        ]
    });
    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        "gemini-3-pro",
        &mut body,
        false,
    )
    .unwrap();
    assert_eq!(
        body["contents"][2]["parts"][0]["functionResponse"]["id"],
        "call_1"
    );
    assert_eq!(
        body["contents"][2]["parts"][0]["functionResponse"]["name"],
        "lookup_weather"
    );
}

#[test]
fn translate_request_openai_to_gemini_merges_parallel_function_responses_in_original_call_order() {
    let mut body = json!({
        "model": "gemini-1.5",
        "messages": [
            { "role": "user", "content": "Hi" },
            {
                "role": "assistant",
                "tool_calls": [
                    {
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "first", "arguments": "{\"step\":1}" }
                    },
                    {
                        "id": "call_2",
                        "type": "function",
                        "function": { "name": "second", "arguments": "{\"step\":2}" }
                    }
                ]
            },
            {
                "role": "tool",
                "tool_call_id": "call_2",
                "content": "{\"ok\":2}"
            },
            {
                "role": "tool",
                "tool_call_id": "call_1",
                "content": "{\"ok\":1}"
            }
        ]
    });
    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        "gemini-3-pro",
        &mut body,
        false,
    )
    .unwrap();

    let contents = body["contents"].as_array().expect("gemini contents");
    assert_eq!(contents.len(), 3, "contents = {contents:?}");
    let responses = contents[2]["parts"].as_array().expect("function responses");
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0]["functionResponse"]["id"], "call_1");
    assert_eq!(responses[1]["functionResponse"]["id"], "call_2");
}

#[test]
fn translate_request_openai_to_gemini_maps_tool_choice_and_allowlist() {
    let mut body = json!({
        "model": "gemini-1.5",
        "messages": [{ "role": "user", "content": "Hi" }],
        "tools": [
            {
                "type": "function",
                "function": {
                    "name": "lookup_weather",
                    "parameters": { "type": "object", "properties": {} }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "lookup_time",
                    "parameters": { "type": "object", "properties": {} }
                }
            }
        ],
        "tool_choice": "required",
        "allowed_tool_names": ["lookup_weather", "lookup_time"]
    });
    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        "gemini-3-pro",
        &mut body,
        false,
    )
    .unwrap();

    assert_eq!(body["toolConfig"]["functionCallingConfig"]["mode"], "ANY");
    assert!(
        body["toolConfig"]["functionCallingConfig"]
            .get("allowedFunctionNames")
            .is_none(),
        "body = {body:?}"
    );
    let declarations = body["tools"][0]["functionDeclarations"]
        .as_array()
        .expect("function declarations");
    assert_eq!(declarations.len(), 2, "body = {body:?}");
    assert_eq!(declarations[0]["name"], "lookup_weather");
    assert_eq!(declarations[1]["name"], "lookup_time");
}

#[test]
fn translate_request_openai_allowed_tools_to_gemini_filters_function_subset() {
    let mut body = json!({
        "model": "gpt-4o",
        "messages": [{ "role": "user", "content": "Hi" }],
        "tools": [
            {
                "type": "function",
                "function": {
                    "name": "lookup_weather",
                    "parameters": { "type": "object", "properties": {} }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "lookup_time",
                    "parameters": { "type": "object", "properties": {} }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "lookup_news",
                    "parameters": { "type": "object", "properties": {} }
                }
            }
        ],
        "tool_choice": {
            "type": "allowed_tools",
            "allowed_tools": {
                "mode": "required",
                "tools": [
                    { "type": "function", "function": { "name": "lookup_weather" } },
                    { "type": "function", "function": { "name": "lookup_time" } }
                ]
            }
        }
    });

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        "gemini-3-pro",
        &mut body,
        false,
    )
    .unwrap();

    assert_eq!(body["toolConfig"]["functionCallingConfig"]["mode"], "ANY");
    assert!(
        body["toolConfig"]["functionCallingConfig"]
            .get("allowedFunctionNames")
            .is_none(),
        "body = {body:?}"
    );
    let declarations = body["tools"][0]["functionDeclarations"]
        .as_array()
        .expect("function declarations");
    assert_eq!(declarations.len(), 2, "body = {body:?}");
    assert_eq!(declarations[0]["name"], "lookup_weather");
    assert_eq!(declarations[1]["name"], "lookup_time");
}

#[test]
fn translate_request_responses_allowed_tools_to_gemini_filters_function_subset() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": "Hi",
        "tools": [
            {
                "type": "function",
                "name": "lookup_weather",
                "parameters": { "type": "object", "properties": {} }
            },
            {
                "type": "function",
                "name": "lookup_time",
                "parameters": { "type": "object", "properties": {} }
            },
            {
                "type": "function",
                "name": "lookup_news",
                "parameters": { "type": "object", "properties": {} }
            }
        ],
        "tool_choice": {
            "type": "allowed_tools",
            "mode": "auto",
            "tools": [
                { "type": "function", "name": "lookup_weather" },
                { "type": "function", "name": "lookup_time" }
            ]
        }
    });

    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Google,
        "gemini-3-pro",
        &mut body,
        false,
    )
    .unwrap();

    assert_eq!(body["toolConfig"]["functionCallingConfig"]["mode"], "AUTO");
    let declarations = body["tools"][0]["functionDeclarations"]
        .as_array()
        .expect("function declarations");
    assert_eq!(declarations.len(), 2, "body = {body:?}");
    assert_eq!(declarations[0]["name"], "lookup_weather");
    assert_eq!(declarations[1]["name"], "lookup_time");
}

#[test]
fn translate_request_allowed_tools_to_non_gemini_targets_rejects_unresolved_selector() {
    for upstream_format in [UpstreamFormat::Google, UpstreamFormat::Anthropic] {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "Hi" }],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "lookup_weather",
                    "parameters": { "type": "object", "properties": {} }
                }
            }],
            "tool_choice": {
                "type": "allowed_tools",
                "allowed_tools": {
                    "mode": "required",
                    "tools": [
                        { "type": "function", "function": { "name": "lookup_time" } }
                    ]
                }
            }
        });

        let err = translate_request(
            UpstreamFormat::OpenAiCompletion,
            upstream_format,
            "target-model",
            &mut body,
            false,
        )
        .expect_err("unresolved allowed_tools selector should fail closed");

        assert!(err.contains("allowed_tools"), "err = {err}");
        assert!(err.contains("lookup_time"), "err = {err}");
    }
}

#[test]
fn translate_request_allowed_tools_to_non_gemini_targets_rejects_non_function_selection() {
    for upstream_format in [UpstreamFormat::Google, UpstreamFormat::Anthropic] {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "Hi" }],
            "tool_choice": {
                "type": "allowed_tools",
                "allowed_tools": {
                    "mode": "required",
                    "tools": [
                        { "type": "custom", "name": "code_exec" }
                    ]
                }
            }
        });

        let err = translate_request(
            UpstreamFormat::OpenAiCompletion,
            upstream_format,
            "target-model",
            &mut body,
            false,
        )
        .expect_err("non-function allowed_tools selection should fail closed");

        assert!(
            err.contains("allowed_tools")
                || err.contains("custom tools")
                || err.contains("custom tool"),
            "err = {err}"
        );
    }
}

#[test]
fn translate_request_openai_to_gemini_maps_tool_choice_modes() {
    let cases = [
        (json!("auto"), json!({ "mode": "AUTO" })),
        (json!("none"), json!({ "mode": "NONE" })),
        (json!("required"), json!({ "mode": "ANY" })),
        (
            json!({ "type": "function", "function": { "name": "lookup_weather" } }),
            json!({
                "mode": "ANY",
                "allowedFunctionNames": ["lookup_weather"]
            }),
        ),
    ];

    for (tool_choice, expected) in cases {
        let mut body = json!({
            "model": "gemini-1.5",
            "messages": [{ "role": "user", "content": "Hi" }],
            "tool_choice": tool_choice
        });
        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-1.5",
            &mut body,
            false,
        )
        .unwrap();

        assert_eq!(body["toolConfig"]["functionCallingConfig"], expected);
    }
}

#[test]
fn translate_request_openai_to_gemini_maps_logprobs_controls() {
    let mut body = json!({
        "model": "gemini-1.5",
        "messages": [{ "role": "user", "content": "Hi" }],
        "logprobs": true,
        "top_logprobs": 5
    });

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        "gemini-1.5",
        &mut body,
        false,
    )
    .unwrap();

    assert_eq!(body["generationConfig"]["responseLogprobs"], true);
    assert_eq!(body["generationConfig"]["logprobs"], 5);
}

#[test]
fn translate_request_openai_to_gemini_does_not_attach_allowlist_for_auto_or_none() {
    let cases = [("auto", "AUTO"), ("none", "NONE")];

    for (tool_choice, expected_mode) in cases {
        let mut body = json!({
            "model": "gemini-1.5",
            "messages": [{ "role": "user", "content": "Hi" }],
            "tool_choice": tool_choice,
            "allowed_tool_names": ["lookup_weather", "lookup_time"]
        });
        translate_request(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Google,
            "gemini-1.5",
            &mut body,
            false,
        )
        .unwrap();

        let config = &body["toolConfig"]["functionCallingConfig"];
        assert_eq!(config["mode"], expected_mode);
        assert!(
            config.get("allowedFunctionNames").is_none(),
            "config = {config:?}"
        );
    }
}

#[test]
fn translate_request_openai_to_gemini_tool_turns_use_dummy_signature_without_reasoning_replay() {
    let mut body = json!({
        "model": "gemini-1.5",
        "messages": [
            { "role": "user", "content": "Hi" },
            {
                "role": "assistant",
                "reasoning_content": "internal reasoning",
                "content": "Calling tool.",
                "tool_calls": [
                    {
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "lookup_weather", "arguments": "{\"city\":\"Tokyo\"}" }
                    },
                    {
                        "id": "call_2",
                        "type": "function",
                        "function": { "name": "lookup_time", "arguments": "{\"city\":\"Tokyo\"}" }
                    }
                ]
            },
            {
                "role": "tool",
                "tool_call_id": "call_1",
                "content": "{\"temperature\":22}"
            },
            {
                "role": "tool",
                "tool_call_id": "call_2",
                "content": "{\"time\":\"10:00\"}"
            }
        ]
    });
    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        "gemini-3-pro",
        &mut body,
        false,
    )
    .unwrap();
    let assistant_parts = body["contents"][1]["parts"]
        .as_array()
        .expect("assistant parts");
    assert!(assistant_parts.iter().all(|part| part["thought"] != true));
    assert_eq!(assistant_parts[0]["text"], "Calling tool.");
    assert!(assistant_parts[1].get("functionCall").is_some());
    assert_eq!(
        assistant_parts[1]["thoughtSignature"],
        "skip_thought_signature_validator"
    );
    assert!(assistant_parts[2].get("functionCall").is_some());
    assert!(assistant_parts[2].get("thoughtSignature").is_none());
}

#[test]
fn translate_request_openai_to_gemini_non_tool_assistant_turn_keeps_reasoning_as_thought() {
    let mut body = json!({
        "model": "gemini-1.5",
        "messages": [
            { "role": "user", "content": "Hi" },
            {
                "role": "assistant",
                "reasoning_content": "internal reasoning",
                "content": "Visible answer"
            }
        ]
    });
    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        "gemini-3-pro",
        &mut body,
        false,
    )
    .unwrap();
    let assistant_parts = body["contents"][1]["parts"]
        .as_array()
        .expect("assistant parts");
    assert_eq!(assistant_parts[0]["thought"], true);
    assert_eq!(assistant_parts[0]["text"], "internal reasoning");
    assert_eq!(assistant_parts[1]["text"], "Visible answer");
}

#[test]
fn translate_request_openai_to_claude_omitted_stream_defaults_false() {
    let mut body = json!({
        "model": "claude-3",
        "messages": [{ "role": "user", "content": "Hi" }]
    });
    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        "claude-3",
        &mut body,
        false,
    )
    .unwrap();
    assert_eq!(body["stream"], false);
}

#[test]
fn translate_request_openai_to_responses_maps_tool_choice() {
    let mut body = json!({
        "model": "gpt-4o",
        "messages": [{ "role": "user", "content": "Hi" }],
        "tool_choice": { "type": "function", "function": { "name": "lookup" } },
        "parallel_tool_calls": false
    });
    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        "gpt-4o",
        &mut body,
        true,
    )
    .unwrap();
    assert_eq!(body["tool_choice"]["type"], "function");
    assert_eq!(body["tool_choice"]["name"], "lookup");
    assert_eq!(body["parallel_tool_calls"], false);
}

#[test]
fn translate_request_openai_to_responses_maps_shared_controls_and_normalizes_legacy_allowlist() {
    let mut body = json!({
        "model": "gpt-4o",
        "messages": [{ "role": "user", "content": "Hi" }],
        "metadata": { "trace_id": "abc" },
        "user": "user-123",
        "service_tier": "priority",
        "stream_options": { "include_obfuscation": true },
        "verbosity": "low",
        "reasoning_effort": "high",
        "tool_choice": "required",
        "allowed_tool_names": ["lookup_weather", "lookup_time"],
        "tools": [
            {
                "type": "function",
                "function": {
                    "name": "lookup_weather",
                    "parameters": { "type": "object", "properties": {} }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "lookup_time",
                    "parameters": { "type": "object", "properties": {} }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "lookup_news",
                    "parameters": { "type": "object", "properties": {} }
                }
            }
        ]
    });

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        "gpt-4o",
        &mut body,
        false,
    )
    .unwrap();

    assert_eq!(body["metadata"]["trace_id"], "abc");
    assert_eq!(body["user"], "user-123");
    assert_eq!(body["service_tier"], "priority");
    assert_eq!(body["stream_options"]["include_obfuscation"], true);
    assert_eq!(body["text"]["verbosity"], "low");
    assert_eq!(body["reasoning"]["effort"], "high");
    assert!(body.get("allowed_tool_names").is_none(), "body = {body:?}");
    assert_eq!(body["tool_choice"]["type"], "allowed_tools");
    assert_eq!(body["tool_choice"]["mode"], "required");
    let tools = body["tool_choice"]["tools"]
        .as_array()
        .expect("allowed tools");
    assert_eq!(tools.len(), 2, "body = {body:?}");
    assert_eq!(tools[0]["type"], "function");
    assert_eq!(tools[0]["name"], "lookup_weather");
    assert_eq!(tools[1]["name"], "lookup_time");
}

#[test]
fn translate_request_openai_custom_tool_to_responses_preserves_custom_type() {
    let mut body = json!({
        "model": "gpt-4o",
        "messages": [{ "role": "user", "content": "Hi" }],
        "tools": [{
            "type": "custom",
            "custom": {
                "name": "code_exec",
                "description": "Executes code with provider-managed semantics",
                "format": { "type": "text" }
            }
        }]
    });

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        "gpt-4o",
        &mut body,
        false,
    )
    .unwrap();

    let tools = body["tools"].as_array().expect("responses tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["type"], "custom");
    assert_eq!(tools[0]["name"], "code_exec");
    assert_eq!(
        tools[0]["description"],
        "Executes code with provider-managed semantics"
    );
    assert_eq!(tools[0]["format"]["type"], "text");
}

#[test]
fn translate_request_openai_custom_tool_choice_to_responses_preserves_custom_type() {
    let mut body = json!({
        "model": "gpt-4o",
        "messages": [{ "role": "user", "content": "Hi" }],
        "tools": [{
            "type": "custom",
            "custom": {
                "name": "code_exec",
                "description": "Executes code with provider-managed semantics",
                "format": { "type": "text" }
            }
        }],
        "tool_choice": {
            "type": "custom",
            "custom": {
                "name": "code_exec"
            }
        }
    });

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        "gpt-4o",
        &mut body,
        false,
    )
    .unwrap();

    assert_eq!(body["tool_choice"]["type"], "custom");
    assert_eq!(body["tool_choice"]["name"], "code_exec");
}

#[test]
fn translate_request_openai_history_custom_tool_call_to_responses_preserves_custom_type() {
    let mut body = json!({
        "model": "gpt-4o",
        "messages": [{
            "role": "assistant",
            "tool_calls": [{
                "id": "call_1",
                "type": "custom",
                "custom": {
                    "name": "code_exec",
                    "input": "print('hi')"
                }
            }]
        }]
    });

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        "gpt-4o",
        &mut body,
        false,
    )
    .unwrap();

    let input = body["input"].as_array().expect("responses input");
    assert_eq!(input.len(), 1);
    assert_eq!(input[0]["type"], "custom_tool_call");
    assert_eq!(input[0]["call_id"], "call_1");
    assert_eq!(input[0]["name"], "code_exec");
    assert_eq!(input[0]["input"], "print('hi')");
}

#[test]
fn translate_request_openai_tool_text_part_arrays_to_responses_output_arrays() {
    let mut body = json!({
        "model": "gpt-4o",
        "messages": [
            {
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "lookup_weather",
                        "arguments": "{\"city\":\"Tokyo\"}"
                    }
                }]
            },
            {
                "role": "tool",
                "tool_call_id": "call_1",
                "content": [
                    { "type": "text", "text": "Sunny" },
                    { "type": "text", "text": " 24C" }
                ]
            }
        ]
    });

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        "gpt-4o",
        &mut body,
        false,
    )
    .unwrap();

    let input = body["input"].as_array().expect("responses input");
    let output = input[1]["output"].as_array().expect("tool output");
    assert_eq!(input[1]["type"], "function_call_output");
    assert_eq!(output.len(), 2);
    assert_eq!(output[0]["type"], "input_text");
    assert_eq!(output[0]["text"], "Sunny");
    assert_eq!(output[1]["type"], "input_text");
    assert_eq!(output[1]["text"], " 24C");
}

#[test]
fn translate_request_openai_custom_allowed_tools_to_responses_preserves_custom_shape() {
    let mut body = json!({
        "model": "gpt-4o",
        "messages": [{ "role": "user", "content": "Hi" }],
        "tools": [
            {
                "type": "custom",
                "custom": {
                    "name": "code_exec",
                    "description": "Executes code"
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "lookup_weather",
                    "parameters": { "type": "object" }
                }
            }
        ],
        "tool_choice": {
            "type": "allowed_tools",
            "allowed_tools": {
                "mode": "required",
                "tools": [
                    {
                        "type": "custom",
                        "custom": { "name": "code_exec" }
                    },
                    {
                        "type": "function",
                        "function": { "name": "lookup_weather" }
                    }
                ]
            }
        }
    });

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        "gpt-4o",
        &mut body,
        false,
    )
    .unwrap();

    assert_eq!(body["tool_choice"]["type"], "allowed_tools");
    let tools = body["tool_choice"]["tools"]
        .as_array()
        .expect("allowed tools");
    assert_eq!(tools[0]["type"], "custom");
    assert_eq!(tools[0]["name"], "code_exec");
    assert_eq!(tools[1]["type"], "function");
    assert_eq!(tools[1]["name"], "lookup_weather");
}

#[test]
fn translate_request_openai_to_responses_maps_logprobs_to_include_and_drops_chat_only_controls() {
    let mut body = json!({
        "model": "gpt-4o",
        "messages": [{ "role": "user", "content": "Hi" }],
        "logprobs": true,
        "top_logprobs": 5,
        "prediction": {
            "type": "content",
            "content": "predicted"
        },
        "web_search_options": {
            "search_context_size": "high"
        },
        "n": 1
    });

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        "gpt-4o",
        &mut body,
        false,
    )
    .unwrap();

    let include = body["include"].as_array().expect("responses include");
    assert!(
        include
            .iter()
            .any(|item| item == "message.output_text.logprobs"),
        "body = {body:?}"
    );
    assert_eq!(body["top_logprobs"], 5);
    assert!(body.get("logprobs").is_none(), "body = {body:?}");
    assert!(body.get("prediction").is_none(), "body = {body:?}");
    assert!(body.get("web_search_options").is_none(), "body = {body:?}");
    assert!(body.get("n").is_none(), "body = {body:?}");
}

#[test]
fn translate_request_openai_history_custom_tool_call_to_gemini_rejects() {
    let mut body = json!({
        "model": "gemini-2.5-flash",
        "messages": [{
            "role": "assistant",
            "tool_calls": [{
                "id": "call_1",
                "type": "custom",
                "custom": {
                    "name": "code_exec",
                    "input": "print('hi')"
                }
            }]
        }]
    });

    let err = translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        "gemini-2.5-flash",
        &mut body,
        false,
    )
    .expect_err("custom tool calls in history should fail closed");

    assert_eq!(
            err,
            "OpenAI custom tools cannot be faithfully translated to Gemini; refusing to downgrade them to function tools"
        );
}

#[test]
fn translate_request_openai_custom_tool_to_anthropic_rejects() {
    let mut body = json!({
        "model": "claude-3",
        "messages": [{ "role": "user", "content": "Hi" }],
        "tools": [{
            "type": "custom",
            "custom": {
                "name": "code_exec",
                "description": "Executes code with provider-managed semantics"
            }
        }]
    });

    let err = translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        "claude-3",
        &mut body,
        false,
    )
    .expect_err("custom tools should fail closed for anthropic");

    assert_eq!(
            err,
            "OpenAI custom tools cannot be faithfully translated to Anthropic; refusing to downgrade them to function tools"
        );
}

#[test]
fn assess_request_translation_responses_custom_tool_to_anthropic_rejects_nonportable_format() {
    let body = json!({
        "model": "claude-3-7-sonnet",
        "tools": [{
            "type": "custom",
            "name": "code_exec",
            "description": "Executes code",
            "format": {
                "type": "grammar",
                "syntax": "lark"
            }
        }],
        "tool_choice": {
            "type": "custom",
            "name": "code_exec"
        },
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": "run this" }]
        }]
    });

    let assessment = assess_request_translation(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        &body,
    );

    let TranslationDecision::Reject(err) = assessment.decision() else {
        panic!("expected reject policy, got {assessment:?}");
    };
    assert!(
        err.contains("custom tool") && err.contains("format") && err.contains("Anthropic"),
        "err = {err}"
    );
}

#[test]
fn assess_request_translation_responses_custom_tool_to_anthropic_strict_rejects_any_bridge() {
    let body = json!({
        "model": "claude-3-7-sonnet",
        "tools": [{
            "type": "custom",
            "name": "code_exec",
            "description": "Executes code",
            "format": { "type": "text" }
        }],
        "tool_choice": {
            "type": "custom",
            "name": "code_exec"
        },
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": "run this" }]
        }]
    });

    let assessment = super::assessment::assess_request_translation_with_compatibility_mode(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        &body,
        crate::config::CompatibilityMode::Strict,
    );

    let TranslationDecision::Reject(err) = assessment.decision() else {
        panic!("expected reject policy, got {assessment:?}");
    };
    assert_eq!(
        err,
        custom_tools_not_portable_message(UpstreamFormat::Anthropic)
    );
}

#[test]
fn assess_request_translation_responses_custom_tool_to_anthropic_balanced_allows_plain_text_but_rejects_grammar(
) {
    let plain_text_body = json!({
        "model": "claude-3-7-sonnet",
        "tools": [{
            "type": "custom",
            "name": "code_exec",
            "description": "Executes code",
            "format": { "type": "text" }
        }],
        "tool_choice": {
            "type": "custom",
            "name": "code_exec"
        },
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": "run this" }]
        }]
    });
    let plain_text_assessment =
        super::assessment::assess_request_translation_with_compatibility_mode(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Anthropic,
            &plain_text_body,
            crate::config::CompatibilityMode::Balanced,
        );
    assert_eq!(plain_text_assessment.decision(), TranslationDecision::Allow);

    let grammar_body = json!({
        "model": "claude-3-7-sonnet",
        "tools": [{
            "type": "custom",
            "name": "apply_patch",
            "description": "Apply a patch",
            "format": {
                "type": "grammar",
                "syntax": "lark",
                "definition": "start: /.+/"
            }
        }],
        "tool_choice": {
            "type": "custom",
            "name": "apply_patch"
        },
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": "create hello.txt" }]
        }]
    });
    let grammar_assessment = super::assessment::assess_request_translation_with_compatibility_mode(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        &grammar_body,
        crate::config::CompatibilityMode::Balanced,
    );

    let TranslationDecision::Reject(err) = grammar_assessment.decision() else {
        panic!("expected reject policy, got {grammar_assessment:?}");
    };
    assert!(
        err.contains("apply_patch") && err.contains("Anthropic"),
        "err = {err}"
    );
}

#[test]
fn assess_request_translation_responses_custom_tool_to_openai_strict_rejects_any_bridge() {
    let body = json!({
        "model": "gpt-4o",
        "tools": [{
            "type": "custom",
            "name": "code_exec",
            "description": "Executes code",
            "format": { "type": "text" }
        }],
        "tool_choice": {
            "type": "custom",
            "name": "code_exec"
        },
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": "run this" }]
        }]
    });

    let assessment = super::assessment::assess_request_translation_with_compatibility_mode(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        &body,
        crate::config::CompatibilityMode::Strict,
    );

    let TranslationDecision::Reject(err) = assessment.decision() else {
        panic!("expected reject policy, got {assessment:?}");
    };
    assert_eq!(
        err,
        custom_tools_not_portable_message(UpstreamFormat::OpenAiCompletion)
    );
}

#[test]
fn assess_request_translation_responses_custom_tool_to_openai_balanced_allows_plain_text_but_rejects_grammar(
) {
    let plain_text_body = json!({
        "model": "gpt-4o",
        "tools": [{
            "type": "custom",
            "name": "code_exec",
            "description": "Executes code",
            "format": { "type": "text" }
        }],
        "tool_choice": {
            "type": "custom",
            "name": "code_exec"
        },
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": "run this" }]
        }]
    });
    let plain_text_assessment =
        super::assessment::assess_request_translation_with_compatibility_mode(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::OpenAiCompletion,
            &plain_text_body,
            crate::config::CompatibilityMode::Balanced,
        );
    assert_eq!(plain_text_assessment.decision(), TranslationDecision::Allow);

    let grammar_body = json!({
        "model": "gpt-4o",
        "tools": [{
            "type": "custom",
            "name": "apply_patch",
            "description": "Apply a patch",
            "format": {
                "type": "grammar",
                "syntax": "lark",
                "definition": "start: /.+/"
            }
        }],
        "tool_choice": {
            "type": "custom",
            "name": "apply_patch"
        },
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": "create hello.txt" }]
        }]
    });
    let grammar_assessment = super::assessment::assess_request_translation_with_compatibility_mode(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        &grammar_body,
        crate::config::CompatibilityMode::Balanced,
    );

    let TranslationDecision::Reject(err) = grammar_assessment.decision() else {
        panic!("expected reject policy, got {grammar_assessment:?}");
    };
    assert!(
        err.contains("apply_patch") && err.contains("OpenAI Chat Completions"),
        "err = {err}"
    );
}

#[test]
fn assess_request_translation_responses_custom_tool_to_gemini_balanced_allows_plain_text_but_rejects_grammar(
) {
    let plain_text_body = json!({
        "model": "gemini-2.5-flash",
        "tools": [{
            "type": "custom",
            "name": "code_exec",
            "description": "Executes code",
            "format": { "type": "text" }
        }],
        "tool_choice": {
            "type": "custom",
            "name": "code_exec"
        },
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": "run this" }]
        }]
    });
    let plain_text_assessment =
        super::assessment::assess_request_translation_with_compatibility_mode(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Google,
            &plain_text_body,
            crate::config::CompatibilityMode::Balanced,
        );
    assert_eq!(plain_text_assessment.decision(), TranslationDecision::Allow);

    let grammar_body = json!({
        "model": "gemini-2.5-flash",
        "tools": [{
            "type": "custom",
            "name": "apply_patch",
            "description": "Apply a patch",
            "format": {
                "type": "grammar",
                "syntax": "lark",
                "definition": "start: /.+/"
            }
        }],
        "tool_choice": {
            "type": "custom",
            "name": "apply_patch"
        },
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": "create hello.txt" }]
        }]
    });
    let grammar_assessment = super::assessment::assess_request_translation_with_compatibility_mode(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Google,
        &grammar_body,
        crate::config::CompatibilityMode::Balanced,
    );

    let TranslationDecision::Reject(err) = grammar_assessment.decision() else {
        panic!("expected reject policy, got {grammar_assessment:?}");
    };
    assert!(
        err.contains("apply_patch") && err.contains("Gemini"),
        "err = {err}"
    );
}

#[test]
fn assess_request_translation_responses_custom_tool_to_gemini_varies_by_compatibility_mode() {
    let body = json!({
        "model": "gemini-2.5-flash",
        "tools": [{
            "type": "custom",
            "name": "apply_patch",
            "description": "Apply a patch",
            "format": {
                "type": "grammar",
                "syntax": "lark",
                "definition": "start: /.+/"
            }
        }],
        "tool_choice": {
            "type": "custom",
            "name": "apply_patch"
        },
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": "create hello.txt" }]
        }]
    });

    let strict_assessment = super::assessment::assess_request_translation_with_compatibility_mode(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Google,
        &body,
        crate::config::CompatibilityMode::Strict,
    );
    let TranslationDecision::Reject(strict_err) = strict_assessment.decision() else {
        panic!("expected strict rejection, got {strict_assessment:?}");
    };
    assert_eq!(
        strict_err,
        custom_tools_not_portable_message(UpstreamFormat::Google)
    );

    let balanced_assessment = super::assessment::assess_request_translation_with_compatibility_mode(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Google,
        &body,
        crate::config::CompatibilityMode::Balanced,
    );
    let TranslationDecision::Reject(balanced_err) = balanced_assessment.decision() else {
        panic!("expected balanced rejection, got {balanced_assessment:?}");
    };
    assert!(
        balanced_err.contains("apply_patch") && balanced_err.contains("Gemini"),
        "err = {balanced_err}"
    );

    let max_compat_assessment =
        super::assessment::assess_request_translation_with_compatibility_mode(
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Google,
            &body,
            crate::config::CompatibilityMode::MaxCompat,
        );
    let TranslationDecision::AllowWithWarnings(warnings) = max_compat_assessment.decision() else {
        panic!("expected max_compat warning path, got {max_compat_assessment:?}");
    };
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("apply_patch") && warning.contains("Gemini")),
        "warnings = {warnings:?}"
    );
}

#[test]
fn translate_request_chat_to_gemini_rejects_audio_format_without_documented_equivalent() {
    let mut body = json!({
        "model": "gemini-2.5-flash",
        "messages": [{ "role": "user", "content": "Read this aloud" }],
        "modalities": ["text", "audio"],
        "audio": {
            "format": "wav",
            "voice": "alloy"
        }
    });

    let err = translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        "gemini-2.5-flash",
        &mut body,
        false,
    )
    .expect_err("Chat audio.format should fail closed when Gemini has no documented equivalent");

    assert!(err.contains("audio"), "err = {err}");
    assert!(err.contains("format"), "err = {err}");
}

#[test]
fn translate_request_chat_to_non_gemini_rejects_audio_output_intent() {
    for upstream in [UpstreamFormat::OpenAiResponses, UpstreamFormat::Anthropic] {
        let mut body = json!({
            "model": "gpt-4o-audio-preview",
            "messages": [{ "role": "user", "content": "Read this aloud" }],
            "modalities": ["audio"],
            "audio": {
                "format": "wav",
                "voice": "alloy"
            }
        });

        let err = translate_request(
            UpstreamFormat::OpenAiCompletion,
            upstream,
            "gpt-4o-audio-preview",
            &mut body,
            false,
        )
        .expect_err("Chat audio output intent should fail closed for non-Gemini targets");

        assert!(err.contains("audio"), "err = {err}");
    }
}

#[test]
fn translate_request_chat_assistant_audio_history_rejects_on_non_chat_targets() {
    for upstream in [
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        UpstreamFormat::Google,
    ] {
        let mut body = json!({
            "model": "gpt-4o-audio-preview",
            "messages": [
                {
                    "role": "assistant",
                    "content": "Earlier spoken reply",
                    "audio": { "id": "audio_123" }
                },
                {
                    "role": "user",
                    "content": "Continue"
                }
            ]
        });

        let err = translate_request(
            UpstreamFormat::OpenAiCompletion,
            upstream,
            "gpt-4o-audio-preview",
            &mut body,
            false,
        )
        .expect_err("assistant audio history should fail closed for non-Chat targets");

        assert!(err.contains("audio"), "err = {err}");
    }
}

#[test]
fn translate_request_openai_to_responses_maps_max_completion_tokens_to_max_output_tokens() {
    let mut body = json!({
        "model": "gpt-4o",
        "messages": [{ "role": "user", "content": "Hi" }],
        "max_completion_tokens": 222
    });

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        "gpt-4o",
        &mut body,
        false,
    )
    .unwrap();

    assert_eq!(body["max_output_tokens"], 222);
    assert!(body.get("max_completion_tokens").is_none());
}

#[test]
fn translate_request_openai_to_claude_uses_policy_default_max_output_tokens_when_missing() {
    let mut body = json!({
        "model": "gpt-4o",
        "messages": [{ "role": "user", "content": "Hi" }]
    });

    translate_request_with_policy(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        "claude-3-7-sonnet",
        &mut body,
        request_translation_policy(crate::config::CompatibilityMode::Balanced, Some(128_000)),
        false,
    )
    .unwrap();

    assert_eq!(body["max_tokens"], 128_000);
}

#[test]
fn translate_request_openai_to_claude_preserves_explicit_max_completion_tokens_over_policy() {
    let mut body = json!({
        "model": "gpt-4o",
        "messages": [{ "role": "user", "content": "Hi" }],
        "max_completion_tokens": 222
    });

    translate_request_with_policy(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        "claude-3-7-sonnet",
        &mut body,
        request_translation_policy(crate::config::CompatibilityMode::Balanced, Some(128_000)),
        false,
    )
    .unwrap();

    assert_eq!(body["max_tokens"], 222);
}

#[test]
fn translate_request_openai_to_claude_preserves_explicit_max_tokens_over_policy() {
    let mut body = json!({
        "model": "gpt-4o",
        "messages": [{ "role": "user", "content": "Hi" }],
        "max_tokens": 333
    });

    translate_request_with_policy(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        "claude-3-7-sonnet",
        &mut body,
        request_translation_policy(crate::config::CompatibilityMode::Balanced, Some(128_000)),
        false,
    )
    .unwrap();

    assert_eq!(body["max_tokens"], 333);
}

#[test]
fn translate_request_openai_to_responses_uses_policy_default_max_output_tokens_when_missing() {
    let mut body = json!({
        "model": "gpt-4o",
        "messages": [{ "role": "user", "content": "Hi" }]
    });

    translate_request_with_policy(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        "gpt-4o",
        &mut body,
        request_translation_policy(crate::config::CompatibilityMode::Balanced, Some(128_000)),
        false,
    )
    .unwrap();

    assert_eq!(body["max_output_tokens"], 128_000);
}

#[test]
fn translate_request_responses_to_openai_uses_policy_default_max_completion_tokens_when_missing() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": "Hello"
    });

    translate_request_with_policy(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        request_translation_policy(crate::config::CompatibilityMode::Balanced, Some(128_000)),
        false,
    )
    .unwrap();

    assert_eq!(body["max_completion_tokens"], 128_000);
}

#[test]
fn translate_request_google_passthrough_preserves_explicit_snake_case_max_output_tokens_over_policy(
) {
    let mut body = json!({
        "model": "gemini-1.5",
        "contents": [{
            "role": "user",
            "parts": [{ "text": "Hi" }]
        }],
        "generation_config": {
            "max_output_tokens": 222
        }
    });

    translate_request_with_policy(
        UpstreamFormat::Google,
        UpstreamFormat::Google,
        "gemini-1.5",
        &mut body,
        request_translation_policy(crate::config::CompatibilityMode::Balanced, Some(128_000)),
        false,
    )
    .unwrap();

    assert_eq!(body["generation_config"]["max_output_tokens"], 222);
    assert!(
        body.get("generationConfig").is_none(),
        "policy should not add a second max-output field when Google request already carries explicit snake_case output limits: {body:?}"
    );
}

#[test]
fn translate_request_responses_to_chat_maps_max_output_tokens_to_max_completion_tokens() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": "Hello",
        "max_output_tokens": 321
    });

    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        false,
    )
    .unwrap();

    assert_eq!(body["max_completion_tokens"], 321);
    assert!(body.get("max_output_tokens").is_none());
}

#[test]
fn translate_request_gemini_to_openai_maps_snake_case_request_fields_and_allowed_tools() {
    let mut body = json!({
        "model": "gemini-1.5",
        "system_instruction": {
            "parts": [{ "text": "You are helpful." }]
        },
        "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
        "generation_config": {
            "max_output_tokens": 222,
            "temperature": 0.3,
            "top_p": 0.9
        },
        "tools": [{
            "function_declarations": [
                {
                    "name": "lookup_weather",
                    "description": "Weather lookup",
                    "parameters": { "type": "object", "properties": {} }
                },
                {
                    "name": "lookup_time",
                    "description": "Time lookup",
                    "parameters": { "type": "object", "properties": {} }
                },
                {
                    "name": "lookup_news",
                    "description": "News lookup",
                    "parameters": { "type": "object", "properties": {} }
                }
            ]
        }],
        "tool_config": {
            "function_calling_config": {
                "mode": "ANY",
                "allowed_function_names": ["lookup_weather", "lookup_time"]
            }
        }
    });
    translate_request(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiCompletion,
        "gemini-1.5",
        &mut body,
        false,
    )
    .unwrap();

    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][0]["content"], "You are helpful.");
    assert_eq!(body["max_tokens"], 222);
    assert_eq!(body["temperature"], 0.3);
    assert_eq!(body["top_p"], 0.9);
    assert_eq!(body["tools"][0]["function"]["name"], "lookup_weather");
    assert_eq!(body["tools"][1]["function"]["name"], "lookup_time");
    assert_eq!(body["tool_choice"], "required");
    assert_eq!(body["tools"].as_array().unwrap().len(), 2);
    assert!(body.get("allowed_tool_names").is_none(), "body = {body:?}");
}

#[test]
fn translate_request_gemini_to_responses_maps_snake_case_request_fields_and_allowed_tools() {
    let mut body = json!({
        "model": "gemini-1.5",
        "system_instruction": {
            "parts": [{ "text": "You are helpful." }]
        },
        "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
        "generation_config": {
            "max_output_tokens": 222,
            "temperature": 0.3,
            "top_p": 0.9
        },
        "tools": [{
            "function_declarations": [
                {
                    "name": "lookup_weather",
                    "description": "Weather lookup",
                    "parameters": { "type": "object", "properties": {} }
                },
                {
                    "name": "lookup_time",
                    "description": "Time lookup",
                    "parameters": { "type": "object", "properties": {} }
                },
                {
                    "name": "lookup_news",
                    "description": "News lookup",
                    "parameters": { "type": "object", "properties": {} }
                }
            ]
        }],
        "tool_config": {
            "function_calling_config": {
                "mode": "ANY",
                "allowed_function_names": ["lookup_weather", "lookup_time"]
            }
        }
    });
    translate_request(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiResponses,
        "gemini-1.5",
        &mut body,
        false,
    )
    .unwrap();

    assert_eq!(body["input"][0]["role"], "system");
    assert_eq!(body["input"][0]["content"][0]["text"], "You are helpful.");
    assert_eq!(body["max_output_tokens"], 222);
    assert_eq!(body["temperature"], 0.3);
    assert_eq!(body["top_p"], 0.9);
    assert_eq!(body["tools"][0]["name"], "lookup_weather");
    assert_eq!(body["tools"][1]["name"], "lookup_time");
    assert_eq!(body["tool_choice"], "required");
    assert_eq!(body["tools"].as_array().unwrap().len(), 2);
    assert!(body.get("allowed_tool_names").is_none(), "body = {body:?}");
}

#[test]
fn translate_request_gemini_to_openai_maps_single_allowed_function_to_forced_function_choice() {
    let mut body = json!({
        "model": "gemini-1.5",
        "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
        "tools": [{
            "functionDeclarations": [
                {
                    "name": "lookup_weather",
                    "description": "Weather lookup",
                    "parameters": { "type": "object", "properties": {} }
                },
                {
                    "name": "lookup_time",
                    "description": "Time lookup",
                    "parameters": { "type": "object", "properties": {} }
                }
            ]
        }],
        "toolConfig": {
            "functionCallingConfig": {
                "mode": "ANY",
                "allowedFunctionNames": ["lookup_weather"]
            }
        }
    });

    translate_request(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiCompletion,
        "gemini-1.5",
        &mut body,
        false,
    )
    .unwrap();

    assert_eq!(body["tool_choice"]["type"], "function");
    assert_eq!(body["tool_choice"]["function"]["name"], "lookup_weather");
    let tools = body["tools"].as_array().expect("chat tools");
    assert_eq!(tools.len(), 1, "body = {body:?}");
    assert_eq!(tools[0]["function"]["name"], "lookup_weather");
}

#[test]
fn translate_request_gemini_to_responses_maps_single_allowed_function_to_forced_function_choice() {
    let mut body = json!({
        "model": "gemini-1.5",
        "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
        "tools": [{
            "functionDeclarations": [
                {
                    "name": "lookup_weather",
                    "description": "Weather lookup",
                    "parameters": { "type": "object", "properties": {} }
                },
                {
                    "name": "lookup_time",
                    "description": "Time lookup",
                    "parameters": { "type": "object", "properties": {} }
                }
            ]
        }],
        "toolConfig": {
            "functionCallingConfig": {
                "mode": "ANY",
                "allowedFunctionNames": ["lookup_weather"]
            }
        }
    });

    translate_request(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiResponses,
        "gemini-1.5",
        &mut body,
        false,
    )
    .unwrap();

    assert_eq!(body["tool_choice"]["type"], "function");
    assert_eq!(body["tool_choice"]["name"], "lookup_weather");
    let tools = body["tools"].as_array().expect("responses tools");
    assert_eq!(tools.len(), 1, "body = {body:?}");
    assert_eq!(tools[0]["name"], "lookup_weather");
}

#[test]
fn translate_request_gemini_to_openai_maps_json_object_output_shape_control() {
    let mut body = json!({
        "model": "gemini-1.5",
        "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
        "generationConfig": {
            "responseMimeType": "application/json"
        }
    });

    translate_request(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiCompletion,
        "gemini-1.5",
        &mut body,
        false,
    )
    .unwrap();

    assert_eq!(body["response_format"]["type"], "json_object");
}

#[test]
fn translate_request_gemini_to_responses_maps_json_object_output_shape_control() {
    let mut body = json!({
        "model": "gemini-1.5",
        "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
        "generation_config": {
            "response_mime_type": "application/json"
        }
    });

    translate_request(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiResponses,
        "gemini-1.5",
        &mut body,
        false,
    )
    .unwrap();

    assert_eq!(body["text"]["format"]["type"], "json_object");
}

#[test]
fn translate_request_gemini_to_anthropic_maps_snake_case_request_fields_without_losing_allowlist() {
    let mut body = json!({
        "model": "gemini-1.5",
        "system_instruction": {
            "parts": [{ "text": "You are helpful." }]
        },
        "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
        "generation_config": {
            "max_output_tokens": 222,
            "temperature": 0.3,
            "top_p": 0.9
        },
        "tools": [{
            "function_declarations": [
                {
                    "name": "lookup_weather",
                    "description": "Weather lookup",
                    "parameters": { "type": "object", "properties": {} }
                },
                {
                    "name": "lookup_time",
                    "description": "Time lookup",
                    "parameters": { "type": "object", "properties": {} }
                },
                {
                    "name": "lookup_news",
                    "description": "News lookup",
                    "parameters": { "type": "object", "properties": {} }
                }
            ]
        }],
        "tool_config": {
            "function_calling_config": {
                "mode": "ANY",
                "allowed_function_names": ["lookup_weather", "lookup_time"]
            }
        }
    });
    translate_request(
        UpstreamFormat::Google,
        UpstreamFormat::Anthropic,
        "gemini-1.5",
        &mut body,
        false,
    )
    .unwrap();

    assert_eq!(body["system"][0]["text"], "You are helpful.");
    assert_eq!(body["max_tokens"], 222);
    assert_eq!(body["temperature"], 0.3);
    assert_eq!(body["top_p"], 0.9);
    let tools = body["tools"].as_array().expect("anthropic tools");
    assert_eq!(tools.len(), 2, "body = {body:?}");
    assert_eq!(tools[0]["name"], "lookup_weather");
    assert_eq!(tools[1]["name"], "lookup_time");
    assert_eq!(body["tool_choice"]["type"], "any");
}

#[test]
fn translate_request_gemini_to_non_gemini_rejects_built_in_and_server_side_tools() {
    let tool_cases = [
        ("googleSearch", json!({ "googleSearch": {} })),
        ("codeExecution", json!({ "codeExecution": {} })),
        ("computerUse", json!({ "computerUse": {} })),
        (
            "mcpServers",
            json!({ "mcpServers": [{ "server": "https://mcp.example" }] }),
        ),
    ];

    for (label, tool) in tool_cases {
        for upstream_format in [
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Anthropic,
        ] {
            let mut body = json!({
                "model": "gemini-1.5",
                "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
                "tools": [tool.clone()]
            });

            let err = translate_request(
                UpstreamFormat::Google,
                upstream_format,
                "gemini-1.5",
                &mut body,
                false,
            )
            .expect_err("Gemini built-in/server-side tools should fail closed");

            assert!(err.contains(label), "label = {label}, err = {err}");
        }
    }
}

#[test]
fn translate_request_gemini_to_non_gemini_rejects_nonportable_tool_config_controls() {
    let config_cases = [
        (
            "includeServerSideToolInvocations",
            json!({
                "functionCallingConfig": { "mode": "ANY" },
                "includeServerSideToolInvocations": true
            }),
        ),
        (
            "retrievalConfig",
            json!({
                "functionCallingConfig": { "mode": "ANY" },
                "retrievalConfig": { "languageCode": "en" }
            }),
        ),
        (
            "VALIDATED",
            json!({
                "functionCallingConfig": {
                    "mode": "VALIDATED",
                    "allowedFunctionNames": ["lookup_weather"]
                }
            }),
        ),
    ];

    for (label, tool_config) in config_cases {
        for upstream_format in [
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Anthropic,
        ] {
            let mut body = json!({
                "model": "gemini-1.5",
                "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
                "tools": [{
                    "functionDeclarations": [{
                        "name": "lookup_weather",
                        "description": "Weather lookup",
                        "parameters": { "type": "object", "properties": {} }
                    }]
                }],
                "toolConfig": tool_config.clone()
            });

            let err = translate_request(
                UpstreamFormat::Google,
                upstream_format,
                "gemini-1.5",
                &mut body,
                false,
            )
            .expect_err("Gemini non-portable tool config should fail closed");

            assert!(err.contains(label), "label = {label}, err = {err}");
        }
    }
}

#[test]
fn translate_request_gemini_to_non_gemini_allows_pure_function_tools_and_portable_modes() {
    let cases = [
        ("AUTO", json!({ "mode": "AUTO" })),
        ("NONE", json!({ "mode": "NONE" })),
        (
            "ANY",
            json!({
                "mode": "ANY",
                "allowedFunctionNames": ["lookup_weather"]
            }),
        ),
    ];

    for (label, function_calling_config) in cases {
        for upstream_format in [
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Anthropic,
        ] {
            let mut body = json!({
                "model": "gemini-1.5",
                "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
                "tools": [{
                    "functionDeclarations": [{
                        "name": "lookup_weather",
                        "description": "Weather lookup",
                        "parameters": {
                            "type": "object",
                            "properties": {
                                "city": { "type": "string" }
                            }
                        }
                    }]
                }],
                "toolConfig": {
                    "functionCallingConfig": function_calling_config.clone()
                }
            });

            translate_request(
                UpstreamFormat::Google,
                upstream_format,
                "gemini-1.5",
                &mut body,
                false,
            )
            .unwrap_or_else(|err| {
                panic!("label = {label}, upstream = {upstream_format:?}, err = {err}")
            });
        }
    }
}

#[test]
fn translate_request_gemini_to_non_gemini_preserves_parameters_json_schema() {
    for upstream_format in [
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
    ] {
        let mut body = json!({
            "model": "gemini-1.5",
            "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
            "tools": [{
                "functionDeclarations": [{
                    "name": "lookup_weather",
                    "description": "Weather lookup",
                    "parametersJsonSchema": {
                        "type": "object",
                        "properties": {
                            "city": { "type": "string" }
                        },
                        "required": ["city"],
                        "additionalProperties": false
                    }
                }]
            }]
        });

        translate_request(
            UpstreamFormat::Google,
            upstream_format,
            "gemini-1.5",
            &mut body,
            false,
        )
        .unwrap_or_else(|err| {
            panic!("upstream = {upstream_format:?}, err = {err}");
        });

        match upstream_format {
            UpstreamFormat::OpenAiCompletion => {
                let parameters = &body["tools"][0]["function"]["parameters"];
                assert_eq!(parameters["properties"]["city"]["type"], "string");
                assert_eq!(parameters["required"][0], "city");
                assert_eq!(parameters["additionalProperties"], false);
            }
            UpstreamFormat::OpenAiResponses => {
                let parameters = &body["tools"][0]["parameters"];
                assert_eq!(parameters["properties"]["city"]["type"], "string");
                assert_eq!(parameters["required"][0], "city");
                assert_eq!(parameters["additionalProperties"], false);
            }
            UpstreamFormat::Anthropic => {
                let input_schema = &body["tools"][0]["input_schema"];
                assert_eq!(input_schema["properties"]["city"]["type"], "string");
                assert_eq!(input_schema["required"][0], "city");
                assert_eq!(input_schema["additionalProperties"], false);
            }
            UpstreamFormat::Google => unreachable!("non-Gemini loop"),
        }
    }
}

#[test]
fn translate_request_gemini_to_non_gemini_rejects_dual_function_schema_sources() {
    for upstream_format in [
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
    ] {
        let mut body = json!({
            "model": "gemini-1.5",
            "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
            "tools": [{
                "functionDeclarations": [{
                    "name": "lookup_weather",
                    "description": "Weather lookup",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "city": { "type": "string" }
                        }
                    },
                    "parametersJsonSchema": {
                        "type": "object",
                        "properties": {
                            "city": { "type": "string" }
                        },
                        "required": ["city"]
                    }
                }]
            }]
        });

        let err = translate_request(
            UpstreamFormat::Google,
            upstream_format,
            "gemini-1.5",
            &mut body,
            false,
        )
        .expect_err("dual function schema sources should fail closed");

        assert!(err.contains("parameters"), "err = {err}");
        assert!(err.contains("parametersJsonSchema"), "err = {err}");
    }
}

#[test]
fn translate_request_gemini_to_non_gemini_rejects_function_output_schemas() {
    let cases = [
        (
            "response",
            json!({
                "response": {
                    "type": "object",
                    "properties": {
                        "temperature": { "type": "number" }
                    }
                }
            }),
        ),
        (
            "responseJsonSchema",
            json!({
                "responseJsonSchema": {
                    "type": "object",
                    "properties": {
                        "temperature": { "type": "number" }
                    }
                }
            }),
        ),
    ];

    for (label, extra_fields) in cases {
        for upstream_format in [
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Anthropic,
        ] {
            let mut declaration = json!({
                "name": "lookup_weather",
                "description": "Weather lookup",
                "parameters": { "type": "object", "properties": {} }
            });
            declaration
                .as_object_mut()
                .expect("declaration object")
                .extend(
                    extra_fields
                        .as_object()
                        .expect("extra fields object")
                        .clone(),
                );

            let mut body = json!({
                "model": "gemini-1.5",
                "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
                "tools": [{
                    "functionDeclarations": [declaration]
                }]
            });

            let err = translate_request(
                UpstreamFormat::Google,
                upstream_format,
                "gemini-1.5",
                &mut body,
                false,
            )
            .expect_err("Gemini function output schemas should fail closed");

            assert!(err.contains(label), "label = {label}, err = {err}");
        }
    }
}

#[test]
fn translate_request_gemini_to_non_gemini_rejects_invalid_allowed_function_names() {
    let cases = [
        ("non-string", json!([123])),
        ("empty array", json!([])),
        ("unknown tool", json!(["lookup_unknown"])),
        (
            "mixed valid and unknown",
            json!(["lookup_weather", "lookup_unknown"]),
        ),
        ("mixed valid and empty", json!(["lookup_weather", ""])),
    ];

    for (label, allowed_function_names) in cases {
        for upstream_format in [
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Anthropic,
        ] {
            let mut body = json!({
                "model": "gemini-1.5",
                "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
                "tools": [{
                    "function_declarations": [
                        {
                            "name": "lookup_weather",
                            "description": "Weather lookup",
                            "parameters": { "type": "object", "properties": {} }
                        },
                        {
                            "name": "lookup_time",
                            "description": "Time lookup",
                            "parameters": { "type": "object", "properties": {} }
                        }
                    ]
                }],
                "tool_config": {
                    "function_calling_config": {
                        "mode": "ANY",
                        "allowed_function_names": allowed_function_names.clone()
                    }
                }
            });

            let err = translate_request(
                UpstreamFormat::Google,
                upstream_format,
                "gemini-1.5",
                &mut body,
                false,
            )
            .expect_err("invalid allowedFunctionNames should fail closed");

            assert!(
                err.contains("allowedFunctionNames") || err.contains("allowed_function_names"),
                "label = {label}, err = {err}"
            );
        }
    }
}

#[test]
fn translate_request_gemini_to_openai_maps_json_output_shape_controls() {
    let mut body = json!({
        "model": "gemini-1.5",
        "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
        "generationConfig": {
            "responseMimeType": "application/json",
            "responseJsonSchema": {
                "type": "object",
                "properties": {
                    "city": { "type": "string" }
                },
                "required": ["city"]
            }
        }
    });

    translate_request(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiCompletion,
        "gemini-1.5",
        &mut body,
        false,
    )
    .unwrap();

    assert_eq!(body["response_format"]["type"], "json_schema");
    assert_eq!(
        body["response_format"]["json_schema"]["schema"]["properties"]["city"]["type"],
        "string"
    );
    assert_eq!(
        body["response_format"]["json_schema"]["schema"]["required"][0],
        "city"
    );
}

#[test]
fn translate_request_gemini_to_responses_maps_json_output_shape_controls() {
    let mut body = json!({
        "model": "gemini-1.5",
        "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
        "generation_config": {
            "response_mime_type": "application/json",
            "response_json_schema": {
                "type": "object",
                "properties": {
                    "city": { "type": "string" }
                },
                "required": ["city"]
            }
        }
    });

    translate_request(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiResponses,
        "gemini-1.5",
        &mut body,
        false,
    )
    .unwrap();

    assert_eq!(body["text"]["format"]["type"], "json_schema");
    assert_eq!(
        body["text"]["format"]["schema"]["properties"]["city"]["type"],
        "string"
    );
    assert_eq!(body["text"]["format"]["schema"]["required"][0], "city");
}

#[test]
fn translate_request_gemini_to_non_gemini_rejects_nonportable_output_shape_controls() {
    let cases = [
        (
            "responseSchema",
            json!({
                "responseMimeType": "application/json",
                "responseSchema": {
                    "type": "object",
                    "properties": {
                        "city": { "type": "string" }
                    }
                }
            }),
        ),
        (
            "text/x.enum",
            json!({
                "responseMimeType": "text/x.enum"
            }),
        ),
    ];

    for (label, generation_config) in cases {
        for upstream_format in [
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
        ] {
            let mut body = json!({
                "model": "gemini-1.5",
                "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
                "generationConfig": generation_config.clone()
            });

            let err = translate_request(
                UpstreamFormat::Google,
                upstream_format,
                "gemini-1.5",
                &mut body,
                false,
            )
            .expect_err("nonportable Gemini output-shape controls should fail closed");

            assert!(err.contains(label), "label = {label}, err = {err}");
        }
    }
}

#[test]
fn translate_request_openai_to_gemini_maps_response_format_json_schema() {
    let mut body = json!({
        "model": "gemini-2.5-flash",
        "messages": [{ "role": "user", "content": "Hi" }],
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "weather_response",
                "schema": {
                    "type": "object",
                    "properties": {
                        "city": { "type": "string" }
                    }
                }
            }
        }
    });

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        "gemini-2.5-flash",
        &mut body,
        false,
    )
    .unwrap();

    assert_eq!(
        body["generationConfig"]["responseMimeType"],
        "application/json"
    );
    assert_eq!(
        body["generationConfig"]["responseJsonSchema"]["properties"]["city"]["type"],
        "string"
    );
}

#[test]
fn translate_request_openai_to_responses_maps_response_format_json_schema() {
    let mut body = json!({
        "model": "gpt-4o",
        "messages": [{ "role": "user", "content": "Hi" }],
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "weather_response",
                "schema": {
                    "type": "object",
                    "properties": {
                        "city": { "type": "string" }
                    }
                },
                "strict": true
            }
        }
    });

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        "gpt-4o",
        &mut body,
        false,
    )
    .unwrap();

    assert_eq!(body["text"]["format"]["type"], "json_schema");
    assert_eq!(body["text"]["format"]["name"], "weather_response");
    assert_eq!(
        body["text"]["format"]["schema"]["properties"]["city"]["type"],
        "string"
    );
    assert_eq!(body["text"]["format"]["strict"], true);
    assert!(body.get("response_format").is_none(), "body = {body:?}");
}

#[test]
fn translate_request_openai_to_claude_maps_json_schema_output_shape() {
    let mut body = json!({
        "model": "claude-3",
        "messages": [{ "role": "user", "content": "Hi" }],
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "weather_response",
                "schema": {
                    "type": "object",
                    "properties": {
                        "city": { "type": "string" }
                    },
                    "required": ["city"]
                }
            }
        }
    });

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        "claude-3",
        &mut body,
        false,
    )
    .unwrap();

    assert_eq!(body["output_config"]["format"]["type"], "json_schema");
    assert_eq!(
        body["output_config"]["format"]["schema"]["properties"]["city"]["type"],
        "string"
    );
    assert_eq!(
        body["output_config"]["format"]["schema"]["required"][0],
        "city"
    );
}

#[test]
fn translate_request_responses_to_claude_maps_json_schema_output_shape() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": "Hi",
        "text": {
            "format": {
                "type": "json_schema",
                "name": "weather_response",
                "schema": {
                    "type": "object",
                    "properties": {
                        "city": { "type": "string" }
                    },
                    "required": ["city"]
                }
            }
        }
    });

    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        "claude-3",
        &mut body,
        false,
    )
    .unwrap();

    assert_eq!(body["output_config"]["format"]["type"], "json_schema");
    assert_eq!(
        body["output_config"]["format"]["schema"]["properties"]["city"]["type"],
        "string"
    );
    assert_eq!(
        body["output_config"]["format"]["schema"]["required"][0],
        "city"
    );
}

#[test]
fn translate_request_openai_json_object_output_shape_to_claude_rejects() {
    for (client_format, mut body) in [
        (
            UpstreamFormat::OpenAiCompletion,
            json!({
                "model": "claude-3",
                "messages": [{ "role": "user", "content": "Hi" }],
                "response_format": { "type": "json_object" }
            }),
        ),
        (
            UpstreamFormat::OpenAiResponses,
            json!({
                "model": "gpt-4o",
                "input": "Hi",
                "text": { "format": { "type": "json_object" } }
            }),
        ),
    ] {
        let err = translate_request(
            client_format,
            UpstreamFormat::Anthropic,
            "claude-3",
            &mut body,
            false,
        )
        .expect_err("json_object output shape should fail closed for Anthropic");

        assert!(err.contains("json_object"), "err = {err}");
    }
}

#[test]
fn translate_request_responses_to_gemini_maps_text_json_schema_format() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": "Hi",
        "text": {
            "format": {
                "type": "json_schema",
                "name": "weather_response",
                "schema": {
                    "type": "object",
                    "properties": {
                        "city": { "type": "string" }
                    }
                }
            }
        }
    });

    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Google,
        "gemini-2.5-flash",
        &mut body,
        false,
    )
    .unwrap();

    assert_eq!(
        body["generationConfig"]["responseMimeType"],
        "application/json"
    );
    assert_eq!(
        body["generationConfig"]["responseJsonSchema"]["properties"]["city"]["type"],
        "string"
    );
}

#[test]
fn translate_request_gemini_to_openai_maps_stop_seed_and_penalties() {
    let mut body = json!({
        "model": "gemini-1.5",
        "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
        "generationConfig": {
            "stopSequences": ["END"],
            "seed": 42,
            "presencePenalty": 0.7,
            "frequencyPenalty": 0.3
        }
    });

    translate_request(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiCompletion,
        "gemini-1.5",
        &mut body,
        false,
    )
    .unwrap();

    assert_eq!(body["stop"][0], "END");
    assert_eq!(body["seed"], 42);
    assert_eq!(body["presence_penalty"], 0.7);
    assert_eq!(body["frequency_penalty"], 0.3);
}

#[test]
fn translate_request_gemini_to_openai_maps_logprobs_controls() {
    let mut body = json!({
        "model": "gemini-1.5",
        "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
        "generationConfig": {
            "responseLogprobs": true,
            "logprobs": 5
        }
    });

    translate_request(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiCompletion,
        "gemini-1.5",
        &mut body,
        false,
    )
    .unwrap();

    assert_eq!(body["logprobs"], true);
    assert_eq!(body["top_logprobs"], 5);
}

#[test]
fn translate_request_gemini_to_responses_maps_logprobs_controls() {
    let mut body = json!({
        "model": "gemini-1.5",
        "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
        "generation_config": {
            "response_logprobs": true,
            "logprobs": 5
        }
    });

    translate_request(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiResponses,
        "gemini-1.5",
        &mut body,
        false,
    )
    .unwrap();

    let include = body["include"].as_array().expect("responses include");
    assert!(
        include
            .iter()
            .any(|item| item == "message.output_text.logprobs"),
        "body = {body:?}"
    );
    assert_eq!(body["top_logprobs"], 5);
}

#[test]
fn translate_request_openai_to_gemini_maps_stop_seed_and_penalties() {
    let mut body = json!({
        "model": "gemini-2.5-flash",
        "messages": [{ "role": "user", "content": "Hi" }],
        "stop": ["END"],
        "seed": 42,
        "presence_penalty": 0.7,
        "frequency_penalty": 0.3
    });

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        "gemini-2.5-flash",
        &mut body,
        false,
    )
    .unwrap();

    assert_eq!(body["generationConfig"]["stopSequences"][0], "END");
    assert_eq!(body["generationConfig"]["seed"], 42);
    assert_eq!(body["generationConfig"]["presencePenalty"], 0.7);
    assert_eq!(body["generationConfig"]["frequencyPenalty"], 0.3);
}

#[test]
fn assess_request_translation_gemini_top_k_warns_and_translation_omits_it() {
    let mut body = json!({
        "model": "gemini-1.5",
        "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
        "generationConfig": {
            "topK": 40
        }
    });

    let assessment = assess_request_translation(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiCompletion,
        &body,
    );
    let TranslationDecision::AllowWithWarnings(warnings) = assessment.decision() else {
        panic!("expected warning policy, got {assessment:?}");
    };
    assert!(warnings.iter().any(|warning| warning.contains("topK")));

    translate_request(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiCompletion,
        "gemini-1.5",
        &mut body,
        false,
    )
    .unwrap();

    assert!(body.get("top_k").is_none(), "body = {body:?}");
    assert!(body.get("topK").is_none(), "body = {body:?}");
}

#[test]
fn assess_request_translation_gemini_to_anthropic_warns_on_dropped_logprobs_controls() {
    let body = json!({
        "model": "gemini-1.5",
        "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
        "generationConfig": {
            "responseLogprobs": true,
            "logprobs": 5
        }
    });

    let assessment =
        assess_request_translation(UpstreamFormat::Google, UpstreamFormat::Anthropic, &body);
    let TranslationDecision::AllowWithWarnings(warnings) = assessment.decision() else {
        panic!("expected warning policy, got {assessment:?}");
    };
    let joined = warnings.join("\n");
    assert!(
        joined.contains("responseLogprobs"),
        "warnings = {warnings:?}"
    );
    assert!(joined.contains("logprobs"), "warnings = {warnings:?}");
}

#[test]
fn assess_request_translation_openai_to_anthropic_reasoning_without_replay_provenance_is_not_rejected(
) {
    let body = json!({
        "model": "claude-3",
        "messages": [{
            "role": "assistant",
            "reasoning_content": "internal reasoning",
            "content": "Visible answer"
        }]
    });

    let assessment = assess_request_translation(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        &body,
    );
    assert!(
        !matches!(assessment.decision(), TranslationDecision::Reject(_)),
        "assessment = {assessment:?}"
    );
}

#[test]
fn assess_request_translation_responses_to_openai_warns_when_reasoning_encrypted_content_is_dropped(
) {
    let body = json!({
        "model": "gpt-4o",
        "input": [{
            "type": "reasoning",
            "summary": [{ "type": "summary_text", "text": "thinking" }],
            "encrypted_content": "opaque_state"
        }]
    });

    let assessment = assess_request_translation(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        &body,
    );
    let TranslationDecision::AllowWithWarnings(warnings) = assessment.decision() else {
        panic!("expected warning policy, got {assessment:?}");
    };
    let joined = warnings.join("\n");
    assert!(
        joined.contains("encrypted_content"),
        "warnings = {warnings:?}"
    );
    assert!(joined.contains("dropped"), "warnings = {warnings:?}");
    assert!(joined.contains("summary"), "warnings = {warnings:?}");
}

#[test]
fn assess_request_translation_store_warns_and_cross_provider_translation_drops_it() {
    let gemini_body = json!({
        "model": "gemini-1.5",
        "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }],
        "store": true
    });
    let gemini_assessment = assess_request_translation(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiCompletion,
        &gemini_body,
    );
    let TranslationDecision::AllowWithWarnings(gemini_warnings) = gemini_assessment.decision()
    else {
        panic!("expected Gemini store warning, got {gemini_assessment:?}");
    };
    assert!(gemini_warnings
        .iter()
        .any(|warning| warning.contains("store")));

    let mut openai_body = json!({
        "model": "gemini-2.5-flash",
        "messages": [{ "role": "user", "content": "Hi" }],
        "store": true
    });
    let openai_assessment = assess_request_translation(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        &openai_body,
    );
    let TranslationDecision::AllowWithWarnings(openai_warnings) = openai_assessment.decision()
    else {
        panic!("expected OpenAI store warning, got {openai_assessment:?}");
    };
    assert!(openai_warnings
        .iter()
        .any(|warning| warning.contains("store")));

    let mut responses_body = json!({
        "model": "gpt-4o",
        "input": "Hi",
        "store": true
    });
    let responses_assessment = assess_request_translation(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        &responses_body,
    );
    let TranslationDecision::AllowWithWarnings(responses_warnings) =
        responses_assessment.decision()
    else {
        panic!("expected Responses store warning, got {responses_assessment:?}");
    };
    assert!(responses_warnings
        .iter()
        .any(|warning| warning.contains("store")));

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        "gemini-2.5-flash",
        &mut openai_body,
        false,
    )
    .unwrap();

    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut responses_body,
        false,
    )
    .unwrap();

    assert!(openai_body.get("store").is_none(), "body = {openai_body:?}");
    assert!(
        responses_body.get("store").is_none(),
        "body = {responses_body:?}"
    );
}

#[test]
fn assess_request_translation_openai_to_responses_warns_on_dropped_sampling_controls() {
    let body = json!({
        "model": "gpt-4o",
        "messages": [{ "role": "user", "content": "Hi" }],
        "stop": ["END"],
        "seed": 42,
        "presence_penalty": 0.7,
        "frequency_penalty": 0.3
    });

    let assessment = assess_request_translation(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        &body,
    );
    let TranslationDecision::AllowWithWarnings(warnings) = assessment.decision() else {
        panic!("expected warning policy, got {assessment:?}");
    };
    let joined = warnings.join("\n");
    assert!(joined.contains("stop"), "warnings = {warnings:?}");
    assert!(joined.contains("seed"), "warnings = {warnings:?}");
    assert!(
        joined.contains("presence_penalty"),
        "warnings = {warnings:?}"
    );
    assert!(
        joined.contains("frequency_penalty"),
        "warnings = {warnings:?}"
    );
}

#[test]
fn assess_request_translation_openai_to_non_chat_warns_on_logit_bias_drop() {
    for upstream_format in [
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        UpstreamFormat::Google,
    ] {
        let body = json!({
            "model": "target-model",
            "messages": [{ "role": "user", "content": "Hi" }],
            "logit_bias": { "42": 3 }
        });

        let assessment =
            assess_request_translation(UpstreamFormat::OpenAiCompletion, upstream_format, &body);
        let TranslationDecision::AllowWithWarnings(warnings) = assessment.decision() else {
            panic!("expected warning policy, got {assessment:?}");
        };
        let joined = warnings.join("\n");
        assert!(joined.contains("logit_bias"), "warnings = {warnings:?}");
    }
}

#[test]
fn translate_request_openai_to_non_chat_drops_logit_bias() {
    for (upstream_format, model) in [
        (UpstreamFormat::OpenAiResponses, "gpt-4o"),
        (UpstreamFormat::Anthropic, "claude-3"),
        (UpstreamFormat::Google, "gemini-2.5-flash"),
    ] {
        let mut body = json!({
            "model": model,
            "messages": [{ "role": "user", "content": "Hi" }],
            "logit_bias": { "42": 3 }
        });

        translate_request(
            UpstreamFormat::OpenAiCompletion,
            upstream_format,
            model,
            &mut body,
            false,
        )
        .unwrap();

        assert!(body.get("logit_bias").is_none(), "body = {body:?}");
    }
}

#[test]
fn assess_request_translation_openai_to_claude_warns_on_dropped_sampling_and_shared_controls_only()
{
    let body = json!({
        "model": "claude-3",
        "messages": [{ "role": "user", "content": "Hi" }],
        "stop": ["END"],
        "temperature": 0.3,
        "top_p": 0.9,
        "service_tier": "priority",
        "verbosity": "low",
        "reasoning_effort": "medium",
        "seed": 42,
        "presence_penalty": 0.7,
        "frequency_penalty": 0.3
    });

    let assessment = assess_request_translation(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        &body,
    );
    let TranslationDecision::AllowWithWarnings(warnings) = assessment.decision() else {
        panic!("expected warning policy, got {assessment:?}");
    };
    let joined = warnings.join("\n");
    assert!(joined.contains("seed"), "warnings = {warnings:?}");
    assert!(
        joined.contains("presence_penalty"),
        "warnings = {warnings:?}"
    );
    assert!(
        joined.contains("frequency_penalty"),
        "warnings = {warnings:?}"
    );
    assert!(joined.contains("service_tier"), "warnings = {warnings:?}");
    assert!(joined.contains("verbosity"), "warnings = {warnings:?}");
    assert!(
        joined.contains("reasoning_effort"),
        "warnings = {warnings:?}"
    );
    assert!(!joined.contains("stop"), "warnings = {warnings:?}");
    assert!(!joined.contains("temperature"), "warnings = {warnings:?}");
    assert!(!joined.contains("top_p"), "warnings = {warnings:?}");
}

#[test]
fn assess_request_translation_openai_to_gemini_warns_on_parallel_tool_calls_drop() {
    let body = json!({
        "model": "gemini-2.5-flash",
        "messages": [{ "role": "user", "content": "Hi" }],
        "tools": [{
            "type": "function",
            "function": {
                "name": "lookup_weather",
                "parameters": { "type": "object", "properties": {} }
            }
        }],
        "parallel_tool_calls": false
    });

    let assessment = assess_request_translation(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        &body,
    );
    let TranslationDecision::AllowWithWarnings(warnings) = assessment.decision() else {
        panic!("expected warning policy, got {assessment:?}");
    };
    let joined = warnings.join("\n");
    assert!(
        joined.contains("parallel_tool_calls"),
        "warnings = {warnings:?}"
    );
}

#[test]
fn assess_request_translation_openai_to_gemini_warns_on_shared_control_drops() {
    let body = json!({
        "model": "gemini-2.5-flash",
        "messages": [{ "role": "user", "content": "Hi" }],
        "stop": ["END"],
        "temperature": 0.3,
        "top_p": 0.9,
        "service_tier": "priority",
        "verbosity": "low",
        "reasoning_effort": "medium"
    });

    let assessment = assess_request_translation(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        &body,
    );
    let TranslationDecision::AllowWithWarnings(warnings) = assessment.decision() else {
        panic!("expected warning policy, got {assessment:?}");
    };
    let joined = warnings.join("\n");
    assert!(joined.contains("service_tier"), "warnings = {warnings:?}");
    assert!(joined.contains("verbosity"), "warnings = {warnings:?}");
    assert!(
        joined.contains("reasoning_effort"),
        "warnings = {warnings:?}"
    );
    assert!(!joined.contains("stop"), "warnings = {warnings:?}");
    assert!(!joined.contains("temperature"), "warnings = {warnings:?}");
    assert!(!joined.contains("top_p"), "warnings = {warnings:?}");
}

#[test]
fn assess_request_translation_openai_to_gemini_warns_on_prediction_and_web_search_drop() {
    let body = json!({
        "model": "gemini-2.5-flash",
        "messages": [{ "role": "user", "content": "Hi" }],
        "prediction": {
            "type": "content",
            "content": "Expected completion"
        },
        "web_search_options": {
            "search_context_size": "medium"
        }
    });

    let assessment = assess_request_translation(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        &body,
    );
    let TranslationDecision::AllowWithWarnings(warnings) = assessment.decision() else {
        panic!("expected warning policy, got {assessment:?}");
    };
    let joined = warnings.join("\n");
    assert!(joined.contains("prediction"), "warnings = {warnings:?}");
    assert!(
        joined.contains("web_search_options"),
        "warnings = {warnings:?}"
    );
}

#[test]
fn assess_request_translation_openai_to_anthropic_warns_on_prediction_and_web_search_drop() {
    let body = json!({
        "model": "claude-3",
        "messages": [{ "role": "user", "content": "Hi" }],
        "prediction": {
            "type": "content",
            "content": "Expected completion"
        },
        "web_search_options": {
            "search_context_size": "medium"
        }
    });

    let assessment = assess_request_translation(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        &body,
    );
    let TranslationDecision::AllowWithWarnings(warnings) = assessment.decision() else {
        panic!("expected warning policy, got {assessment:?}");
    };
    let joined = warnings.join("\n");
    assert!(joined.contains("prediction"), "warnings = {warnings:?}");
    assert!(
        joined.contains("web_search_options"),
        "warnings = {warnings:?}"
    );
}

#[test]
fn assess_request_translation_responses_to_non_responses_warns_on_context_management_drop() {
    let body = json!({
        "model": "gpt-4o",
        "input": "Hi",
        "context_management": {
            "type": "auto"
        }
    });

    for upstream_format in [
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        UpstreamFormat::Anthropic,
    ] {
        let assessment =
            assess_request_translation(UpstreamFormat::OpenAiResponses, upstream_format, &body);
        let TranslationDecision::AllowWithWarnings(warnings) = assessment.decision() else {
            panic!("expected warning policy, got {assessment:?}");
        };
        let joined = warnings.join("\n");
        assert!(
            joined.contains("context_management"),
            "upstream = {upstream_format:?}, warnings = {warnings:?}"
        );
    }
}

#[test]
fn assess_request_translation_claude_to_openai_warns_on_dropped_thinking_context_management_and_cache_controls(
) {
    let body = json!({
        "model": "claude-3",
        "thinking": {
            "type": "enabled",
            "budget_tokens": 2048
        },
        "context_management": {
            "type": "auto"
        },
        "cache_control": {
            "type": "ephemeral"
        },
        "messages": [{
            "role": "user",
            "content": [{
                "type": "text",
                "text": "Hi",
                "cache_control": { "type": "ephemeral" }
            }]
        }]
    });

    let assessment = assess_request_translation(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        &body,
    );
    let TranslationDecision::AllowWithWarnings(warnings) = assessment.decision() else {
        panic!("expected warning policy, got {assessment:?}");
    };
    let joined = warnings.join("\n");
    assert!(joined.contains("thinking"), "warnings = {warnings:?}");
    assert!(
        joined.contains("context_management"),
        "warnings = {warnings:?}"
    );
    assert!(joined.contains("cache_control"), "warnings = {warnings:?}");
}

#[test]
fn assess_request_translation_claude_to_openai_warns_on_dropped_thinking_provenance() {
    let body = json!({
        "model": "claude-3",
        "messages": [{
            "role": "assistant",
            "content": [{
                "type": "thinking",
                "thinking": "internal reasoning",
                "signature": "sig_123"
            }]
        }]
    });

    let assessment = assess_request_translation(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        &body,
    );
    let TranslationDecision::AllowWithWarnings(warnings) = assessment.decision() else {
        panic!("expected warning policy, got {assessment:?}");
    };
    let joined = warnings.join("\n");
    assert!(joined.contains("signature"), "warnings = {warnings:?}");
    assert!(joined.contains("dropped"), "warnings = {warnings:?}");
}

#[test]
fn assess_request_translation_claude_to_responses_allows_thinking_provenance_with_carrier() {
    let body = json!({
        "model": "claude-3",
        "messages": [{
            "role": "assistant",
            "content": [{
                "type": "thinking",
                "thinking": "internal reasoning",
                "signature": "sig_123"
            }]
        }]
    });

    let assessment = assess_request_translation(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &body,
    );
    assert_eq!(assessment.decision(), TranslationDecision::Allow);
}

#[test]
fn assess_request_translation_claude_to_openai_still_rejects_container_state_surface() {
    let body = json!({
        "model": "claude-3",
        "container": {
            "id": "container_123"
        },
        "messages": [{ "role": "user", "content": "Hi" }]
    });

    let assessment = assess_request_translation(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        &body,
    );
    let TranslationDecision::Reject(message) = assessment.decision() else {
        panic!("expected reject policy, got {assessment:?}");
    };
    assert!(message.contains("container"), "message = {message}");
}

#[test]
fn assess_request_translation_claude_to_openai_allows_mappable_tool_choice() {
    let body = json!({
        "model": "claude-3",
        "tool_choice": {
            "type": "tool",
            "name": "lookup",
            "disable_parallel_tool_use": true
        },
        "tools": [{
            "name": "lookup",
            "input_schema": { "type": "object", "properties": {} }
        }],
        "messages": [{ "role": "user", "content": "Hi" }]
    });

    let assessment = assess_request_translation(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        &body,
    );
    assert_eq!(assessment.decision(), TranslationDecision::Allow);
}

#[test]
fn assess_request_translation_responses_to_openai_warns_on_truly_dropped_controls() {
    let body = json!({
        "model": "gpt-4o",
        "input": "Hi",
        "metadata": { "trace_id": "abc" },
        "user": "user-123",
        "service_tier": "priority",
        "stop": ["END"],
        "seed": 42,
        "presence_penalty": 0.7,
        "frequency_penalty": 0.3,
        "top_logprobs": 5,
        "stream_options": { "include_obfuscation": true },
        "include": ["reasoning.encrypted_content"],
        "reasoning": { "effort": "medium" },
        "text": {
            "format": { "type": "text" },
            "verbosity": "low"
        },
        "max_tool_calls": 2,
        "truncation": "auto",
        "prompt_cache_key": "cache-key",
        "prompt_cache_retention": "24h",
        "safety_identifier": "safe-user"
    });

    let assessment = assess_request_translation(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        &body,
    );
    let TranslationDecision::AllowWithWarnings(warnings) = assessment.decision() else {
        panic!("expected warning policy, got {assessment:?}");
    };
    let joined = warnings.join("\n");
    for field in [
        "stop",
        "seed",
        "presence_penalty",
        "frequency_penalty",
        "include",
        "max_tool_calls",
        "truncation",
    ] {
        assert!(
            joined.contains(field),
            "field = {field}, warnings = {warnings:?}"
        );
    }
    for field in [
        "metadata",
        "user",
        "service_tier",
        "include_obfuscation",
        "reasoning",
        "verbosity",
        "top_logprobs",
        "prompt_cache_key",
        "prompt_cache_retention",
        "safety_identifier",
    ] {
        assert!(
            !joined.contains(field),
            "field = {field}, warnings = {warnings:?}"
        );
    }
}

#[test]
fn translate_request_responses_to_claude_keeps_tool_use_and_result_adjacent() {
    let mut body = json!({
        "model": "codex-anthropic",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Run pwd" }]
            },
            {
                "type": "function_call",
                "call_id": "call_1",
                "name": "exec_command",
                "arguments": "{\"cmd\":\"pwd\"}"
            },
            {
                "type": "function_call_output",
                "call_id": "call_1",
                "output": "/home/percy/temp"
            }
        ],
        "stream": true
    });
    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        "codex-anthropic",
        &mut body,
        true,
    )
    .unwrap();

    let messages = body["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[1]["role"], "assistant");
    let assistant_blocks = messages[1]["content"]
        .as_array()
        .expect("assistant content");
    assert_eq!(assistant_blocks.len(), 1);
    assert_eq!(assistant_blocks[0]["type"], "tool_use");
    assert_eq!(assistant_blocks[0]["id"], "call_1");
    assert_eq!(assistant_blocks[0]["input"]["cmd"], "pwd");

    assert_eq!(messages[2]["role"], "user");
    let user_blocks = messages[2]["content"].as_array().expect("user content");
    assert_eq!(user_blocks.len(), 1);
    assert_eq!(user_blocks[0]["type"], "tool_result");
    assert_eq!(user_blocks[0]["tool_use_id"], "call_1");
    assert_eq!(user_blocks[0]["content"], "/home/percy/temp");
}

#[test]
fn translate_request_responses_to_claude_merges_assistant_text_with_tool_use() {
    let mut body = json!({
        "model": "codex-anthropic",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Run pwd" }]
            },
            {
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": "Let me check." }]
            },
            {
                "type": "function_call",
                "call_id": "call_1",
                "name": "exec_command",
                "arguments": "{\"cmd\":\"pwd\"}"
            },
            {
                "type": "function_call_output",
                "call_id": "call_1",
                "output": "/home/percy/temp"
            }
        ],
        "stream": true
    });
    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        "codex-anthropic",
        &mut body,
        true,
    )
    .unwrap();

    let messages = body["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 3);
    let assistant_blocks = messages[1]["content"]
        .as_array()
        .expect("assistant content");
    assert_eq!(assistant_blocks[0]["type"], "text");
    assert_eq!(assistant_blocks[0]["text"], "Let me check.");
    assert_eq!(assistant_blocks[1]["type"], "tool_use");
    assert_eq!(assistant_blocks[1]["id"], "call_1");
    let user_blocks = messages[2]["content"].as_array().expect("user content");
    assert_eq!(user_blocks[0]["type"], "tool_result");
    assert_eq!(user_blocks[0]["tool_use_id"], "call_1");
}

#[test]
fn translate_request_responses_to_claude_moves_user_warning_after_tool_result() {
    let mut body = json!({
        "model": "codex-anthropic",
        "input": [
            {
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": "Running test..." }]
            },
            {
                "type": "function_call",
                "call_id": "call_1",
                "name": "exec_command",
                "arguments": "{\"cmd\":\"python test.py\"}"
            },
            {
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Warning: process limit reached" }]
            },
            {
                "type": "function_call_output",
                "call_id": "call_1",
                "output": "Done"
            }
        ],
        "stream": true
    });
    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        "codex-anthropic",
        &mut body,
        true,
    )
    .unwrap();

    let messages = body["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1]["role"], "user");
    let user_blocks = messages[1]["content"].as_array().expect("user content");
    assert_eq!(user_blocks[0]["type"], "tool_result");
    assert_eq!(user_blocks[0]["tool_use_id"], "call_1");
    assert_eq!(user_blocks[1]["type"], "text");
    assert_eq!(user_blocks[1]["text"], "Warning: process limit reached");
}

#[test]
fn translate_request_responses_to_claude_preserves_multiple_assistant_and_deferred_user_fragments()
{
    let mut body = json!({
        "model": "codex-anthropic",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Run pwd" }]
            },
            {
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": "Let me check." }]
            },
            {
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": "Still checking." }]
            },
            {
                "type": "function_call",
                "call_id": "call_1",
                "name": "exec_command",
                "arguments": "{\"cmd\":\"pwd\"}"
            },
            {
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Warning A" }]
            },
            {
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Warning B" }]
            },
            {
                "type": "function_call_output",
                "call_id": "call_1",
                "output": "Done"
            },
            {
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Final note" }]
            }
        ],
        "stream": true
    });
    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        "codex-anthropic",
        &mut body,
        true,
    )
    .unwrap();

    let messages = body["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 4, "body = {body:?}");
    let assistant_blocks = messages[1]["content"]
        .as_array()
        .expect("assistant content");
    assert_eq!(assistant_blocks[0]["type"], "text");
    assert_eq!(assistant_blocks[0]["text"], "Let me check.");
    assert_eq!(assistant_blocks[1]["type"], "text");
    assert_eq!(assistant_blocks[1]["text"], "Still checking.");
    assert_eq!(assistant_blocks[2]["type"], "tool_use");
    assert_eq!(assistant_blocks[2]["id"], "call_1");

    assert_eq!(messages[2]["role"], "user");
    let user_blocks = messages[2]["content"].as_array().expect("user content");
    assert_eq!(user_blocks[0]["type"], "tool_result");
    assert_eq!(user_blocks[0]["tool_use_id"], "call_1");
    assert_eq!(user_blocks[0]["content"], "Done");
    assert_eq!(user_blocks[1]["type"], "text");
    assert_eq!(user_blocks[1]["text"], "Warning A");
    assert_eq!(user_blocks[2]["type"], "text");
    assert_eq!(user_blocks[2]["text"], "Warning B");

    assert_eq!(messages[3]["role"], "user");
    assert_eq!(messages[3]["content"][0]["type"], "text");
    assert_eq!(messages[3]["content"][0]["text"], "Final note");
}

#[test]
fn translate_request_responses_custom_tool_to_claude_bridges_definition_choice_and_history() {
    let mut body = json!({
        "model": "claude-3-7-sonnet",
        "tools": [{
            "type": "custom",
            "name": "code_exec",
            "description": "Executes code",
            "format": {
                "type": "text"
            }
        }],
        "tool_choice": {
            "type": "custom",
            "name": "code_exec"
        },
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Run this script" }]
            },
            {
                "type": "custom_tool_call",
                "call_id": "call_custom",
                "name": "code_exec",
                "input": "print('hi')"
            },
            {
                "type": "custom_tool_call_output",
                "call_id": "call_custom",
                "output": "exit 0"
            }
        ]
    });

    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        "claude-3-7-sonnet",
        &mut body,
        false,
    )
    .expect("Responses custom tools should bridge to Anthropic");

    assert_eq!(
        body["_llmup_tool_bridge_context"],
        json!({
            "compatibility_mode": "max_compat",
            "entries": {
                "code_exec": {
                    "source_kind": "custom_text",
                    "transport_kind": "function_object_wrapper",
                    "wrapper_field": "input",
                    "expected_canonical_shape": "single_required_string"
                }
            }
        })
    );
    let tools = body["tools"].as_array().expect("anthropic tools");
    assert_eq!(
        tools[0],
        json!({
            "name": "code_exec",
            "description": "Executes code",
            "input_schema": {
                "type": "object",
                "properties": {
                    "input": { "type": "string" }
                },
                "required": ["input"],
                "additionalProperties": false
            }
        })
    );
    assert_eq!(
        body["tool_choice"],
        json!({
            "type": "tool",
            "name": "code_exec"
        })
    );

    let messages = body["messages"].as_array().expect("anthropic messages");
    assert_eq!(messages.len(), 3, "body = {body:?}");
    assert_eq!(messages[0]["role"], "user");
    let assistant_blocks = messages[1]["content"]
        .as_array()
        .expect("assistant content");
    assert_eq!(
        assistant_blocks[0],
        json!({
            "type": "tool_use",
            "id": "call_custom",
            "name": "code_exec",
            "input": { "input": "print('hi')" }
        })
    );
    let user_blocks = messages[2]["content"].as_array().expect("user content");
    assert_eq!(
        user_blocks[0],
        json!({
            "type": "tool_result",
            "tool_use_id": "call_custom",
            "content": "exit 0"
        })
    );
}

#[test]
fn translate_request_claude_structured_tool_result_content_round_trips() {
    let mut body = json!({
        "model": "claude-3",
        "messages": [
            {
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_1",
                    "name": "lookup_weather",
                    "input": { "city": "Tokyo" }
                }]
            },
            {
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "toolu_1",
                    "content": [
                        { "type": "text", "text": "done" },
                        { "type": "json", "json": { "temperature": 22 } }
                    ]
                }]
            }
        ]
    });

    translate_request(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        "claude-3",
        &mut body,
        false,
    )
    .unwrap();

    let messages = body["messages"].as_array().expect("openai messages");
    assert_eq!(messages[1]["role"], "tool");
    assert!(
        messages[1]["content"].is_array(),
        "content should stay structured, body = {body:?}"
    );

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        "claude-3",
        &mut body,
        false,
    )
    .unwrap();

    let content = body["messages"][1]["content"]
        .as_array()
        .expect("claude content");
    assert_eq!(content[0]["type"], "tool_result");
    assert!(content[0]["content"].is_array(), "body = {body:?}");
    assert_eq!(content[0]["content"][0]["type"], "text");
    assert_eq!(content[0]["content"][1]["type"], "json");
}

#[test]
fn translate_request_responses_string_grammar_custom_tool_to_claude_max_compat_bridges_with_text_contract(
) {
    let apply_patch_grammar = r#"start: begin_patch hunk+ end_patch
begin_patch: "*** Begin Patch" LF
end_patch: "*** End Patch" LF?

hunk: add_hunk | delete_hunk | update_hunk
add_hunk: "*** Add File: " filename LF add_line+
delete_hunk: "*** Delete File: " filename LF
update_hunk: "*** Update File: " filename LF change_move? change?

filename: /(.+)/
add_line: "+" /(.*)/ LF -> line

change_move: "*** Move to: " filename LF
change: (change_context | change_line)+ eof_line?
change_context: ("@@" | "@@ " /(.+)/) LF
change_line: ("+" | "-" | " ") /(.*)/ LF
eof_line: "*** End of File" LF

%import common.LF"#;
    let patch_input = "*** Begin Patch\n*** Add File: hello.txt\n+hello\n*** End Patch\n";
    let mut body = json!({
        "model": "claude-3-7-sonnet",
        "tools": [{
            "type": "custom",
            "name": "apply_patch",
            "description": "Use the `apply_patch` tool to edit files. This is a FREEFORM tool, so do not wrap the patch in JSON.",
            "format": {
                "type": "grammar",
                "syntax": "lark",
                "definition": apply_patch_grammar
            }
        }],
        "tool_choice": {
            "type": "custom",
            "name": "apply_patch"
        },
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Create hello.txt" }]
            },
            {
                "type": "custom_tool_call",
                "call_id": "call_apply_patch",
                "name": "apply_patch",
                "input": patch_input
            },
            {
                "type": "custom_tool_call_output",
                "call_id": "call_apply_patch",
                "output": "Success. Updated the following files:\nA hello.txt\n"
            }
        ]
    });

    translate_request_with_policy(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        "claude-3-7-sonnet",
        &mut body,
        request_translation_policy(crate::config::CompatibilityMode::MaxCompat, None),
        false,
    )
    .expect("string-grammar custom tools should bridge to Anthropic under max_compat");

    let tools = body["tools"].as_array().expect("anthropic tools");
    assert_eq!(
        body["_llmup_tool_bridge_context"],
        json!({
            "compatibility_mode": "max_compat",
            "entries": {
                "apply_patch": {
                    "source_kind": "custom_grammar",
                    "transport_kind": "function_object_wrapper",
                    "wrapper_field": "input",
                    "expected_canonical_shape": "single_required_string"
                }
            }
        })
    );
    assert_eq!(tools[0]["name"], "apply_patch");
    assert_eq!(
        tools[0]["input_schema"],
        json!({
            "type": "object",
            "properties": {
                "input": { "type": "string" }
            },
            "required": ["input"],
            "additionalProperties": false
        })
    );
    let description = tools[0]["description"]
        .as_str()
        .expect("anthropic tool description");
    assert!(
        description.contains("Use the `apply_patch` tool to edit files."),
        "description = {description}"
    );
    assert!(
        description.contains("Anthropic will not enforce it structurally"),
        "description = {description}"
    );
    assert!(
        description.contains("syntax: lark"),
        "description = {description}"
    );
    assert!(
        description.contains("start: begin_patch hunk+ end_patch"),
        "description = {description}"
    );
    assert_eq!(
        body["tool_choice"],
        json!({
            "type": "tool",
            "name": "apply_patch"
        })
    );

    let messages = body["messages"].as_array().expect("anthropic messages");
    assert_eq!(messages.len(), 3, "body = {body:?}");
    let assistant_blocks = messages[1]["content"]
        .as_array()
        .expect("assistant content");
    assert_eq!(
        assistant_blocks[0],
        json!({
            "type": "tool_use",
            "id": "call_apply_patch",
            "name": "apply_patch",
            "input": { "input": patch_input }
        })
    );
    let user_blocks = messages[2]["content"].as_array().expect("user content");
    assert_eq!(user_blocks[0]["type"], "tool_result");
    assert_eq!(user_blocks[0]["tool_use_id"], "call_apply_patch");
}

#[test]
fn translate_request_responses_string_grammar_custom_tool_to_claude_max_compat_warns_and_records_mode(
) {
    let mut body = json!({
        "model": "claude-3-7-sonnet",
        "tools": [{
            "type": "custom",
            "name": "apply_patch",
            "description": "Apply a patch",
            "format": {
                "type": "grammar",
                "syntax": "lark",
                "definition": "start: /.+/"
            }
        }],
        "tool_choice": {
            "type": "custom",
            "name": "apply_patch"
        },
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": "Create hello.txt" }]
        }]
    });

    let assessment = super::assessment::assess_request_translation_with_compatibility_mode(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        &body,
        crate::config::CompatibilityMode::MaxCompat,
    );
    let TranslationDecision::AllowWithWarnings(warnings) = assessment.decision() else {
        panic!("expected warning policy, got {assessment:?}");
    };
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("apply_patch") && warning.contains("Anthropic")),
        "warnings = {warnings:?}"
    );

    translate_request_with_policy(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        "claude-3-7-sonnet",
        &mut body,
        request_translation_policy(crate::config::CompatibilityMode::MaxCompat, None),
        false,
    )
    .expect("max_compat grammar custom tools should bridge to Anthropic");

    assert_eq!(
        body["_llmup_tool_bridge_context"]["compatibility_mode"],
        "max_compat"
    );
    let tools = body["tools"].as_array().expect("anthropic tools");
    assert_eq!(tools[0]["name"], "apply_patch");
    assert_eq!(body["tool_choice"]["name"], "apply_patch");
}

#[test]
fn translate_request_responses_string_grammar_custom_tool_to_openai_max_compat_warns_and_records_mode(
) {
    let mut body = json!({
        "model": "gpt-4o",
        "tools": [{
            "type": "custom",
            "name": "apply_patch",
            "description": "Apply a patch",
            "format": {
                "type": "grammar",
                "syntax": "lark",
                "definition": "start: /.+/"
            }
        }],
        "tool_choice": {
            "type": "custom",
            "name": "apply_patch"
        },
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": "Create hello.txt" }]
        }]
    });

    let assessment = super::assessment::assess_request_translation_with_compatibility_mode(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        &body,
        crate::config::CompatibilityMode::MaxCompat,
    );
    let TranslationDecision::AllowWithWarnings(warnings) = assessment.decision() else {
        panic!("expected warning policy, got {assessment:?}");
    };
    assert!(
        warnings.iter().any(|warning| {
            warning.contains("apply_patch") && warning.contains("OpenAI Chat Completions")
        }),
        "warnings = {warnings:?}"
    );

    translate_request_with_policy(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        request_translation_policy(crate::config::CompatibilityMode::MaxCompat, None),
        false,
    )
    .expect("max_compat grammar custom tools should bridge to OpenAI Chat Completions");

    assert_eq!(
        body["_llmup_tool_bridge_context"]["compatibility_mode"],
        "max_compat"
    );
    assert_eq!(body["tools"][0]["function"]["name"], "apply_patch");
    assert_eq!(body["tool_choice"]["function"]["name"], "apply_patch");
    let serialized = body.to_string();
    assert!(
        !serialized.contains("__llmup_custom__"),
        "bridge should keep stable tool names, body = {body:?}"
    );
}

#[test]
fn translate_request_responses_string_grammar_custom_tool_to_openai_max_compat_bridges_with_chat_completions_contract(
) {
    let apply_patch_grammar = r#"start: begin_patch hunk+ end_patch
begin_patch: "*** Begin Patch" LF
end_patch: "*** End Patch" LF?

hunk: add_hunk | delete_hunk | update_hunk
add_hunk: "*** Add File: " filename LF add_line+
delete_hunk: "*** Delete File: " filename LF
update_hunk: "*** Update File: " filename LF change_move? change?

filename: /(.+)/
add_line: "+" /(.*)/ LF -> line

change_move: "*** Move to: " filename LF
change: (change_context | change_line)+ eof_line?
change_context: ("@@" | "@@ " /(.+)/) LF
change_line: ("+" | "-" | " ") /(.*)/ LF
eof_line: "*** End of File" LF

%import common.LF"#;
    let patch_input = "*** Begin Patch\n*** Add File: hello.txt\n+hello\n*** End Patch\n";
    let mut body = json!({
        "model": "gpt-4o",
        "tools": [{
            "type": "custom",
            "name": "apply_patch",
            "description": "Use the `apply_patch` tool to edit files. This is a FREEFORM tool, so do not wrap the patch in JSON.",
            "format": {
                "type": "grammar",
                "syntax": "lark",
                "definition": apply_patch_grammar
            }
        }],
        "tool_choice": {
            "type": "custom",
            "name": "apply_patch"
        },
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Create hello.txt" }]
            },
            {
                "type": "custom_tool_call",
                "call_id": "call_apply_patch",
                "name": "apply_patch",
                "input": patch_input
            },
            {
                "type": "custom_tool_call_output",
                "call_id": "call_apply_patch",
                "output": "Success. Updated the following files:\nA hello.txt\n"
            }
        ]
    });

    translate_request_with_policy(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        "gpt-4o",
        &mut body,
        request_translation_policy(crate::config::CompatibilityMode::MaxCompat, None),
        false,
    )
    .expect(
        "string-grammar custom tools should bridge to OpenAI Chat Completions under max_compat",
    );

    assert_eq!(
        body["_llmup_tool_bridge_context"]["compatibility_mode"],
        "max_compat"
    );
    let tools = body["tools"].as_array().expect("openai tools");
    assert_eq!(tools[0]["function"]["name"], "apply_patch");
    assert_eq!(
        tools[0]["function"]["parameters"],
        json!({
            "type": "object",
            "properties": {
                "input": { "type": "string" }
            },
            "required": ["input"],
            "additionalProperties": false
        })
    );
    let description = tools[0]["function"]["description"]
        .as_str()
        .expect("openai tool description");
    assert!(
        description.contains("Use the `apply_patch` tool to edit files."),
        "description = {description}"
    );
    assert!(
        description.contains("OpenAI Chat Completions receives this tool through the canonical `{ \"input\": string }` wrapper"),
        "description = {description}"
    );
    assert!(
        description.contains("OpenAI Chat Completions will not enforce it structurally"),
        "description = {description}"
    );
    assert!(
        description.contains("syntax: lark"),
        "description = {description}"
    );
    assert!(
        description.contains("start: begin_patch hunk+ end_patch"),
        "description = {description}"
    );
    assert_eq!(
        body["tool_choice"],
        json!({
            "type": "function",
            "function": { "name": "apply_patch" }
        })
    );

    let messages = body["messages"].as_array().expect("openai messages");
    assert_eq!(messages.len(), 3, "body = {body:?}");
    assert_eq!(
        messages[1]["tool_calls"][0],
        json!({
            "id": "call_apply_patch",
            "type": "function",
            "function": {
                "name": "apply_patch",
                "arguments": serde_json::to_string(&json!({ "input": patch_input })).unwrap()
            }
        })
    );
    assert_eq!(messages[2]["role"], "tool");
    assert_eq!(messages[2]["tool_call_id"], "call_apply_patch");
    assert_eq!(
        messages[2]["content"],
        "Success. Updated the following files:\nA hello.txt\n"
    );
}

#[test]
fn translate_request_responses_string_grammar_custom_tool_to_gemini_max_compat_bridges_with_gemini_contract(
) {
    let patch_input = "*** Begin Patch\n*** Add File: hello.txt\n+hello\n*** End Patch\n";
    let mut body = json!({
        "model": "gemini-2.5-flash",
        "tools": [{
            "type": "custom",
            "name": "apply_patch",
            "description": "Apply a patch",
            "format": {
                "type": "grammar",
                "syntax": "lark",
                "definition": "start: /.+/"
            }
        }],
        "tool_choice": {
            "type": "custom",
            "name": "apply_patch"
        },
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Create hello.txt" }]
            },
            {
                "type": "custom_tool_call",
                "call_id": "call_apply_patch",
                "name": "apply_patch",
                "input": patch_input
            },
            {
                "type": "custom_tool_call_output",
                "call_id": "call_apply_patch",
                "output": "Success. Updated the following files:\nA hello.txt\n"
            }
        ]
    });

    translate_request_with_policy(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Google,
        "gemini-2.5-flash",
        &mut body,
        request_translation_policy(crate::config::CompatibilityMode::MaxCompat, None),
        false,
    )
    .expect("string-grammar custom tools should bridge to Gemini under max_compat");

    assert_eq!(
        body["_llmup_tool_bridge_context"]["compatibility_mode"],
        "max_compat"
    );
    assert_eq!(
        body["tools"][0]["functionDeclarations"][0]["name"],
        "apply_patch"
    );
    assert_eq!(
        body["tools"][0]["functionDeclarations"][0]["parameters"],
        json!({
            "type": "object",
            "properties": {
                "input": { "type": "string" }
            },
            "required": ["input"],
            "additionalProperties": false
        })
    );
    assert_eq!(
        body["toolConfig"]["functionCallingConfig"]["allowedFunctionNames"][0],
        "apply_patch"
    );
    assert_eq!(body["contents"][1]["role"], "model");
    assert_eq!(
        body["contents"][1]["parts"][0]["functionCall"],
        json!({
            "id": "call_apply_patch",
            "name": "apply_patch",
            "args": { "input": patch_input }
        })
    );
    assert_eq!(body["contents"][2]["role"], "user");
    assert_eq!(
        body["contents"][2]["parts"][0]["functionResponse"]["name"],
        "apply_patch"
    );
    let serialized = body.to_string();
    assert!(
        !serialized.contains("__llmup_custom__"),
        "Gemini bridge should keep stable tool names, body = {body:?}"
    );
}

#[test]
fn max_compat_structural_repair_pass_canonicalizes_request_scoped_custom_bridge_for_gemini_transport(
) {
    let patch_input = "*** Begin Patch\n*** Add File: hello.txt\n+hello\n*** End Patch\n";
    let mut body = json!({
        "_llmup_tool_bridge_context": {
            "compatibility_mode": "max_compat",
            "entries": {
                "apply_patch": {
                    "source_kind": "custom_grammar",
                    "transport_kind": "function_object_wrapper",
                    "wrapper_field": "input",
                    "expected_canonical_shape": "single_required_string"
                }
            }
        },
        "tools": [{
            "type": "custom",
            "custom": {
                "name": "apply_patch",
                "description": "Apply a patch",
                "format": {
                    "type": "grammar",
                    "syntax": "lark",
                    "definition": "start: /.+/"
                }
            }
        }],
        "tool_choice": {
            "type": "custom",
            "custom": { "name": "apply_patch" }
        },
        "messages": [{
            "role": "assistant",
            "tool_calls": [{
                "id": "call_apply_patch",
                "type": "custom",
                "custom": {
                    "name": "apply_patch",
                    "input": patch_input
                }
            }]
        }]
    });

    super::apply_max_compat_structural_repair_pass(
        crate::config::CompatibilityMode::MaxCompat,
        UpstreamFormat::Google,
        &mut body,
    )
    .expect("repair pass should canonicalize request-scoped custom bridge structures");

    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["tools"][0]["function"]["name"], "apply_patch");
    assert_eq!(
        body["tool_choice"],
        json!({
            "type": "function",
            "function": { "name": "apply_patch" }
        })
    );
    assert_eq!(body["messages"][0]["tool_calls"][0]["type"], "function");
    assert_eq!(
        body["messages"][0]["tool_calls"][0]["function"],
        json!({
            "name": "apply_patch",
            "arguments": serde_json::to_string(&json!({ "input": patch_input })).unwrap()
        })
    );
}

#[test]
fn translate_request_claude_request_scoped_custom_tool_history_to_responses_restores_custom_items()
{
    let exact_input = "first line\n{\"patch\":\"*** Begin Patch\"}\n\"quoted\"";
    let mut body = json!({
        "_llmup_tool_bridge_context": {
            "compatibility_mode": "balanced",
            "entries": {
                "code_exec": {
                    "source_kind": "custom_text",
                    "transport_kind": "function_object_wrapper",
                    "wrapper_field": "input",
                    "expected_canonical_shape": "single_required_string"
                }
            }
        },
        "model": "gpt-5",
        "tools": [{
            "name": "code_exec",
            "description": "Executes code",
            "input_schema": {
                "type": "object",
                "properties": {
                    "input": { "type": "string" }
                },
                "required": ["input"],
                "additionalProperties": false
            }
        }],
        "tool_choice": {
            "type": "tool",
            "name": "code_exec"
        },
        "messages": [
            {
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_custom",
                    "name": "code_exec",
                    "input": { "input": exact_input }
                }]
            },
            {
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "toolu_custom",
                    "content": "exit 0"
                }]
            }
        ]
    });

    translate_request(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        "gpt-5",
        &mut body,
        false,
    )
    .expect("Anthropic bridged custom-tool history should restore Responses semantics");

    let tools = body["tools"].as_array().expect("responses tools");
    assert_eq!(
        tools[0],
        json!({
            "type": "custom",
            "name": "code_exec",
            "description": "Executes code"
        })
    );
    assert_eq!(
        body["tool_choice"],
        json!({
            "type": "custom",
            "name": "code_exec"
        })
    );

    let input = body["input"].as_array().expect("responses input");
    assert_eq!(
        input[0],
        json!({
            "type": "custom_tool_call",
            "call_id": "toolu_custom",
            "name": "code_exec",
            "input": exact_input
        })
    );
    assert_eq!(
        input[1],
        json!({
            "type": "custom_tool_call_output",
            "call_id": "toolu_custom",
            "output": "exit 0"
        })
    );
}

#[test]
fn translate_request_claude_request_scoped_noncanonical_custom_tool_history_falls_back_open() {
    let mut body = json!({
        "_llmup_tool_bridge_context": {
            "compatibility_mode": "balanced",
            "entries": {
                "code_exec": {
                    "source_kind": "custom_text",
                    "transport_kind": "function_object_wrapper",
                    "wrapper_field": "input",
                    "expected_canonical_shape": "single_required_string"
                }
            }
        },
        "model": "gpt-5",
        "messages": [
            {
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_custom",
                    "name": "code_exec",
                    "input": {
                        "input": "print('hi')",
                        "extra": true
                    }
                }]
            },
            {
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "toolu_custom",
                    "content": "exit 0"
                }]
            }
        ]
    });

    translate_request(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        "gpt-5",
        &mut body,
        false,
    )
    .expect("noncanonical bridge payloads should fall back to function semantics");

    let input = body["input"].as_array().expect("responses input");
    assert_eq!(input[0]["type"], "function_call");
    assert_eq!(input[0]["call_id"], "toolu_custom");
    assert_eq!(input[0]["name"], "code_exec");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            input[0]["arguments"].as_str().expect("function arguments"),
        )
        .expect("arguments json"),
        json!({
            "input": "print('hi')",
            "extra": true
        })
    );
    assert_eq!(
        input[1],
        json!({
            "type": "function_call_output",
            "call_id": "toolu_custom",
            "output": "exit 0"
        })
    );
}

#[test]
fn responses_custom_tool_bridge_re_attests_non_replayable_marker_for_openai_tool_call() {
    let mut item = json!({
        "type": "custom_tool_call",
        "call_id": "call_apply_patch",
        "name": "apply_patch",
        "input": "*** Begin Patch\n*** Add File: hello.txt\n+hello\n"
    });
    super::tools::mark_tool_call_as_non_replayable(&mut item);

    let tool_call =
        super::tools::responses_tool_call_item_to_openai_tool_call_with_request_scoped_custom_bridge_strict(&item, "Anthropic")
        .expect("bridge should succeed")
        .expect("tool call");

    assert_eq!(tool_call["type"], "function");
    assert_eq!(tool_call["function"]["name"], "apply_patch");
    assert_eq!(
        tool_call["function"]["arguments"],
        "{\"input\":\"*** Begin Patch\\n*** Add File: hello.txt\\n+hello\\n\"}"
    );
    assert!(
        super::tools::tool_call_is_marked_non_replayable(&tool_call),
        "bridged tool call should keep a trusted replay marker, tool_call = {tool_call:?}"
    );
}

#[test]
fn responses_custom_tool_bridge_round_trip_preserves_trusted_non_replayable_marker() {
    let mut item = json!({
        "type": "custom_tool_call",
        "call_id": "call_apply_patch",
        "name": "apply_patch",
        "input": "*** Begin Patch\n*** Add File: hello.txt\n+hello\n"
    });
    super::tools::mark_tool_call_as_non_replayable(&mut item);

    let tool_call =
        super::tools::responses_tool_call_item_to_openai_tool_call_with_request_scoped_custom_bridge_strict(&item, "Anthropic")
        .expect("bridge should succeed")
        .expect("tool call");

    let round_tripped =
        super::tools::openai_tool_call_to_responses_item_decoding_custom_bridge_with_context(
            &tool_call,
            Some(&json!({
                "compatibility_mode": "balanced",
                "entries": {
                    "apply_patch": {
                        "source_kind": "custom_grammar",
                        "transport_kind": "function_object_wrapper",
                        "wrapper_field": "input",
                        "expected_canonical_shape": "single_required_string"
                    }
                }
            })),
        )
        .expect("decode bridge back to custom tool");

    assert_eq!(round_tripped["type"], "custom_tool_call");
    assert_eq!(round_tripped["call_id"], "call_apply_patch");
    assert_eq!(round_tripped["name"], "apply_patch");
    assert_eq!(
        round_tripped["input"],
        "*** Begin Patch\n*** Add File: hello.txt\n+hello\n"
    );
    assert!(
        super::tools::tool_call_is_marked_non_replayable(&round_tripped),
        "round-tripped custom tool call should keep a trusted replay marker, item = {round_tripped:?}"
    );
}

#[test]
fn translate_request_openai_bridged_marked_custom_tool_call_to_claude_degrades_to_text() {
    let mut responses_item = json!({
        "type": "custom_tool_call",
        "call_id": "call_apply_patch",
        "name": "apply_patch",
        "input": "*** Begin Patch\n*** Add File: hello.txt\n+hello\n"
    });
    super::tools::mark_tool_call_as_non_replayable(&mut responses_item);
    let bridged_tool_call =
        super::tools::responses_tool_call_item_to_openai_tool_call_with_request_scoped_custom_bridge_strict(&responses_item, "Anthropic")
        .expect("bridge should succeed")
        .expect("tool call");

    let mut body = json!({
        "model": "claude-3",
        "messages": [{
            "role": "assistant",
            "tool_calls": [bridged_tool_call]
        }]
    });

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        "claude-3",
        &mut body,
        false,
    )
    .expect("trusted bridged marker should degrade instead of replaying as structured tool use");

    let blocks = body["messages"][0]["content"]
        .as_array()
        .expect("claude assistant blocks");
    assert_eq!(blocks[0]["type"], "text", "body = {body:?}");
    let text = blocks[0]["text"].as_str().expect("text block");
    assert!(text.contains("apply_patch"), "text = {text}");
    assert!(text.contains("*** Begin Patch"), "text = {text}");
}

#[test]
fn translate_request_claude_to_openai_omitted_stream_defaults_false() {
    let mut body = json!({
        "model": "claude-3",
        "messages": [{ "role": "user", "content": "Hi" }]
    });
    translate_request(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        "claude-3",
        &mut body,
        false,
    )
    .unwrap();
    assert_eq!(body["stream"], false);
}

#[test]
fn translate_request_claude_signed_thinking_preserves_text_and_responses_carrier() {
    let body = json!({
        "model": "claude-3",
        "messages": [{
            "role": "assistant",
            "content": [{
                "type": "thinking",
                "thinking": "internal reasoning",
                "signature": "sig_123"
            }]
        }]
    });

    for upstream_format in [
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiResponses,
    ] {
        let mut translated = body.clone();
        translate_request(
            UpstreamFormat::Anthropic,
            upstream_format,
            "claude-3",
            &mut translated,
            false,
        )
        .expect("signed thinking provenance should degrade instead of fail closed");

        match upstream_format {
            UpstreamFormat::OpenAiCompletion => {
                assert_eq!(
                    translated["messages"][0]["reasoning_content"],
                    "internal reasoning"
                );
                assert!(translated["messages"][0]
                    .get("_anthropic_reasoning_replay")
                    .is_none());
            }
            UpstreamFormat::Google => {
                let parts = translated["contents"][0]["parts"]
                    .as_array()
                    .expect("gemini parts");
                assert_eq!(parts[0]["thought"], true);
                assert_eq!(parts[0]["text"], "internal reasoning");
            }
            UpstreamFormat::OpenAiResponses => {
                assert_eq!(translated["input"][0]["type"], "reasoning");
                assert_eq!(
                    translated["input"][0]["summary"][0]["text"],
                    "internal reasoning"
                );
                assert!(translated["input"][0]["encrypted_content"].is_string());
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn translate_request_responses_to_claude_replays_anthropic_reasoning_carrier() {
    let signed_response = json!({
        "id": "msg_sig",
        "content": [
            {
                "type": "thinking",
                "thinking": "internal reasoning",
                "signature": "sig_123"
            },
            { "type": "text", "text": "Visible answer" }
        ],
        "stop_reason": "end_turn",
        "model": "claude-3"
    });
    let translated_response = translate_response(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &signed_response,
    )
    .expect("Anthropic signed thinking should translate to Responses");
    let output = translated_response["output"]
        .as_array()
        .expect("responses output");
    let reasoning_item = output
        .iter()
        .find(|item| item["type"] == "reasoning")
        .expect("reasoning item")
        .clone();
    let message_item = output
        .iter()
        .find(|item| item["type"] == "message")
        .expect("message item")
        .clone();

    let mut body = json!({
        "model": "claude-3",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Think about it" }]
            },
            reasoning_item,
            message_item,
            {
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Continue" }]
            }
        ]
    });

    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        "claude-3",
        &mut body,
        false,
    )
    .expect("Responses reasoning carrier should replay to Anthropic");

    let messages = body["messages"].as_array().expect("anthropic messages");
    assert_eq!(messages[1]["role"], "assistant");
    let assistant_content = messages[1]["content"]
        .as_array()
        .expect("assistant content");
    assert_eq!(assistant_content[0]["type"], "thinking");
    assert_eq!(assistant_content[0]["thinking"], "internal reasoning");
    assert_eq!(assistant_content[0]["signature"], "sig_123");
    assert_eq!(assistant_content[1]["type"], "text");
    assert_eq!(assistant_content[1]["text"], "Visible answer");
}

#[test]
fn translate_request_responses_to_claude_replays_anthropic_omitted_reasoning_carrier() {
    let omitted_response = json!({
        "id": "msg_omitted",
        "content": [
            {
                "type": "thinking",
                "thinking": { "display": "omitted" },
                "signature": "sig_omitted"
            },
            { "type": "text", "text": "Visible answer" }
        ],
        "stop_reason": "end_turn",
        "model": "claude-3"
    });
    let translated_response = translate_response(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &omitted_response,
    )
    .expect("Anthropic omitted thinking should translate to Responses");
    let output = translated_response["output"]
        .as_array()
        .expect("responses output");
    let reasoning_item = output
        .iter()
        .find(|item| item["type"] == "reasoning")
        .expect("reasoning item")
        .clone();
    let message_item = output
        .iter()
        .find(|item| item["type"] == "message")
        .expect("message item")
        .clone();

    let mut body = json!({
        "model": "claude-3",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Think about it" }]
            },
            reasoning_item,
            message_item,
            {
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Continue" }]
            }
        ]
    });

    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        "claude-3",
        &mut body,
        false,
    )
    .expect("Responses omitted carrier should replay to Anthropic");

    let messages = body["messages"].as_array().expect("anthropic messages");
    assert_eq!(messages[1]["role"], "assistant");
    let assistant_content = messages[1]["content"]
        .as_array()
        .expect("assistant content");
    assert_eq!(assistant_content[0]["type"], "thinking");
    assert_eq!(
        assistant_content[0]["thinking"],
        json!({ "display": "omitted" })
    );
    assert_eq!(assistant_content[0]["signature"], "sig_omitted");
    assert_eq!(assistant_content[1]["type"], "text");
    assert_eq!(assistant_content[1]["text"], "Visible answer");
}

#[test]
fn translate_request_claude_omitted_thinking_drops_hidden_reasoning_and_preserves_responses_carrier(
) {
    let body = json!({
        "model": "claude-3",
        "messages": [{
            "role": "assistant",
            "content": [
                {
                    "type": "thinking",
                    "thinking": { "display": "omitted" },
                    "signature": "sig_123"
                },
                { "type": "text", "text": "Visible answer" }
            ]
        }]
    });

    for upstream_format in [
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Google,
    ] {
        let mut translated = body.clone();
        translate_request(
            UpstreamFormat::Anthropic,
            upstream_format,
            "claude-3",
            &mut translated,
            false,
        )
        .expect("omitted thinking should be dropped instead of fail closed");

        match upstream_format {
            UpstreamFormat::OpenAiCompletion => {
                assert_eq!(translated["messages"][0]["content"], "Visible answer");
                assert!(translated["messages"][0].get("reasoning_content").is_none());
            }
            UpstreamFormat::OpenAiResponses => {
                assert_eq!(translated["input"].as_array().unwrap().len(), 2);
                assert_eq!(translated["input"][0]["type"], "reasoning");
                assert_eq!(translated["input"][0]["summary"], json!([]));
                assert!(translated["input"][0]["encrypted_content"].is_string());
                assert_eq!(translated["input"][1]["type"], "message");
                assert_eq!(
                    translated["input"][1]["content"][0]["text"],
                    "Visible answer"
                );
            }
            UpstreamFormat::Google => {
                let parts = translated["contents"][0]["parts"]
                    .as_array()
                    .expect("gemini parts");
                assert_eq!(parts.len(), 1);
                assert_eq!(parts[0]["text"], "Visible answer");
                assert!(parts[0].get("thought").is_none());
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn translate_request_claude_to_non_anthropic_rejects_nonportable_tool_definition_metadata() {
    let tool_cases = [
        (
            "strict",
            json!({
                "name": "lookup_weather",
                "description": "Weather lookup",
                "input_schema": { "type": "object", "properties": {} },
                "strict": true
            }),
        ),
        (
            "defer_loading",
            json!({
                "name": "lookup_weather",
                "description": "Weather lookup",
                "input_schema": { "type": "object", "properties": {} },
                "defer_loading": true
            }),
        ),
        (
            "allowed_callers",
            json!({
                "name": "lookup_weather",
                "description": "Weather lookup",
                "input_schema": { "type": "object", "properties": {} },
                "allowed_callers": ["assistant"]
            }),
        ),
        (
            "input_examples",
            json!({
                "name": "lookup_weather",
                "description": "Weather lookup",
                "input_schema": { "type": "object", "properties": {} },
                "input_examples": [{
                    "city": "Tokyo"
                }]
            }),
        ),
        (
            "server-side",
            json!({
                "type": "web_search_20250305",
                "name": "web_search"
            }),
        ),
    ];

    for (label, tool) in tool_cases {
        for upstream_format in [
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            UpstreamFormat::Google,
        ] {
            let mut body = json!({
                "model": "claude-3",
                "messages": [{ "role": "user", "content": "Hi" }],
                "tools": [tool.clone()]
            });

            let err = translate_request(
                UpstreamFormat::Anthropic,
                upstream_format,
                "claude-3",
                &mut body,
                false,
            )
            .expect_err("Anthropic tool metadata should fail closed");

            assert!(err.contains("tool"), "label = {label}, err = {err}");
        }
    }
}

#[test]
fn translate_request_claude_to_non_anthropic_allows_supported_tool_definition_fields() {
    for upstream_format in [
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Google,
    ] {
        let mut body = json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "tools": [{
                "name": "lookup_weather",
                "description": "Weather lookup",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "city": { "type": "string" }
                    }
                }
            }]
        });

        translate_request(
            UpstreamFormat::Anthropic,
            upstream_format,
            "claude-3",
            &mut body,
            false,
        )
        .expect("supported Anthropic tool fields should translate");
    }
}

#[test]
fn translate_request_claude_to_non_anthropic_rejects_user_turn_that_would_reorder_tool_results() {
    let mut body = json!({
        "model": "claude-3",
        "messages": [
            {
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_1",
                    "name": "lookup_weather",
                    "input": { "city": "Tokyo" }
                }]
            },
            {
                "role": "user",
                "content": [
                    { "type": "text", "text": "Before the result" },
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_1",
                        "content": "sunny"
                    }
                ]
            }
        ]
    });

    let err = translate_request(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        "claude-3",
        &mut body,
        false,
    )
    .expect_err("mixed user turns that reorder tool_results should fail closed");

    assert!(err.contains("tool_result"), "err = {err}");
    assert!(err.contains("order"), "err = {err}");
}

#[test]
fn translate_request_claude_to_openai_preserves_multiblock_system_without_injected_newlines() {
    let mut body = json!({
        "model": "claude-3",
        "system": [
            { "type": "text", "text": "System A" },
            { "type": "text", "text": "System B" }
        ],
        "messages": [{ "role": "user", "content": "Hi" }]
    });

    translate_request(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        "claude-3",
        &mut body,
        false,
    )
    .unwrap();

    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages[0]["role"], "system");
    let content = messages[0]["content"].as_array().expect("system parts");
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[0]["text"], "System A");
    assert_eq!(content[1]["type"], "text");
    assert_eq!(content[1]["text"], "System B");
}

#[test]
fn translate_request_claude_to_gemini_preserves_multiblock_system_without_injected_newlines() {
    let mut body = json!({
        "model": "claude-3",
        "system": [
            { "type": "text", "text": "System A" },
            { "type": "text", "text": "System B" }
        ],
        "messages": [{ "role": "user", "content": "Hi" }]
    });

    translate_request(
        UpstreamFormat::Anthropic,
        UpstreamFormat::Google,
        "claude-3",
        &mut body,
        false,
    )
    .unwrap();

    let parts = body["systemInstruction"]["parts"]
        .as_array()
        .expect("system instruction parts");
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0]["text"], "System A");
    assert_eq!(parts[1]["text"], "System B");
}

#[test]
fn translate_request_claude_to_openai_drops_warning_only_request_controls_on_openai_completion_lane(
) {
    let mut body = json!({
        "model": "claude-3",
        "thinking": {
            "type": "enabled",
            "budget_tokens": 2048
        },
        "context_management": {
            "type": "auto"
        },
        "cache_control": { "type": "ephemeral" },
        "system": [{
            "type": "text",
            "text": "System policy",
            "cache_control": { "type": "ephemeral" }
        }],
        "messages": [{
            "role": "user",
            "content": [{
                "type": "text",
                "text": "Hi",
                "cache_control": { "type": "ephemeral" }
            }]
        }]
    });

    translate_request(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        "claude-3",
        &mut body,
        false,
    )
    .expect("warning-only Anthropic controls should be dropped for OpenAI completion lane");

    assert!(body.get("thinking").is_none(), "body = {body:?}");
    assert!(body.get("context_management").is_none(), "body = {body:?}");
    assert!(body.get("cache_control").is_none(), "body = {body:?}");

    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 2, "messages = {messages:?}");
    assert_eq!(messages[0]["role"], "system");
    let system_parts = messages[0]["content"].as_array().expect("system parts");
    assert_eq!(system_parts.len(), 1, "system_parts = {system_parts:?}");
    assert_eq!(system_parts[0]["type"], "text");
    assert_eq!(system_parts[0]["text"], "System policy");
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(messages[1]["content"], "Hi");
}

#[test]
fn translate_request_claude_to_openai_maps_tool_choice_and_parallel_calls() {
    let mut body = json!({
        "model": "claude-3",
        "tool_choice": {
            "type": "tool",
            "name": "lookup",
            "disable_parallel_tool_use": true
        },
        "tools": [{
            "name": "lookup",
            "input_schema": { "type": "object", "properties": {} }
        }],
        "messages": [{ "role": "user", "content": "Hi" }]
    });

    translate_request(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        "claude-3",
        &mut body,
        false,
    )
    .expect("Anthropic tool_choice should map to OpenAI tool controls");

    assert_eq!(body["tool_choice"]["type"], "function");
    assert_eq!(body["tool_choice"]["function"]["name"], "lookup");
    assert_eq!(body["parallel_tool_calls"], false);
}

#[test]
fn translate_request_claude_to_openai_rejects_unsupported_document_block() {
    let mut body = json!({
        "model": "claude-3",
        "messages": [{
            "role": "user",
            "content": [{
                "type": "document",
                "source": {
                    "type": "base64",
                    "media_type": "application/pdf",
                    "data": "JVBERi0x"
                }
            }]
        }]
    });

    let err = translate_request(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        "claude-3",
        &mut body,
        false,
    )
    .expect_err("unsupported typed blocks should not be silently dropped");

    assert!(err.contains("document"), "err = {err}");
}

#[test]
fn translate_request_claude_to_openai_rejects_future_unknown_block() {
    let mut body = json!({
        "model": "claude-3",
        "messages": [{
            "role": "user",
            "content": [{
                "type": "mystery_block",
                "payload": "???"
            }]
        }]
    });

    let err = translate_request(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        "claude-3",
        &mut body,
        false,
    )
    .expect_err("future unknown anthropic blocks should fail closed");

    assert!(err.contains("mystery_block"), "err = {err}");
}

#[test]
fn translate_request_claude_to_openai_allows_business_cache_control_keys() {
    let mut body = json!({
        "model": "claude-3",
        "messages": [
            { "role": "user", "content": "Use the tool" },
            {
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_1",
                    "name": "lookup_weather",
                    "input": {
                        "city": "Tokyo",
                        "cache_control": "business-value"
                    }
                }]
            }
        ],
        "tools": [{
            "name": "lookup_weather",
            "description": "Weather lookup",
            "input_schema": {
                "type": "object",
                "properties": {
                    "city": { "type": "string" },
                    "cache_control": { "type": "string" }
                }
            }
        }]
    });

    translate_request(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        "claude-3",
        &mut body,
        false,
    )
    .unwrap();

    let tool_calls = body["messages"][1]["tool_calls"]
        .as_array()
        .expect("assistant tool calls");
    let arguments: Value = serde_json::from_str(
        tool_calls[0]["function"]["arguments"]
            .as_str()
            .expect("arguments string"),
    )
    .expect("tool arguments json");
    assert_eq!(arguments["cache_control"], "business-value");
    assert_eq!(
        body["tools"][0]["function"]["parameters"]["properties"]["cache_control"]["type"],
        "string"
    );
}

#[test]
fn translate_request_openai_invalid_tool_arguments_to_claude_rejects_instead_of_coercing_empty_object(
) {
    let mut body = json!({
        "model": "claude-3",
        "messages": [{
            "role": "assistant",
            "tool_calls": [{
                "id": "call_1",
                "type": "function",
                "function": {
                    "name": "lookup_weather",
                    "arguments": "{\"city\":\"Tokyo\""
                }
            }]
        }]
    });

    let err = translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        "claude-3",
        &mut body,
        false,
    )
    .expect_err("invalid JSON arguments should fail closed");

    assert!(err.contains("arguments"), "err = {err}");
    assert!(err.contains("JSON"), "err = {err}");
}

#[test]
fn translate_request_openai_non_object_tool_arguments_to_gemini_rejects() {
    let mut body = json!({
        "model": "gemini-2.5-flash",
        "messages": [{
            "role": "assistant",
            "tool_calls": [{
                "id": "call_1",
                "type": "function",
                "function": {
                    "name": "lookup_weather",
                    "arguments": "[]"
                }
            }]
        }]
    });

    let err = translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        "gemini-2.5-flash",
        &mut body,
        false,
    )
    .expect_err("non-object tool input should fail closed");

    assert!(
        err.contains("JSON object"),
        "expected object-specific failure, got {err}"
    );
}

#[test]
fn translate_request_openai_marked_invalid_tool_arguments_to_claude_degrade_to_text() {
    let mut body = json!({
        "model": "claude-3",
        "messages": [{
            "role": "assistant",
            "tool_calls": [{
                "id": "call_1",
                "type": "function",
                "_llmup_non_replayable_tool_call": { "reason": "incomplete_arguments" },
                "function": {
                    "name": "lookup_weather",
                    "arguments": "{\"city\":\"Tokyo\""
                }
            }]
        }]
    });
    super::tools::mark_tool_call_as_non_replayable(&mut body["messages"][0]["tool_calls"][0]);

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        "claude-3",
        &mut body,
        false,
    )
    .expect("marked incomplete tool calls should degrade to text");

    let blocks = body["messages"][0]["content"]
        .as_array()
        .expect("claude assistant blocks");
    assert_eq!(blocks[0]["type"], "text");
    let text = blocks[0]["text"].as_str().expect("text block");
    assert!(text.contains("lookup_weather"), "text = {text}");
    assert!(text.contains("{\"city\":\"Tokyo\""), "text = {text}");
}

#[test]
fn translate_request_openai_unsigned_non_replayable_marker_to_claude_still_fails_closed() {
    let mut body = json!({
        "model": "claude-3",
        "messages": [{
            "role": "assistant",
            "tool_calls": [{
                "id": "call_1",
                "type": "function",
                "_llmup_non_replayable_tool_call": { "reason": "incomplete_arguments" },
                "function": {
                    "name": "lookup_weather",
                    "arguments": "{\"city\":\"Tokyo\""
                }
            }]
        }]
    });

    let err = translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        "claude-3",
        &mut body,
        false,
    )
    .expect_err("unsigned replay marker must not bypass structured-args validation");

    assert!(err.contains("arguments"), "err = {err}");
    assert!(err.contains("JSON"), "err = {err}");
}

#[test]
fn translate_request_openai_marked_invalid_tool_arguments_to_gemini_degrade_to_text() {
    let mut body = json!({
        "model": "gemini-2.5-flash",
        "messages": [{
            "role": "assistant",
            "tool_calls": [{
                "id": "call_1",
                "type": "function",
                "_llmup_non_replayable_tool_call": { "reason": "incomplete_arguments" },
                "function": {
                    "name": "lookup_weather",
                    "arguments": "{\"city\":\"Tokyo\""
                }
            }]
        }]
    });
    super::tools::mark_tool_call_as_non_replayable(&mut body["messages"][0]["tool_calls"][0]);

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        "gemini-2.5-flash",
        &mut body,
        false,
    )
    .expect("marked incomplete tool calls should degrade to text");

    let parts = body["contents"][0]["parts"]
        .as_array()
        .expect("gemini parts");
    assert_eq!(
        parts[0]["text"],
        "Tool call `lookup_weather` with partial arguments: {\"city\":\"Tokyo\""
    );
    assert!(parts[0].get("functionCall").is_none(), "body = {body:?}");
}

#[test]
fn translate_request_claude_to_openai_collapses_text_blocks_to_string() {
    let mut body = json!({
        "model": "claude-3",
        "messages": [{
            "role": "user",
            "content": [
                { "type": "text", "text": "alpha\n" },
                { "type": "text", "text": "beta" }
            ]
        }]
    });
    translate_request(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        "claude-3",
        &mut body,
        false,
    )
    .unwrap();
    assert_eq!(body["messages"][0]["content"], "alpha\nbeta");
}

#[test]
fn translate_request_claude_to_openai_drops_metadata_for_compatibility() {
    let mut body = json!({
        "model": "claude-3",
        "metadata": { "user_id": "abc" },
        "messages": [{ "role": "user", "content": "Hi" }]
    });
    translate_request(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        "claude-3",
        &mut body,
        false,
    )
    .unwrap();
    assert!(body.get("metadata").is_none());
}

#[test]
fn translate_request_gemini_to_openai_omitted_stream_defaults_false() {
    let mut body = json!({
        "contents": [{ "parts": [{ "text": "Hi" }] }]
    });
    translate_request(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiCompletion,
        "gemini-1.5",
        &mut body,
        false,
    )
    .unwrap();
    assert_eq!(body["stream"], false);
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["messages"][0]["content"], "Hi");
}

#[test]
fn translate_request_gemini_to_openai_missing_role_preserves_text() {
    let mut body = json!({
        "model": "gemini-1.5",
        "contents": [{ "parts": [{ "text": "Reply with exactly: ok" }] }]
    });
    translate_request(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiCompletion,
        "gemini-1.5",
        &mut body,
        true,
    )
    .unwrap();
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["messages"][0]["content"], "Reply with exactly: ok");
}

#[test]
fn translate_request_openai_to_gemini_rejects_n_greater_than_one() {
    let mut body = json!({
        "model": "gemini-2.5-flash",
        "n": 2,
        "messages": [{ "role": "user", "content": "Hi" }]
    });

    let err = translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        "gemini-2.5-flash",
        &mut body,
        false,
    )
    .expect_err("cross-protocol n > 1 should fail closed");

    assert!(err.contains("n"), "err = {err}");
    assert!(err.contains("single"), "err = {err}");
}

#[test]
fn translate_request_gemini_to_openai_rejects_candidate_count_greater_than_one() {
    let mut body = json!({
        "model": "gemini-2.5-flash",
        "generationConfig": { "candidateCount": 2 },
        "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }]
    });

    let err = translate_request(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiCompletion,
        "gemini-2.5-flash",
        &mut body,
        false,
    )
    .expect_err("cross-protocol candidateCount > 1 should fail closed");

    assert!(err.contains("candidateCount"), "err = {err}");
    assert!(err.contains("single"), "err = {err}");
}

#[test]
fn translate_request_gemini_to_openai_accepts_snake_case_parts() {
    let mut body = json!({
        "model": "gemini-1.5",
        "contents": [{
            "parts": [
                {
                    "inline_data": {
                        "mime_type": "image/jpeg",
                        "data": "abc123"
                    }
                },
                {
                    "function_call": {
                        "id": "call_1",
                        "name": "lookup_weather",
                        "args": { "city": "Tokyo" }
                    }
                }
            ]
        }]
    });
    translate_request(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiCompletion,
        "gemini-1.5",
        &mut body,
        false,
    )
    .unwrap();
    assert_eq!(body["messages"][0]["role"], "assistant");
    assert_eq!(body["messages"][0]["tool_calls"][0]["id"], "call_1");
    assert_eq!(
        body["messages"][0]["tool_calls"][0]["function"]["name"],
        "lookup_weather"
    );
    assert!(body["messages"][0]["content"]
        .as_array()
        .expect("array content")
        .iter()
        .any(|part| part["type"] == "image_url"));
}

#[test]
fn translate_request_gemini_to_openai_preserves_text_and_function_response_order() {
    let mut body = json!({
        "model": "gemini-1.5",
        "contents": [{
            "role": "user",
            "parts": [
                { "text": "Before tool." },
                {
                    "functionResponse": {
                        "id": "call_1",
                        "name": "lookup_weather",
                        "response": { "result": { "temperature": 22 } }
                    }
                },
                { "text": "After tool." }
            ]
        }]
    });
    translate_request(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiCompletion,
        "gemini-1.5",
        &mut body,
        false,
    )
    .unwrap();
    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"], "Before tool.");
    assert_eq!(messages[1]["role"], "tool");
    assert_eq!(messages[1]["tool_call_id"], "call_1");
    assert_eq!(messages[2]["role"], "user");
    assert_eq!(messages[2]["content"], "After tool.");
}

#[test]
fn translate_request_gemini_to_non_gemini_rejects_function_response_parts() {
    let body = json!({
        "model": "gemini-1.5",
        "contents": [{
            "role": "user",
            "parts": [{
                "functionResponse": {
                    "id": "call_1",
                    "name": "lookup_weather",
                    "response": {
                        "result": {
                            "parts": [
                                { "text": "sunny" }
                            ]
                        }
                    }
                }
            }]
        }]
    });

    for upstream_format in [
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
    ] {
        let mut translated = body.clone();
        let err = translate_request(
            UpstreamFormat::Google,
            upstream_format,
            "gemini-1.5",
            &mut translated,
            false,
        )
        .expect_err("Gemini functionResponse.parts should fail closed");

        assert!(err.contains("functionResponse"), "err = {err}");
        assert!(err.contains("parts"), "err = {err}");
    }
}

#[test]
fn convert_gemini_content_to_openai_rejects_function_response_result_parts_at_boundary() {
    let content = json!({
        "role": "user",
        "parts": [{
            "functionResponse": {
                "id": "call_1",
                "name": "lookup_weather",
                "response": {
                    "result": {
                        "parts": [{ "text": "sunny" }]
                    }
                }
            }
        }]
    });

    let err = convert_gemini_content_to_openai(&content)
        .expect_err("functionResponse.response.result.parts should fail closed at conversion");

    assert!(err.contains("functionResponse"), "err = {err}");
    assert!(err.contains("parts"), "err = {err}");
}

#[test]
fn translate_request_gemini_to_openai_allows_structured_function_response_without_parts() {
    let mut body = json!({
        "model": "gemini-1.5",
        "contents": [{
            "role": "user",
            "parts": [{
                "functionResponse": {
                    "id": "call_1",
                    "name": "lookup_weather",
                    "response": { "result": { "temperature": 22 } }
                }
            }]
        }]
    });

    translate_request(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiCompletion,
        "gemini-1.5",
        &mut body,
        false,
    )
    .expect("structured functionResponse.response should still translate");

    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], "tool");
    assert_eq!(messages[0]["tool_call_id"], "call_1");
}

#[test]
fn translate_request_gemini_to_openai_streaming_does_not_coalesce_parallel_tool_results() {
    let mut body = json!({
        "model": "minimax-anth",
        "contents": [
            {
                "role": "user",
                "parts": [{ "text": "<session_context>workspace snapshot</session_context>" }]
            },
            {
                "role": "user",
                "parts": [{ "text": "Inspect calc.py and main.py." }]
            },
            {
                "role": "model",
                "parts": [
                    { "text": "\n" },
                    {
                        "functionCall": {
                            "id": "call_function_1",
                            "name": "read_file",
                            "args": { "file_path": "/tmp/calc.py" }
                        }
                    },
                    {
                        "functionCall": {
                            "id": "call_function_2",
                            "name": "read_file",
                            "args": { "file_path": "/tmp/main.py" }
                        }
                    }
                ]
            },
            {
                "role": "user",
                "parts": [
                    {
                        "functionResponse": {
                            "id": "call_function_1",
                            "name": "read_file",
                            "response": {
                                "output": "def add(a, b):\n    return a - b\n"
                            }
                        }
                    },
                    {
                        "functionResponse": {
                            "id": "call_function_2",
                            "name": "read_file",
                            "response": {
                                "output": "print(add(2, 3))\n"
                            }
                        }
                    }
                ]
            }
        ]
    });

    translate_request(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiCompletion,
        "MiniMax-M2.7-highspeed",
        &mut body,
        true,
    )
    .expect("realistic streaming Gemini request should preserve both tool results");

    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 4, "messages = {messages:?}");
    assert_eq!(messages[0]["role"], "user");
    assert!(messages[0]["content"]
        .as_str()
        .expect("string content")
        .contains("Inspect calc.py and main.py."));
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[2]["role"], "tool");
    assert_eq!(messages[2]["tool_call_id"], "call_function_1");
    assert_eq!(
        messages[2]["content"],
        "{\"output\":\"def add(a, b):\\n    return a - b\\n\"}"
    );
    assert_eq!(messages[3]["role"], "tool");
    assert_eq!(messages[3]["tool_call_id"], "call_function_2");
    assert_eq!(
        messages[3]["content"],
        "{\"output\":\"print(add(2, 3))\\n\"}"
    );
}

#[test]
fn translate_request_openai_streaming_still_coalesces_plain_string_user_messages() {
    let mut body = json!({
        "model": "minimax-openai",
        "messages": [
            { "role": "user", "content": "alpha" },
            { "role": "user", "content": "beta" }
        ]
    });

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiCompletion,
        "minimax-openai",
        &mut body,
        true,
    )
    .expect("plain user messages should still coalesce");

    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 1, "messages = {messages:?}");
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"], "alpha\n\nbeta");
}

#[test]
fn translate_request_openai_to_gemini_keeps_json_tool_results_on_response_result_path() {
    let mut body = json!({
        "model": "gemini-1.5",
        "messages": [
            {
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": { "name": "lookup_weather", "arguments": "{\"city\":\"Tokyo\"}" }
                }]
            },
            {
                "role": "tool",
                "tool_call_id": "call_1",
                "content": { "temperature": 22, "unit": "C" }
            }
        ]
    });

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        "gemini-1.5",
        &mut body,
        false,
    )
    .unwrap();

    let function_response = &body["contents"][1]["parts"][0]["functionResponse"];
    assert_eq!(function_response["response"]["result"]["temperature"], 22);
    assert!(function_response.get("parts").is_none(), "body = {body:?}");
}

#[test]
fn translate_request_responses_to_gemini_keeps_json_tool_results_on_response_result_path() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": [
            {
                "type": "function_call",
                "call_id": "call_1",
                "name": "lookup_weather",
                "arguments": "{\"city\":\"Tokyo\"}"
            },
            {
                "type": "function_call_output",
                "call_id": "call_1",
                "output": { "temperature": 22, "unit": "C" }
            }
        ]
    });

    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Google,
        "gemini-1.5",
        &mut body,
        false,
    )
    .unwrap();

    let function_response = &body["contents"][1]["parts"][0]["functionResponse"];
    assert_eq!(function_response["response"]["result"]["temperature"], 22);
    assert!(function_response.get("parts").is_none(), "body = {body:?}");
}

#[test]
fn translate_request_responses_marked_invalid_tool_arguments_to_claude_degrade_to_text() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": [{
            "type": "function_call",
            "call_id": "call_1",
            "name": "lookup_weather",
            "arguments": "{\"city\":\"Tokyo\"",
            "_llmup_non_replayable_tool_call": { "reason": "incomplete_arguments" }
        }]
    });
    super::tools::mark_tool_call_as_non_replayable(&mut body["input"][0]);

    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        "claude-3",
        &mut body,
        false,
    )
    .expect("marked incomplete responses tool calls should degrade to text");

    let blocks = body["messages"][0]["content"]
        .as_array()
        .expect("claude assistant blocks");
    assert_eq!(blocks[0]["type"], "text");
    let text = blocks[0]["text"].as_str().expect("text block");
    assert!(text.contains("lookup_weather"), "text = {text}");
    assert!(text.contains("{\"city\":\"Tokyo\""), "text = {text}");
}

#[test]
fn translate_request_responses_marked_custom_tool_call_output_to_claude_degrades_to_user_text() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Apply this patch" }]
            },
            {
                "type": "custom_tool_call",
                "call_id": "call_patch",
                "name": "apply_patch",
                "input": "*** Begin Patch\n*** Add File: hello.txt\n+hello\n"
            },
            {
                "type": "custom_tool_call_output",
                "call_id": "call_patch",
                "output": "Success. Updated the following files:\nA hello.txt\n"
            }
        ]
    });
    super::tools::mark_tool_call_as_non_replayable(&mut body["input"][1]);

    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        "claude-3",
        &mut body,
        false,
    )
    .expect("marked tool call outputs should degrade to user text instead of failing");

    let messages = body["messages"].as_array().expect("claude messages");
    assert_eq!(messages.len(), 3, "body = {body:?}");
    let assistant_blocks = messages[1]["content"]
        .as_array()
        .expect("assistant content");
    assert_eq!(assistant_blocks[0]["type"], "text");
    let assistant_text = assistant_blocks[0]["text"]
        .as_str()
        .expect("assistant text");
    assert!(
        assistant_text.contains("apply_patch"),
        "text = {assistant_text}"
    );

    let user_blocks = messages[2]["content"].as_array().expect("user content");
    assert_eq!(user_blocks.len(), 1, "body = {body:?}");
    assert_eq!(user_blocks[0]["type"], "text");
    let user_text = user_blocks[0]["text"].as_str().expect("user text");
    assert!(user_text.contains("apply_patch"), "text = {user_text}");
    assert!(
        user_text.contains("Updated the following files"),
        "text = {user_text}"
    );
}

#[test]
fn translate_request_responses_marked_function_tool_call_output_to_claude_degrades_to_user_text() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Check weather" }]
            },
            {
                "type": "function_call",
                "call_id": "call_weather",
                "name": "lookup_weather",
                "arguments": "{\"city\":\"Tokyo\""
            },
            {
                "type": "function_call_output",
                "call_id": "call_weather",
                "output": { "temperature": 22, "unit": "C" }
            }
        ]
    });
    super::tools::mark_tool_call_as_non_replayable(&mut body["input"][1]);

    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        "claude-3",
        &mut body,
        false,
    )
    .expect("marked function tool outputs should degrade to user text");

    let messages = body["messages"].as_array().expect("claude messages");
    assert_eq!(messages.len(), 3, "body = {body:?}");
    let user_blocks = messages[2]["content"].as_array().expect("user content");
    assert_eq!(user_blocks.len(), 1, "body = {body:?}");
    assert_eq!(user_blocks[0]["type"], "text");
    let user_text = user_blocks[0]["text"].as_str().expect("user text");
    assert!(user_text.contains("lookup_weather"), "text = {user_text}");
    assert!(
        user_text.contains("\"temperature\":22"),
        "text = {user_text}"
    );
}

#[test]
fn translate_request_responses_unsigned_non_replayable_marker_to_claude_still_fails_closed() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": [{
            "type": "function_call",
            "call_id": "call_1",
            "name": "lookup_weather",
            "arguments": "{\"city\":\"Tokyo\"",
            "_llmup_non_replayable_tool_call": { "reason": "incomplete_arguments" }
        }]
    });

    let err = translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        "claude-3",
        &mut body,
        false,
    )
    .expect_err("unsigned replay marker must not bypass structured-args validation");

    assert!(err.contains("arguments"), "err = {err}");
    assert!(err.contains("JSON"), "err = {err}");
}

#[test]
fn translate_request_responses_marked_invalid_tool_arguments_to_gemini_degrade_to_text() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": [{
            "type": "function_call",
            "call_id": "call_1",
            "name": "lookup_weather",
            "arguments": "{\"city\":\"Tokyo\"",
            "_llmup_non_replayable_tool_call": { "reason": "incomplete_arguments" }
        }]
    });
    super::tools::mark_tool_call_as_non_replayable(&mut body["input"][0]);

    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Google,
        "gemini-2.5-flash",
        &mut body,
        false,
    )
    .expect("marked incomplete responses tool calls should degrade to text");

    let parts = body["contents"][0]["parts"]
        .as_array()
        .expect("gemini parts");
    assert_eq!(
        parts[0]["text"],
        "Tool call `lookup_weather` with partial arguments: {\"city\":\"Tokyo\""
    );
    assert!(parts[0].get("functionCall").is_none(), "body = {body:?}");
}

#[test]
fn translate_request_responses_unmarked_invalid_tool_arguments_to_claude_still_fail_closed() {
    let mut body = json!({
        "model": "gpt-4o",
        "input": [{
            "type": "function_call",
            "call_id": "call_1",
            "name": "lookup_weather",
            "arguments": "{\"city\":\"Tokyo\""
        }]
    });

    let err = translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        "claude-3",
        &mut body,
        false,
    )
    .expect_err("unmarked malformed responses tool calls should fail closed");

    assert!(err.contains("arguments"), "err = {err}");
    assert!(err.contains("JSON"), "err = {err}");
}

#[test]
fn translate_request_claude_to_gemini_keeps_json_tool_results_on_response_result_path() {
    let mut body = json!({
        "model": "claude-3",
        "messages": [
            {
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_1",
                    "name": "lookup_weather",
                    "input": { "city": "Tokyo" }
                }]
            },
            {
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "toolu_1",
                    "content": { "temperature": 22, "unit": "C" }
                }]
            }
        ]
    });

    translate_request(
        UpstreamFormat::Anthropic,
        UpstreamFormat::Google,
        "gemini-1.5",
        &mut body,
        false,
    )
    .unwrap();

    let function_response = &body["contents"][1]["parts"][0]["functionResponse"];
    assert_eq!(function_response["response"]["result"]["temperature"], 22);
    assert!(function_response.get("parts").is_none(), "body = {body:?}");
}

#[test]
fn translate_request_openai_to_gemini_moves_inline_multimodal_tool_results_into_function_response_parts(
) {
    let mut body = json!({
        "model": "gemini-3-pro",
        "messages": [
            {
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": { "name": "inspect_media", "arguments": "{\"city\":\"Tokyo\"}" }
                }]
            },
            {
                "role": "tool",
                "tool_call_id": "call_1",
                "content": [
                    { "type": "text", "text": "Captured artifacts" },
                    {
                        "type": "image_url",
                        "image_url": { "url": "data:image/png;base64,AAAA" }
                    },
                    {
                        "type": "file",
                        "file": {
                            "file_data": "data:application/pdf;base64,CCCC",
                            "filename": "report.pdf"
                        }
                    }
                ]
            }
        ]
    });

    translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        "gemini-3-pro",
        &mut body,
        false,
    )
    .unwrap();

    let function_response = &body["contents"][1]["parts"][0]["functionResponse"];
    let result = function_response["response"]["result"]
        .as_array()
        .expect("result array");
    let parts = function_response["parts"]
        .as_array()
        .expect("functionResponse parts");

    assert_eq!(result[0]["type"], "text");
    assert_eq!(result[0]["text"], "Captured artifacts");
    assert_eq!(result[1]["type"], "image");
    assert_eq!(result[1]["image"]["$ref"], "call_1_part_1.png");
    assert_eq!(result[2]["type"], "file");
    assert_eq!(result[2]["file"]["$ref"], "call_1_part_2.pdf");
    assert_eq!(result[2]["filename"], "report.pdf");

    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0]["inlineData"]["displayName"], "call_1_part_1.png");
    assert_eq!(parts[0]["inlineData"]["mimeType"], "image/png");
    assert_eq!(parts[1]["inlineData"]["displayName"], "call_1_part_2.pdf");
    assert_eq!(parts[1]["inlineData"]["mimeType"], "application/pdf");
}

#[test]
fn translate_request_responses_to_gemini_moves_inline_multimodal_tool_results_into_function_response_parts(
) {
    let mut body = json!({
        "model": "gpt-4o",
        "input": [
            {
                "type": "function_call",
                "call_id": "call_1",
                "name": "inspect_media",
                "arguments": "{\"city\":\"Tokyo\"}"
            },
            {
                "type": "function_call_output",
                "call_id": "call_1",
                "output": [
                    { "type": "input_text", "text": "Captured screenshot" },
                    {
                        "type": "input_image",
                        "image_url": "data:image/png;base64,AAAA"
                    }
                ]
            }
        ]
    });

    translate_request(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Google,
        "gemini-3-pro",
        &mut body,
        false,
    )
    .unwrap();

    let function_response = &body["contents"][1]["parts"][0]["functionResponse"];
    let result = function_response["response"]["result"]
        .as_array()
        .expect("result array");
    let parts = function_response["parts"]
        .as_array()
        .expect("functionResponse parts");

    assert_eq!(result[0]["type"], "text");
    assert_eq!(result[0]["text"], "Captured screenshot");
    assert_eq!(result[1]["type"], "image");
    assert_eq!(result[1]["image"]["$ref"], "call_1_part_1.png");
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0]["inlineData"]["displayName"], "call_1_part_1.png");
}

#[test]
fn translate_request_claude_to_gemini_moves_inline_multimodal_tool_results_into_function_response_parts(
) {
    let mut body = json!({
        "model": "claude-3",
        "messages": [
            {
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_1",
                    "name": "inspect_media",
                    "input": { "city": "Tokyo" }
                }]
            },
            {
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "toolu_1",
                    "content": [
                        { "type": "text", "text": "Captured frame" },
                        {
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": "image/png",
                                "data": "AAAA"
                            }
                        }
                    ]
                }]
            }
        ]
    });

    translate_request(
        UpstreamFormat::Anthropic,
        UpstreamFormat::Google,
        "gemini-3-pro",
        &mut body,
        false,
    )
    .unwrap();

    let function_response = &body["contents"][1]["parts"][0]["functionResponse"];
    let result = function_response["response"]["result"]
        .as_array()
        .expect("result array");
    let parts = function_response["parts"]
        .as_array()
        .expect("functionResponse parts");

    assert_eq!(result[0]["type"], "text");
    assert_eq!(result[0]["text"], "Captured frame");
    assert_eq!(result[1]["type"], "image");
    assert_eq!(result[1]["image"]["$ref"], "toolu_1_part_1.png");
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0]["inlineData"]["displayName"], "toolu_1_part_1.png");
    assert_eq!(parts[0]["inlineData"]["mimeType"], "image/png");
}

#[test]
fn translate_request_openai_to_gemini_rejects_remote_tool_result_media_references() {
    let mut body = json!({
        "model": "gemini-3-pro",
        "messages": [
            {
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": { "name": "inspect_media", "arguments": "{\"city\":\"Tokyo\"}" }
                }]
            },
            {
                "role": "tool",
                "tool_call_id": "call_1",
                "content": [{
                    "type": "image_url",
                    "image_url": { "url": "https://example.com/cat.png" }
                }]
            }
        ]
    });

    let err = translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        "gemini-3-pro",
        &mut body,
        false,
    )
    .expect_err("remote tool result media should fail closed");

    assert!(err.contains("remote"), "err = {err}");
    assert!(err.contains("tool result"), "err = {err}");
}

#[test]
fn translate_request_openai_to_gemini_rejects_file_id_tool_result_references() {
    let mut body = json!({
        "model": "gemini-3-pro",
        "messages": [
            {
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": { "name": "inspect_media", "arguments": "{\"city\":\"Tokyo\"}" }
                }]
            },
            {
                "role": "tool",
                "tool_call_id": "call_1",
                "content": [{
                    "type": "file",
                    "file": { "file_id": "file_123" }
                }]
            }
        ]
    });

    let err = translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        "gemini-3-pro",
        &mut body,
        false,
    )
    .expect_err("file_id tool result references should fail closed");

    assert!(err.contains("file_id"), "err = {err}");
    assert!(err.contains("tool result"), "err = {err}");
}

#[test]
fn translate_request_openai_to_gemini_rejects_unknown_typed_tool_result_media_blocks() {
    let mut body = json!({
        "model": "gemini-3-pro",
        "messages": [
            {
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": { "name": "inspect_media", "arguments": "{\"city\":\"Tokyo\"}" }
                }]
            },
            {
                "role": "tool",
                "tool_call_id": "call_1",
                "content": [{
                    "type": "video",
                    "data": "AAAA"
                }]
            }
        ]
    });

    let err = translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        "gemini-3-pro",
        &mut body,
        false,
    )
    .expect_err("unknown typed tool result media should fail closed");

    assert!(err.contains("video"), "err = {err}");
    assert!(err.contains("tool result"), "err = {err}");
}

#[test]
fn translate_request_openai_to_gemini_rejects_multimodal_function_response_parts_for_gemini_1_5() {
    let mut body = json!({
        "model": "gemini-1.5",
        "messages": [
            {
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": { "name": "inspect_media", "arguments": "{\"city\":\"Tokyo\"}" }
                }]
            },
            {
                "role": "tool",
                "tool_call_id": "call_1",
                "content": [{
                    "type": "image_url",
                    "image_url": { "url": "data:image/png;base64,AAAA" }
                }]
            }
        ]
    });

    let err = translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        "gemini-1.5",
        &mut body,
        false,
    )
    .expect_err("Gemini 1.5 should reject multimodal functionResponse.parts");

    assert!(err.contains("Gemini 3"), "err = {err}");
    assert!(err.contains("functionResponse.parts"), "err = {err}");
}

#[test]
fn translate_request_openai_to_gemini_rejects_unsupported_multimodal_function_response_mime() {
    let mut body = json!({
        "model": "gemini-3-pro",
        "messages": [
            {
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": { "name": "inspect_media", "arguments": "{\"city\":\"Tokyo\"}" }
                }]
            },
            {
                "role": "tool",
                "tool_call_id": "call_1",
                "content": [{
                    "type": "input_audio",
                    "input_audio": { "data": "BBBB", "format": "wav" }
                }]
            }
        ]
    });

    let err = translate_request(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        "gemini-3-pro",
        &mut body,
        false,
    )
    .expect_err("unsupported tool-result MIME should fail closed");

    assert!(err.contains("audio/wav"), "err = {err}");
    assert!(err.contains("functionResponse.parts"), "err = {err}");
}

#[test]
fn translate_response_same_format_passthrough() {
    let body = json!({
        "id": "x",
        "choices": [{ "message": { "role": "assistant", "content": "Hi" }, "finish_reason": "stop" }]
    });
    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .unwrap();
    assert_eq!(out, body);
}

#[test]
fn translate_response_claude_to_openai_has_choices() {
    let body = json!({
        "id": "msg_1",
        "content": [{ "type": "text", "text": "Hello back" }],
        "stop_reason": "end_turn",
        "model": "claude-3",
        "usage": { "input_tokens": 10, "output_tokens": 5 }
    });
    let out = translate_response(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .unwrap();
    assert!(out.get("choices").is_some());
    assert_eq!(out["choices"][0]["message"]["content"], "Hello back");
    assert_eq!(out["usage"]["prompt_tokens"], 10);
}

#[test]
fn translate_response_claude_context_window_stop_maps_to_openai_error_reason() {
    let body = json!({
        "id": "msg_1",
        "content": [{ "type": "text", "text": "" }],
        "stop_reason": "model_context_window_exceeded",
        "model": "claude-3"
    });
    let out = translate_response(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .unwrap();
    assert_eq!(
        out["choices"][0]["finish_reason"],
        "context_length_exceeded"
    );
}

#[test]
fn translate_response_claude_refusal_maps_to_content_filter() {
    let body = json!({
        "id": "msg_1",
        "content": [{ "type": "text", "text": "I can't help with that." }],
        "stop_reason": "refusal",
        "model": "claude-3"
    });
    let out = translate_response(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .unwrap();
    assert_eq!(out["choices"][0]["finish_reason"], "content_filter");
}

#[test]
fn translate_response_claude_refusal_sets_openai_refusal_surface() {
    let body = json!({
        "id": "msg_1",
        "content": [{ "type": "text", "text": "I can't help with that." }],
        "stop_reason": "refusal",
        "model": "claude-3"
    });

    let openai = translate_response(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .unwrap();
    assert_eq!(openai["choices"][0]["finish_reason"], "content_filter");
    assert_eq!(
        openai["choices"][0]["message"]["refusal"],
        "I can't help with that."
    );
    assert!(openai["choices"][0]["message"]["content"].is_null());

    let responses = translate_response(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &body,
    )
    .unwrap();
    assert_eq!(responses["status"], "incomplete");
    let output = responses["output"].as_array().expect("responses output");
    assert_eq!(output[0]["type"], "message");
    assert_eq!(output[0]["content"][0]["type"], "refusal");
    assert_eq!(
        output[0]["content"][0]["refusal"],
        "I can't help with that."
    );
}

#[test]
fn translate_response_responses_to_openai_preserves_text_and_refusal_together() {
    let body = json!({
        "id": "resp_refusal_mix",
        "object": "response",
        "created_at": 1,
        "status": "completed",
        "output": [{
            "type": "message",
            "role": "assistant",
            "content": [
                { "type": "output_text", "text": "Visible answer." },
                { "type": "refusal", "refusal": "But I can't help with the unsafe part." }
            ]
        }]
    });

    let out = translate_response(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .unwrap();

    let message = &out["choices"][0]["message"];
    assert_eq!(message["content"], "Visible answer.");
    assert_eq!(message["refusal"], "But I can't help with the unsafe part.");
}

#[test]
fn translate_response_claude_to_openai_rejects_unsupported_redacted_thinking_block() {
    let body = json!({
        "id": "msg_redacted",
        "content": [{
            "type": "redacted_thinking",
            "data": "opaque"
        }],
        "stop_reason": "end_turn",
        "model": "claude-3"
    });

    let err = translate_response(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .expect_err("unsupported response blocks should fail closed");

    assert!(err.contains("redacted_thinking"), "err = {err}");
}

#[test]
fn translate_response_claude_to_openai_rejects_future_unknown_block() {
    let body = json!({
        "id": "msg_unknown",
        "content": [{
            "type": "mystery_block",
            "payload": "???"
        }],
        "stop_reason": "end_turn",
        "model": "claude-3"
    });

    let err = translate_response(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .expect_err("future unknown anthropic response blocks should fail closed");

    assert!(err.contains("mystery_block"), "err = {err}");
}

#[test]
fn translate_response_claude_server_tool_use_preserved_non_streaming() {
    let body = json!({
        "id": "msg_server_tool",
        "content": [{
            "type": "server_tool_use",
            "id": "toolu_server_1",
            "name": "web_search",
            "input": { "query": "rust" }
        }],
        "stop_reason": "tool_use",
        "model": "claude-3"
    });

    let out = translate_response(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .unwrap();

    let tool_calls = out["choices"][0]["message"]["tool_calls"]
        .as_array()
        .expect("tool calls");
    assert_eq!(
        tool_calls[0]["proxied_tool_kind"],
        "anthropic_server_tool_use"
    );
    assert_eq!(tool_calls[0]["function"]["name"], "web_search");
}

#[test]
fn translate_response_claude_pause_turn_maps_to_pause_turn_finish() {
    let body = json!({
        "id": "msg_1",
        "content": [{ "type": "text", "text": "" }],
        "stop_reason": "pause_turn",
        "model": "claude-3"
    });
    let out = translate_response(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .unwrap();
    assert_eq!(out["choices"][0]["finish_reason"], "pause_turn");
}

#[test]
fn translate_response_openai_to_claude_has_content_array() {
    let body = json!({
        "id": "chatcmpl-1",
        "choices": [{ "message": { "role": "assistant", "content": "Hi" }, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 2 }
    });
    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        &body,
    )
    .unwrap();
    assert!(out.get("content").is_some());
    assert!(out["content"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b.get("type").and_then(Value::as_str) == Some("text")));
}

#[test]
fn translate_response_openai_to_claude_preserves_unprovenanced_reasoning_as_unsigned_thinking() {
    let body = json!({
        "id": "chatcmpl-1",
        "model": "minimax-openai",
        "choices": [{
            "message": {
                "role": "assistant",
                "reasoning_content": "private chain of thought",
                "content": "PONG"
            },
            "finish_reason": "stop"
        }]
    });

    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        &body,
    )
    .unwrap();

    assert_eq!(out["type"], "message");
    assert_eq!(out["stop_reason"], "end_turn");
    let content = out["content"].as_array().expect("anthropic content");
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["type"], "thinking");
    assert_eq!(content[0]["thinking"], "private chain of thought");
    assert!(content[0].get("signature").is_none());
    assert_eq!(content[1]["type"], "text");
    assert_eq!(content[1]["text"], "PONG");
}

#[test]
fn translate_response_openai_to_claude_preserves_unsigned_thinking_and_tool_use_without_provenance()
{
    let body = json!({
        "id": "chatcmpl-1",
        "model": "minimax-openai",
        "choices": [{
            "message": {
                "role": "assistant",
                "reasoning_content": "use python to inspect the traceback",
                "content": "Calling tool.",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "run_python",
                        "arguments": "{\"file\":\"bug.py\"}"
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });

    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        &body,
    )
    .unwrap();

    assert_eq!(out["type"], "message");
    assert_eq!(out["stop_reason"], "tool_use");
    let content = out["content"].as_array().expect("anthropic content");
    assert_eq!(content.len(), 3);
    assert_eq!(content[0]["type"], "thinking");
    assert_eq!(
        content[0]["thinking"],
        "use python to inspect the traceback"
    );
    assert!(content[0].get("signature").is_none());
    assert_eq!(content[1]["type"], "text");
    assert_eq!(content[1]["text"], "Calling tool.");
    assert_eq!(content[2]["type"], "tool_use");
    assert_eq!(content[2]["id"], "call_1");
    assert_eq!(content[2]["name"], "run_python");
    assert_eq!(content[2]["input"]["file"], "bug.py");
}

#[test]
fn translate_response_openai_to_claude_preserves_reasoning_with_anthropic_replay_blocks() {
    let body = json!({
        "id": "chatcmpl-1",
        "model": "anthropic-bridge",
        "choices": [{
            "message": {
                "role": "assistant",
                "reasoning_content": "internal reasoning",
                "_anthropic_reasoning_replay": [{
                    "type": "thinking",
                    "thinking": "internal reasoning",
                    "signature": "sig_123"
                }],
                "content": "Visible answer"
            },
            "finish_reason": "stop"
        }]
    });

    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        &body,
    )
    .unwrap();

    assert_eq!(out["type"], "message");
    let content = out["content"].as_array().expect("anthropic content");
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["type"], "thinking");
    assert_eq!(content[0]["thinking"], "internal reasoning");
    assert_eq!(content[0]["signature"], "sig_123");
    assert_eq!(content[1]["type"], "text");
    assert_eq!(content[1]["text"], "Visible answer");
}

#[test]
fn translate_response_openai_error_finishes_to_claude_stop_reasons() {
    let body = json!({
        "id": "chatcmpl-1",
        "choices": [{ "message": { "role": "assistant", "content": "" }, "finish_reason": "context_length_exceeded" }]
    });
    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        &body,
    )
    .unwrap();
    assert_eq!(out["stop_reason"], "model_context_window_exceeded");
}

#[test]
fn translate_response_openai_error_finish_to_claude_error_body() {
    let body = json!({
        "id": "chatcmpl-1",
        "choices": [{ "message": { "role": "assistant", "content": "" }, "finish_reason": "error" }]
    });
    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        &body,
    )
    .unwrap();
    assert_eq!(out["type"], "error");
    assert_eq!(out["error"]["type"], "api_error");
    assert!(out.get("stop_reason").is_none());
}

#[test]
fn translate_response_openai_tool_error_finish_to_claude_error_body() {
    let body = json!({
        "id": "chatcmpl-1",
        "choices": [{ "message": { "role": "assistant", "content": "" }, "finish_reason": "tool_error" }]
    });
    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        &body,
    )
    .unwrap();
    assert_eq!(out["type"], "error");
    assert_eq!(out["error"]["type"], "invalid_request_error");
    assert!(out.get("stop_reason").is_none());
}

#[test]
fn translate_response_openai_pause_turn_to_claude_stop_reason() {
    let body = json!({
        "id": "chatcmpl-1",
        "choices": [{ "message": { "role": "assistant", "content": "" }, "finish_reason": "pause_turn" }]
    });
    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        &body,
    )
    .unwrap();
    assert_eq!(out["stop_reason"], "pause_turn");
}

#[test]
fn translate_response_openai_to_responses_maps_pause_turn_to_incomplete() {
    let body = json!({
        "id": "chatcmpl_1",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "" },
            "finish_reason": "pause_turn"
        }]
    });
    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        &body,
    )
    .unwrap();
    assert_eq!(out["status"], "incomplete");
    assert_eq!(out["incomplete_details"]["reason"], "pause_turn");
}

#[test]
fn translate_response_responses_to_openai_maps_usage_fields() {
    let body = json!({
        "id": "resp_1",
        "object": "response",
        "output": [{
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": "Hi" }]
        }],
        "usage": {
            "input_tokens": 11,
            "output_tokens": 7,
            "total_tokens": 18,
            "input_tokens_details": { "cached_tokens": 3 },
            "output_tokens_details": { "reasoning_tokens": 2 }
        }
    });
    let out = translate_response(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .unwrap();
    assert_eq!(out["usage"]["prompt_tokens"], 11);
    assert_eq!(out["usage"]["completion_tokens"], 7);
    assert_eq!(out["usage"]["prompt_tokens_details"]["cached_tokens"], 3);
    assert_eq!(
        out["usage"]["completion_tokens_details"]["reasoning_tokens"],
        2
    );
}

#[test]
fn translate_response_responses_to_openai_maps_top_level_output_audio_to_message_audio() {
    let body = json!({
        "id": "resp_audio",
        "object": "response",
        "output": [
            {
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": "Hi" }]
            },
            {
                "type": "output_audio",
                "data": "AAAA",
                "transcript": "hello"
            }
        ]
    });

    let out = translate_response(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .expect("Responses output_audio should map to Chat assistant audio");

    assert_eq!(out["choices"][0]["message"]["content"], "Hi");
    assert_eq!(out["choices"][0]["message"]["audio"]["data"], "AAAA");
    assert_eq!(out["choices"][0]["message"]["audio"]["transcript"], "hello");
}

#[test]
fn translate_response_responses_to_openai_preserves_output_text_logprobs() {
    let body = json!({
        "id": "resp_logprobs",
        "object": "response",
        "output": [{
            "type": "message",
            "role": "assistant",
            "content": [{
                "type": "output_text",
                "text": "Hi",
                "logprobs": [{
                    "token": "Hi",
                    "bytes": [72, 105],
                    "logprob": -0.1,
                    "top_logprobs": [{
                        "token": "Hi",
                        "bytes": [72, 105],
                        "logprob": -0.1
                    }]
                }]
            }]
        }]
    });

    let out = translate_response(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .expect("Responses output_text.logprobs should map to Chat choice.logprobs");

    assert_eq!(out["choices"][0]["message"]["content"], "Hi");
    assert_eq!(out["choices"][0]["logprobs"]["content"][0]["token"], "Hi");
    assert_eq!(
        out["choices"][0]["logprobs"]["content"][0]["top_logprobs"][0]["token"],
        "Hi"
    );
}

#[test]
fn translate_response_responses_to_openai_accepts_legacy_nested_output_audio_shape() {
    let body = json!({
        "id": "resp_audio_legacy",
        "object": "response",
        "output": [{
            "type": "output_audio",
            "audio": {
                "data": "AAAA",
                "format": "wav"
            }
        }]
    });

    let out = translate_response(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .expect("legacy nested output_audio should still parse");

    assert_eq!(out["choices"][0]["message"]["audio"]["data"], "AAAA");
    assert_eq!(out["choices"][0]["message"]["audio"]["format"], "wav");
}

#[test]
fn translate_response_responses_incomplete_to_openai_preserves_terminal_and_usage_details() {
    let body = json!({
        "id": "resp_1",
        "object": "response",
        "created_at": 42,
        "status": "incomplete",
        "incomplete_details": { "reason": "max_output_tokens" },
        "output": [{
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": "Hi" }]
        }],
        "usage": {
            "input_tokens": 11,
            "output_tokens": 7,
            "total_tokens": 18,
            "input_tokens_details": { "cached_tokens": 3 },
            "output_tokens_details": { "reasoning_tokens": 2 }
        }
    });
    let out = translate_response(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .unwrap();
    assert_eq!(out["choices"][0]["message"]["content"], "Hi");
    assert_eq!(out["choices"][0]["finish_reason"], "length");
    assert_eq!(out["usage"]["prompt_tokens_details"]["cached_tokens"], 3);
    assert_eq!(
        out["usage"]["completion_tokens_details"]["reasoning_tokens"],
        2
    );
}

#[test]
fn translate_response_responses_to_openai_preserves_audio_prediction_and_unknown_usage_fields() {
    let body = json!({
        "id": "resp_usage",
        "object": "response",
        "output": [{
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": "Hi" }]
        }],
        "usage": {
            "input_tokens": 11,
            "output_tokens": 7,
            "total_tokens": 18,
            "service_tier": "priority",
            "provider_metric": 99,
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
        }
    });

    let out = translate_response(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .unwrap();

    assert_eq!(out["usage"]["service_tier"], "priority");
    assert_eq!(out["usage"]["provider_metric"], 99);
    assert_eq!(out["usage"]["prompt_tokens_details"]["cached_tokens"], 3);
    assert_eq!(out["usage"]["prompt_tokens_details"]["audio_tokens"], 2);
    assert_eq!(
        out["usage"]["prompt_tokens_details"]["future_prompt_detail"],
        4
    );
    assert_eq!(
        out["usage"]["completion_tokens_details"]["reasoning_tokens"],
        1
    );
    assert_eq!(out["usage"]["completion_tokens_details"]["audio_tokens"], 5);
    assert_eq!(
        out["usage"]["completion_tokens_details"]["accepted_prediction_tokens"],
        6
    );
    assert_eq!(
        out["usage"]["completion_tokens_details"]["rejected_prediction_tokens"],
        2
    );
    assert_eq!(
        out["usage"]["completion_tokens_details"]["future_completion_detail"],
        8
    );
}

#[test]
fn translate_response_responses_failed_to_openai_maps_context_failure() {
    let body = json!({
        "id": "resp_1",
        "object": "response",
        "created_at": 42,
        "status": "failed",
        "error": { "code": "context_length_exceeded" },
        "output": [{
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": "" }]
        }]
    });
    let out = translate_response(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .unwrap();
    assert_eq!(
        out["choices"][0]["finish_reason"],
        "context_length_exceeded"
    );
}

#[test]
fn translate_response_responses_failed_unknown_code_maps_to_error() {
    let body = json!({
        "id": "resp_1",
        "object": "response",
        "created_at": 42,
        "status": "failed",
        "error": { "code": "server_error" },
        "output": [{
            "id": "fc_1",
            "type": "function_call",
            "call_id": "call_1",
            "name": "lookup_weather",
            "arguments": "{\"city\":\"Tokyo\"}"
        }]
    });
    let out = translate_response(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .unwrap();
    assert_eq!(out["choices"][0]["finish_reason"], "error");
    assert!(out["choices"][0]["message"]["tool_calls"].is_array());
}

#[test]
fn translate_response_responses_failed_tool_validation_maps_to_tool_error() {
    let body = json!({
        "id": "resp_1",
        "object": "response",
        "created_at": 42,
        "status": "failed",
        "error": { "code": "tool_validation_error" },
        "output": [{
            "id": "fc_1",
            "type": "function_call",
            "call_id": "call_1",
            "name": "lookup_weather",
            "arguments": "{\"city\":\"Tokyo\"}"
        }]
    });
    let out = translate_response(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .unwrap();
    assert_eq!(out["choices"][0]["finish_reason"], "tool_error");
    assert!(out["choices"][0]["message"]["tool_calls"].is_array());
}

#[test]
fn translate_response_openai_to_responses_maps_usage_fields() {
    let body = json!({
        "id": "chatcmpl_1",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "Hi" },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 11,
            "completion_tokens": 7,
            "total_tokens": 18,
            "prompt_tokens_details": { "cached_tokens": 3 },
            "completion_tokens_details": { "reasoning_tokens": 2 }
        }
    });
    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        &body,
    )
    .unwrap();
    assert_eq!(out["usage"]["input_tokens"], 11);
    assert_eq!(out["usage"]["output_tokens"], 7);
    assert_eq!(out["usage"]["input_tokens_details"]["cached_tokens"], 3);
    assert_eq!(out["usage"]["output_tokens_details"]["reasoning_tokens"], 2);
}

#[test]
fn translate_response_openai_assistant_audio_maps_to_responses_output_audio() {
    let body = json!({
        "id": "chatcmpl_audio",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "Hi",
                "audio": {
                    "data": "AAAA",
                    "transcript": "hello"
                }
            },
            "finish_reason": "stop"
        }]
    });

    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        &body,
    )
    .expect("assistant audio should map to Responses output_audio");

    let output = out["output"].as_array().expect("responses output");
    assert_eq!(output[0]["type"], "message");
    assert_eq!(output[0]["content"][0]["text"], "Hi");
    assert_eq!(output[1]["type"], "output_audio");
    assert_eq!(output[1]["data"], "AAAA");
    assert_eq!(output[1]["transcript"], "hello");
    assert!(output[1].get("audio").is_none(), "output = {output:?}");
}

#[test]
fn translate_response_openai_to_responses_preserves_choice_logprobs_on_output_text() {
    let body = json!({
        "id": "chatcmpl_logprobs",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "Hi"
            },
            "logprobs": {
                "content": [{
                    "token": "Hi",
                    "bytes": [72, 105],
                    "logprob": -0.1,
                    "top_logprobs": [{
                        "token": "Hi",
                        "bytes": [72, 105],
                        "logprob": -0.1
                    }]
                }],
                "refusal": []
            },
            "finish_reason": "stop"
        }]
    });

    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        &body,
    )
    .expect("Chat choice.logprobs should map to Responses output_text.logprobs");

    let content = out["output"][0]["content"]
        .as_array()
        .expect("responses content");
    assert_eq!(content[0]["type"], "output_text");
    assert_eq!(content[0]["text"], "Hi");
    assert_eq!(content[0]["logprobs"][0]["token"], "Hi");
    assert_eq!(content[0]["logprobs"][0]["top_logprobs"][0]["token"], "Hi");
}

#[test]
fn translate_response_openai_assistant_audio_with_id_rejects_for_responses() {
    let body = json!({
        "id": "chatcmpl_audio_id",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "Hi",
                "audio": {
                    "data": "AAAA",
                    "transcript": "hello",
                    "id": "aud_123"
                }
            },
            "finish_reason": "stop"
        }]
    });

    let err = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        &body,
    )
    .expect_err("assistant audio ids should fail closed for Responses");

    assert!(err.contains("audio"), "err = {err}");
    assert!(err.contains("id"), "err = {err}");
}

#[test]
fn translate_response_openai_assistant_audio_with_expires_at_rejects_for_responses() {
    let body = json!({
        "id": "chatcmpl_audio_exp",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "Hi",
                "audio": {
                    "data": "AAAA",
                    "transcript": "hello",
                    "expires_at": 1234567890
                }
            },
            "finish_reason": "stop"
        }]
    });

    let err = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        &body,
    )
    .expect_err("assistant audio expiry should fail closed for Responses");

    assert!(err.contains("audio"), "err = {err}");
    assert!(err.contains("expires_at"), "err = {err}");
}

#[test]
fn translate_response_openai_assistant_audio_still_fails_closed_for_non_responses_targets() {
    let body = json!({
        "id": "chatcmpl_audio",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "Hi",
                "audio": {
                    "data": "AAAA",
                    "format": "wav"
                }
            },
            "finish_reason": "stop"
        }]
    });

    for client_format in [UpstreamFormat::Anthropic, UpstreamFormat::Google] {
        let err = translate_response(UpstreamFormat::OpenAiCompletion, client_format, &body)
            .expect_err("assistant audio should still fail closed for non-Responses sinks");
        assert!(err.contains("audio"), "err = {err}");
        assert!(err.contains("OpenAI"), "err = {err}");
    }
}

#[test]
fn translate_response_openai_to_responses_preserves_audio_prediction_and_unknown_usage_fields() {
    let body = json!({
        "id": "chatcmpl_usage",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "Hi" },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 11,
            "completion_tokens": 7,
            "total_tokens": 18,
            "service_tier": "priority",
            "provider_metric": 99,
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
        }
    });

    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        &body,
    )
    .unwrap();

    assert_eq!(out["usage"]["service_tier"], "priority");
    assert_eq!(out["usage"]["provider_metric"], 99);
    assert_eq!(out["usage"]["input_tokens_details"]["cached_tokens"], 3);
    assert_eq!(out["usage"]["input_tokens_details"]["audio_tokens"], 2);
    assert_eq!(
        out["usage"]["input_tokens_details"]["future_prompt_detail"],
        4
    );
    assert_eq!(out["usage"]["output_tokens_details"]["reasoning_tokens"], 1);
    assert_eq!(out["usage"]["output_tokens_details"]["audio_tokens"], 5);
    assert_eq!(
        out["usage"]["output_tokens_details"]["accepted_prediction_tokens"],
        6
    );
    assert_eq!(
        out["usage"]["output_tokens_details"]["rejected_prediction_tokens"],
        2
    );
    assert_eq!(
        out["usage"]["output_tokens_details"]["future_completion_detail"],
        8
    );
}

#[test]
fn translate_response_openai_annotations_round_trip_to_responses() {
    let body = json!({
        "id": "chatcmpl_annotations",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": [{
                    "type": "text",
                    "text": "Rust",
                    "annotations": [{
                        "type": "url_citation",
                        "url": "https://www.rust-lang.org"
                    }]
                }]
            },
            "finish_reason": "stop"
        }]
    });

    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        &body,
    )
    .unwrap();

    let content = out["output"][0]["content"]
        .as_array()
        .expect("responses content");
    assert_eq!(content[0]["type"], "output_text");
    assert_eq!(
        content[0]["annotations"][0]["url"],
        "https://www.rust-lang.org"
    );
}

#[test]
fn translate_response_responses_to_openai_preserves_interleaved_annotation_order() {
    let body = json!({
        "id": "resp_annotations",
        "object": "response",
        "created_at": 1,
        "status": "completed",
        "output": [{
            "type": "message",
            "role": "assistant",
            "content": [
                {
                    "type": "output_text",
                    "text": "annotated-1",
                    "annotations": [{
                        "type": "url_citation",
                        "url": "https://one.example"
                    }]
                },
                {
                    "type": "output_text",
                    "text": "plain-middle"
                },
                {
                    "type": "output_text",
                    "text": "annotated-2",
                    "annotations": [{
                        "type": "url_citation",
                        "url": "https://two.example"
                    }]
                }
            ]
        }]
    });

    let out = translate_response(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .unwrap();

    let content = out["choices"][0]["message"]["content"]
        .as_array()
        .expect("openai content array");
    assert_eq!(content[0]["text"], "annotated-1");
    assert_eq!(content[1]["text"], "plain-middle");
    assert_eq!(content[2]["text"], "annotated-2");
    assert_eq!(content[0]["annotations"][0]["url"], "https://one.example");
    assert!(
        content[1].get("annotations").is_none(),
        "content = {content:?}"
    );
    assert_eq!(content[2]["annotations"][0]["url"], "https://two.example");
}

#[test]
fn translate_response_responses_to_openai_preserves_custom_and_proxied_tool_kinds() {
    let body = json!({
        "id": "resp_tools",
        "object": "response",
        "created_at": 1,
        "status": "completed",
        "output": [
            {
                "type": "custom_tool_call",
                "call_id": "call_custom",
                "name": "code_exec",
                "input": "print('hi')"
            },
            {
                "type": "function_call",
                "call_id": "call_server",
                "name": "web_search",
                "arguments": "{\"query\":\"rust\"}",
                "proxied_tool_kind": "anthropic_server_tool_use"
            }
        ]
    });

    let out = translate_response(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .unwrap();

    let tool_calls = out["choices"][0]["message"]["tool_calls"]
        .as_array()
        .expect("tool calls");
    assert_eq!(tool_calls[0]["type"], "custom");
    assert_eq!(tool_calls[0]["custom"]["name"], "code_exec");
    assert_eq!(tool_calls[0]["custom"]["input"], "print('hi')");
    assert_eq!(
        tool_calls[1]["proxied_tool_kind"],
        "anthropic_server_tool_use"
    );
}

#[test]
fn translate_response_responses_portable_output_subset_stays_valid() {
    let body = json!({
        "id": "resp_portable",
        "object": "response",
        "created_at": 1,
        "status": "completed",
        "output": [
            {
                "type": "reasoning",
                "summary": [{
                    "type": "summary_text",
                    "text": "Need a tool."
                }]
            },
            {
                "type": "message",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": "Looking it up."
                }]
            },
            {
                "type": "function_call",
                "call_id": "call_1",
                "name": "lookup_weather",
                "arguments": "{\"city\":\"Tokyo\"}"
            },
            {
                "type": "output_audio",
                "data": "AAAA",
                "transcript": "Looking it up."
            }
        ]
    });

    let out = translate_response(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .expect("portable Responses output subset should remain translatable");

    let message = &out["choices"][0]["message"];
    assert_eq!(message["reasoning_content"], "Need a tool.");
    assert_eq!(message["content"], "Looking it up.");
    assert_eq!(
        message["tool_calls"][0]["function"]["name"],
        "lookup_weather"
    );
    assert_eq!(message["audio"]["data"], "AAAA");
    assert_eq!(message["audio"]["transcript"], "Looking it up.");
}

#[test]
fn translate_response_responses_namespaced_tool_calls_fail_closed_for_non_responses_clients() {
    let body = json!({
        "id": "resp_namespaced_tools",
        "object": "response",
        "created_at": 1,
        "status": "completed",
        "output": [
            {
                "type": "custom_tool_call",
                "call_id": "call_custom",
                "name": "lookup_account",
                "namespace": "crm",
                "input": "account_id=123"
            }
        ]
    });

    for client_format in [
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        UpstreamFormat::Google,
    ] {
        let err = translate_response(UpstreamFormat::OpenAiResponses, client_format, &body)
            .expect_err("Responses namespaced tool calls should fail closed");
        assert!(err.contains("namespace"), "err = {err}");
    }
}

#[test]
fn translate_response_responses_nonportable_output_items_fail_closed_for_non_responses_clients() {
    let cases = [
        (
            "computer_call_output",
            json!({
                "type": "computer_call_output",
                "call_id": "call_computer",
                "output": {
                    "type": "computer_screenshot",
                    "image_url": "https://example.com/screen.png"
                }
            }),
        ),
        (
            "compaction",
            json!({
                "type": "compaction",
                "id": "cmp_123",
                "encrypted_content": "opaque"
            }),
        ),
    ];

    for (label, item) in cases {
        let body = json!({
            "id": format!("resp_{label}"),
            "object": "response",
            "created_at": 1,
            "status": "completed",
            "output": [item]
        });

        for client_format in [
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Anthropic,
            UpstreamFormat::Google,
        ] {
            let err = translate_response(UpstreamFormat::OpenAiResponses, client_format, &body)
                .expect_err("nonportable Responses output items should fail closed");
            assert!(err.contains(label), "label = {label}, err = {err}");
        }
    }
}

#[test]
fn translate_response_responses_reasoning_encrypted_content_preserves_carrier_for_anthropic() {
    let encrypted_content = super::openai_responses::encode_anthropic_reasoning_carrier(&[json!({
        "type": "thinking",
        "thinking": "Private reasoning.",
        "signature": "sig_opaque"
    })])
    .expect("carrier should encode");
    let body = json!({
        "id": "resp_reasoning_encrypted",
        "object": "response",
        "created_at": 1,
        "status": "completed",
        "output": [{
            "type": "reasoning",
            "summary": [{
                "type": "summary_text",
                "text": "Private reasoning."
            }],
            "encrypted_content": encrypted_content
        }]
    });

    let out = translate_response(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Anthropic,
        &body,
    )
    .expect("Anthropic clients should preserve reasoning carrier");
    let content = out["content"].as_array().expect("anthropic content");
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "thinking");
    assert_eq!(content[0]["thinking"], "Private reasoning.");
    assert_eq!(content[0]["signature"], "sig_opaque");
}

#[test]
fn translate_response_responses_reasoning_encrypted_content_still_fails_closed_for_non_anthropic_clients(
) {
    let encrypted_content = super::openai_responses::encode_anthropic_reasoning_carrier(&[json!({
        "type": "thinking",
        "thinking": "Private reasoning.",
        "signature": "sig_opaque"
    })])
    .expect("carrier should encode");
    let body = json!({
        "id": "resp_reasoning_encrypted",
        "object": "response",
        "created_at": 1,
        "status": "completed",
        "output": [{
            "type": "reasoning",
            "summary": [{
                "type": "summary_text",
                "text": "Private reasoning."
            }],
            "encrypted_content": encrypted_content
        }]
    });

    for client_format in [UpstreamFormat::OpenAiCompletion, UpstreamFormat::Google] {
        let err = translate_response(UpstreamFormat::OpenAiResponses, client_format, &body)
            .expect_err("encrypted reasoning should still fail closed for non-Anthropic clients");
        assert!(err.contains("encrypted_content"), "err = {err}");
    }
}

#[test]
fn translate_response_responses_tool_call_output_items_fail_closed_for_non_responses_clients() {
    let cases = [
        json!({
            "type": "function_call_output",
            "call_id": "call_fn",
            "output": "done"
        }),
        json!({
            "type": "custom_tool_call_output",
            "call_id": "call_custom",
            "output": "done"
        }),
    ];

    for item in cases {
        let label = item["type"].as_str().expect("item type");
        let body = json!({
            "id": format!("resp_{label}"),
            "object": "response",
            "created_at": 1,
            "status": "completed",
            "output": [item]
        });

        for client_format in [
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::Anthropic,
            UpstreamFormat::Google,
        ] {
            let err = translate_response(UpstreamFormat::OpenAiResponses, client_format, &body)
                .expect_err("Responses tool-call output items should fail closed");
            assert!(err.contains(label), "label = {label}, err = {err}");
        }
    }
}

#[test]
fn translate_response_openai_to_claude_restores_server_tool_use_from_marker() {
    let body = json!({
        "id": "chatcmpl_server_tool",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "tool_calls": [{
                    "id": "server_1",
                    "type": "function",
                    "proxied_tool_kind": "anthropic_server_tool_use",
                    "function": {
                        "name": "web_search",
                        "arguments": "{\"query\":\"rust\"}"
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });

    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        &body,
    )
    .unwrap();

    let content = out["content"].as_array().expect("anthropic content");
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "server_tool_use");
    assert_eq!(content[0]["name"], "web_search");
}

#[test]
fn translate_response_gemini_to_responses_decodes_request_scoped_custom_bridge_without_prefix_leak()
{
    let patch_input = "*** Begin Patch\n*** Add File: hello.txt\n+hello\n*** End Patch\n";
    let body = json!({
        "_llmup_tool_bridge_context": {
            "compatibility_mode": "max_compat",
            "entries": {
                "apply_patch": {
                    "source_kind": "custom_grammar",
                    "transport_kind": "function_object_wrapper",
                    "wrapper_field": "input",
                    "expected_canonical_shape": "single_required_string"
                }
            }
        },
        "responseId": "resp_gemini_custom",
        "modelVersion": "gemini-2.5-flash",
        "candidates": [{
            "content": {
                "role": "model",
                "parts": [{
                    "functionCall": {
                        "id": "call_apply_patch",
                        "name": "apply_patch",
                        "args": { "input": patch_input }
                    }
                }]
            },
            "finishReason": "STOP"
        }]
    });

    let out = translate_response(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiResponses,
        &body,
    )
    .expect("Gemini response should decode bridged custom tool call");

    let output = out["output"].as_array().expect("responses output");
    assert_eq!(
        output[0],
        json!({
            "type": "custom_tool_call",
            "call_id": "call_apply_patch",
            "name": "apply_patch",
            "input": patch_input
        })
    );
}

#[test]
fn translate_response_openai_to_responses_decodes_request_scoped_function_call_to_custom_tool_call_with_exact_input(
) {
    let exact_input = "first line\n{\"patch\":\"*** Begin Patch\"}\n\"quoted\"";
    let body = json!({
        "_llmup_tool_bridge_context": {
            "compatibility_mode": "balanced",
            "entries": {
                "code_exec": {
                    "source_kind": "custom_text",
                    "transport_kind": "function_object_wrapper",
                    "wrapper_field": "input",
                    "expected_canonical_shape": "single_required_string"
                }
            }
        },
        "id": "chatcmpl_tools",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "tool_calls": [
                    {
                        "id": "call_custom",
                        "type": "function",
                        "function": {
                            "name": "code_exec",
                            "arguments": serde_json::to_string(&json!({ "input": exact_input }))
                                .expect("bridge args")
                        }
                    }
                ]
            },
            "finish_reason": "tool_calls"
        }]
    });

    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        &body,
    )
    .unwrap();

    let output = out["output"].as_array().expect("responses output");
    assert_eq!(
        output[0],
        json!({
            "type": "custom_tool_call",
            "call_id": "call_custom",
            "name": "code_exec",
            "input": exact_input
        })
    );
}

#[test]
fn translate_response_openai_to_responses_decodes_request_scoped_custom_bridge_without_prefix_leak()
{
    let patch_input = "*** Begin Patch\n*** Add File: hello.txt\n+hello\n*** End Patch\n";
    let body = json!({
        "_llmup_tool_bridge_context": {
            "compatibility_mode": "balanced",
            "entries": {
                "apply_patch": {
                    "source_kind": "custom_grammar",
                    "transport_kind": "function_object_wrapper",
                    "wrapper_field": "input",
                    "expected_canonical_shape": "single_required_string"
                }
            }
        },
        "id": "chatcmpl_tools",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "tool_calls": [
                    {
                        "id": "call_apply_patch",
                        "type": "function",
                        "function": {
                            "name": "apply_patch",
                            "arguments": serde_json::to_string(&json!({ "input": patch_input }))
                                .expect("bridge args")
                        }
                    }
                ]
            },
            "finish_reason": "tool_calls"
        }]
    });

    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        &body,
    )
    .unwrap();

    let output = out["output"].as_array().expect("responses output");
    assert_eq!(
        output[0],
        json!({
            "type": "custom_tool_call",
            "call_id": "call_apply_patch",
            "name": "apply_patch",
            "input": patch_input
        })
    );
}

#[test]
fn translate_response_openai_to_responses_request_scoped_custom_bridge_falls_back_without_prefix_leak(
) {
    let raw_arguments = serde_json::to_string(&json!({ "input": 5, "extra": true })).expect("json");
    let body = json!({
        "_llmup_tool_bridge_context": {
            "compatibility_mode": "balanced",
            "entries": {
                "apply_patch": {
                    "source_kind": "custom_grammar",
                    "transport_kind": "function_object_wrapper",
                    "wrapper_field": "input",
                    "expected_canonical_shape": "single_required_string"
                }
            }
        },
        "id": "chatcmpl_tools",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_apply_patch",
                    "type": "function",
                    "function": {
                        "name": "apply_patch",
                        "arguments": raw_arguments
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });

    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        &body,
    )
    .expect("noncanonical request-scoped bridge args should fall back to function_call");

    let output = out["output"].as_array().expect("responses output");
    assert_eq!(
        output[0],
        json!({
            "type": "function_call",
            "call_id": "call_apply_patch",
            "name": "apply_patch",
            "arguments": raw_arguments
        })
    );
}

#[test]
fn translate_response_openai_to_responses_does_not_decode_reserved_prefix_function_call_without_request_context(
) {
    let raw_arguments = serde_json::to_string(&json!({ "input": "print('hi')" })).expect("json");
    let body = json!({
        "id": "chatcmpl_tools",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_custom",
                    "type": "function",
                    "function": {
                        "name": "__llmup_custom__code_exec",
                        "arguments": raw_arguments
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });

    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        &body,
    )
    .expect("reserved-prefix names should not decode without request context");

    let output = out["output"].as_array().expect("responses output");
    assert_eq!(
        output[0],
        json!({
            "type": "function_call",
            "call_id": "call_custom",
            "name": "__llmup_custom__code_exec",
            "arguments": raw_arguments
        })
    );
}

#[test]
fn translate_response_openai_to_responses_request_scoped_bridge_falls_back_when_arguments_are_noncanonical(
) {
    let bad_arguments = [
        "not valid json".to_string(),
        serde_json::to_string(&json!({ "output": "missing input" })).expect("json"),
        serde_json::to_string(&json!({ "input": 5 })).expect("json"),
        serde_json::to_string(&json!({ "input": "print('hi')", "extra": true })).expect("json"),
    ];

    for raw_arguments in bad_arguments {
        let body = json!({
            "_llmup_tool_bridge_context": {
                "compatibility_mode": "balanced",
                "entries": {
                    "code_exec": {
                        "source_kind": "custom_text",
                        "transport_kind": "function_object_wrapper",
                        "wrapper_field": "input",
                        "expected_canonical_shape": "single_required_string"
                    }
                }
            },
            "id": "chatcmpl_tools",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_custom",
                        "type": "function",
                        "function": {
                            "name": "code_exec",
                            "arguments": raw_arguments
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });

        let out = translate_response(
            UpstreamFormat::OpenAiCompletion,
            UpstreamFormat::OpenAiResponses,
            &body,
        )
        .expect("noncanonical bridged args should fall back to function_call");

        let output = out["output"].as_array().expect("responses output");
        assert_eq!(
            output[0],
            json!({
                "type": "function_call",
                "call_id": "call_custom",
                "name": "code_exec",
                "arguments": raw_arguments
            })
        );
    }
}

#[test]
fn translate_response_openai_to_responses_keeps_unprefixed_apply_patch_as_function() {
    let raw_arguments =
        serde_json::to_string(&json!({ "patch": "*** Begin Patch\n*** End Patch\n" }))
            .expect("function args");
    let body = json!({
        "id": "chatcmpl_tools",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_apply_patch",
                    "type": "function",
                    "function": {
                        "name": "apply_patch",
                        "arguments": raw_arguments
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });

    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        &body,
    )
    .unwrap();

    let output = out["output"].as_array().expect("responses output");
    assert_eq!(
        output[0],
        json!({
            "type": "function_call",
            "call_id": "call_apply_patch",
            "name": "apply_patch",
            "arguments": raw_arguments
        })
    );
}

#[test]
fn translate_response_openai_to_claude_preserves_text_annotations_as_citations() {
    let body = json!({
        "id": "chatcmpl_citations",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": [{
                    "type": "text",
                    "text": "Fact.",
                    "annotations": [{
                        "type": "url_citation",
                        "url": "https://example.com/fact"
                    }]
                }]
            },
            "finish_reason": "stop"
        }]
    });

    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Anthropic,
        &body,
    )
    .unwrap();

    let content = out["content"].as_array().expect("anthropic content");
    assert_eq!(content[0]["type"], "text");
    assert_eq!(
        content[0]["citations"][0]["url"],
        "https://example.com/fact"
    );
}

#[test]
fn translate_response_claude_usage_preserves_extra_usage_fields() {
    let body = json!({
        "id": "msg_usage",
        "content": [{ "type": "text", "text": "Hi" }],
        "stop_reason": "end_turn",
        "model": "claude-3",
        "usage": {
            "input_tokens": 10,
            "output_tokens": 5,
            "cache_read_input_tokens": 2,
            "cache_creation_input_tokens": 3,
            "service_tier": "priority",
            "output_tokens_details": {
                "reasoning_tokens": 4
            }
        }
    });

    let out = translate_response(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .unwrap();

    assert_eq!(out["usage"]["prompt_tokens"], 15);
    assert_eq!(out["usage"]["completion_tokens"], 5);
    assert_eq!(out["usage"]["cache_read_input_tokens"], 2);
    assert_eq!(out["usage"]["cache_creation_input_tokens"], 3);
    assert_eq!(out["usage"]["service_tier"], "priority");
    assert_eq!(out["usage"]["output_tokens_details"]["reasoning_tokens"], 4);
}

#[test]
fn translate_response_openai_to_responses_preserves_reasoning_output() {
    let body = json!({
        "id": "chatcmpl_1",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "reasoning_content": "thinking",
                "content": "Hi"
            },
            "finish_reason": "stop"
        }]
    });
    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        &body,
    )
    .unwrap();
    assert_eq!(out["output"][0]["type"], "reasoning");
    assert_eq!(out["output"][0]["summary"][0]["text"], "thinking");
    assert_eq!(out["output"][1]["type"], "message");
}

#[test]
fn translate_response_openai_to_responses_maps_length_to_incomplete() {
    let body = json!({
        "id": "chatcmpl_1",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "Hi" },
            "finish_reason": "length"
        }]
    });
    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        &body,
    )
    .unwrap();
    assert_eq!(out["status"], "incomplete");
    assert_eq!(out["incomplete_details"]["reason"], "max_output_tokens");
}

#[test]
fn translate_response_openai_to_responses_maps_content_filter_to_incomplete() {
    let body = json!({
        "id": "chatcmpl_1",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "Hi" },
            "finish_reason": "content_filter"
        }]
    });
    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        &body,
    )
    .unwrap();
    assert_eq!(out["status"], "incomplete");
    assert_eq!(out["incomplete_details"]["reason"], "content_filter");
}

#[test]
fn translate_response_openai_to_responses_maps_context_window_to_failed() {
    let body = json!({
        "id": "chatcmpl_1",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "" },
            "finish_reason": "context_length_exceeded"
        }]
    });
    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        &body,
    )
    .unwrap();
    assert_eq!(out["status"], "failed");
    assert_eq!(out["error"]["code"], "context_length_exceeded");
    assert_eq!(out["incomplete_details"], serde_json::Value::Null);
}

#[test]
fn translate_response_openai_to_gemini_maps_usage_fields() {
    let body = json!({
        "id": "chatcmpl_1",
        "object": "chat.completion",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "Hi" },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 11,
            "completion_tokens": 7,
            "total_tokens": 18,
            "prompt_tokens_details": { "cached_tokens": 3 },
            "completion_tokens_details": { "reasoning_tokens": 2 }
        }
    });
    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        &body,
    )
    .unwrap();
    assert_eq!(out["usageMetadata"]["promptTokenCount"], 11);
    assert_eq!(out["usageMetadata"]["candidatesTokenCount"], 5);
    assert_eq!(out["usageMetadata"]["thoughtsTokenCount"], 2);
    assert_eq!(out["usageMetadata"]["cachedContentTokenCount"], 3);
}

#[test]
fn translate_response_openai_to_gemini_maps_response_logprobs() {
    let body = json!({
        "id": "chatcmpl_logprobs_gemini",
        "object": "chat.completion",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "Hi" },
            "logprobs": {
                "content": [{
                    "token": "Hi",
                    "bytes": [72, 105],
                    "logprob": -0.1,
                    "top_logprobs": [
                        { "token": "Hi", "bytes": [72, 105], "logprob": -0.1 },
                        { "token": "Hey", "bytes": [72, 101, 121], "logprob": -0.4 }
                    ]
                }],
                "refusal": []
            },
            "finish_reason": "stop"
        }]
    });

    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        &body,
    )
    .expect("Chat response logprobs should map to Gemini candidate logprobs");

    assert_eq!(out["candidates"][0]["avgLogprobs"], -0.1);
    assert_eq!(
        out["candidates"][0]["logprobsResult"]["logProbabilitySum"],
        -0.1
    );
    assert_eq!(
        out["candidates"][0]["logprobsResult"]["chosenCandidates"][0]["token"],
        "Hi"
    );
    assert_eq!(
        out["candidates"][0]["logprobsResult"]["topCandidates"][0]["candidates"][0]["token"],
        "Hi"
    );
    assert_eq!(
        out["candidates"][0]["logprobsResult"]["topCandidates"][0]["candidates"][1]["token"],
        "Hey"
    );
}

#[test]
fn translate_response_openai_to_gemini_rejects_nonportable_refusal_logprobs() {
    let body = json!({
        "id": "chatcmpl_refusal_logprobs_gemini",
        "object": "chat.completion",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": null,
                "refusal": "I can't help with that."
            },
            "logprobs": {
                "content": [],
                "refusal": [{
                    "token": "I",
                    "bytes": [73],
                    "logprob": -0.1,
                    "top_logprobs": []
                }]
            },
            "finish_reason": "content_filter"
        }]
    });

    let err = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        &body,
    )
    .expect_err("refusal logprobs should fail closed for Gemini translation");

    assert!(err.contains("refusal"), "err = {err}");
    assert!(err.contains("Gemini"), "err = {err}");
}

#[test]
fn translate_response_responses_to_gemini_maps_output_text_logprobs() {
    let body = json!({
        "id": "resp_logprobs_gemini",
        "object": "response",
        "model": "gpt-4o",
        "output": [{
            "type": "message",
            "role": "assistant",
            "content": [{
                "type": "output_text",
                "text": "Hi",
                "logprobs": [{
                    "token": "Hi",
                    "bytes": [72, 105],
                    "logprob": -0.1,
                    "top_logprobs": [
                        { "token": "Hi", "bytes": [72, 105], "logprob": -0.1 },
                        { "token": "Hey", "bytes": [72, 101, 121], "logprob": -0.4 }
                    ]
                }]
            }]
        }]
    });

    let out = translate_response(
        UpstreamFormat::OpenAiResponses,
        UpstreamFormat::Google,
        &body,
    )
    .expect("Responses output_text.logprobs should map to Gemini candidate logprobs");

    assert_eq!(out["candidates"][0]["avgLogprobs"], -0.1);
    assert_eq!(
        out["candidates"][0]["logprobsResult"]["logProbabilitySum"],
        -0.1
    );
    assert_eq!(
        out["candidates"][0]["logprobsResult"]["chosenCandidates"][0]["token"],
        "Hi"
    );
    assert_eq!(
        out["candidates"][0]["logprobsResult"]["topCandidates"][0]["candidates"][1]["token"],
        "Hey"
    );
}

#[test]
fn translate_response_openai_to_gemini_tool_calls_include_dummy_signature() {
    let body = json!({
        "id": "chatcmpl_gem_fc",
        "object": "chat.completion",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "reasoning_content": "Need tools.",
                "content": "Calling tools.",
                "tool_calls": [
                    {
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "lookup_weather", "arguments": "{\"city\":\"Tokyo\"}" }
                    },
                    {
                        "id": "call_2",
                        "type": "function",
                        "function": { "name": "lookup_time", "arguments": "{\"city\":\"Tokyo\"}" }
                    }
                ]
            },
            "finish_reason": "tool_calls"
        }]
    });
    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        &body,
    )
    .unwrap();
    let parts = out["candidates"][0]["content"]["parts"]
        .as_array()
        .expect("gemini parts");
    let function_parts = parts
        .iter()
        .filter(|part| part.get("functionCall").is_some())
        .collect::<Vec<_>>();
    assert_eq!(
        function_parts[0]["thoughtSignature"],
        "skip_thought_signature_validator"
    );
    assert!(function_parts[1].get("thoughtSignature").is_none());
}

#[test]
fn translate_response_gemini_to_openai_maps_finish_and_reasoning_usage_details() {
    let body = json!({
        "response": {
            "responseId": "gem_resp_1",
            "modelVersion": "gemini-2.5",
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{ "text": "Hi" }]
                },
                "finishReason": "MAX_TOKENS"
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
    let out = translate_response(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .unwrap();
    assert_eq!(out["id"], "gem_resp_1");
    assert_eq!(out["model"], "gemini-2.5");
    assert_eq!(out["choices"][0]["finish_reason"], "length");
    assert_eq!(out["usage"]["prompt_tokens"], 11);
    assert_eq!(out["usage"]["completion_tokens"], 7);
    assert_eq!(out["usage"]["prompt_tokens_details"]["cached_tokens"], 3);
    assert_eq!(
        out["usage"]["completion_tokens_details"]["reasoning_tokens"],
        2
    );
}

#[test]
fn translate_response_gemini_to_openai_preserves_response_logprobs() {
    let body = json!({
        "response": {
            "responseId": "gem_resp_logprobs_chat",
            "modelVersion": "gemini-2.5",
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{ "text": "Hi" }]
                },
                "finishReason": "STOP",
                "avgLogprobs": -0.1,
                "logprobsResult": {
                    "logProbabilitySum": -0.1,
                    "chosenCandidates": [{
                        "token": "Hi",
                        "tokenId": 42,
                        "logProbability": -0.1
                    }],
                    "topCandidates": [{
                        "candidates": [
                            { "token": "Hi", "tokenId": 42, "logProbability": -0.1 },
                            { "token": "Hey", "tokenId": 43, "logProbability": -0.4 }
                        ]
                    }]
                }
            }]
        }
    });

    let out = translate_response(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .expect("Gemini candidate logprobs should map to Chat choice.logprobs");

    assert_eq!(out["choices"][0]["message"]["content"], "Hi");
    assert_eq!(out["choices"][0]["logprobs"]["content"][0]["token"], "Hi");
    assert_eq!(
        out["choices"][0]["logprobs"]["content"][0]["top_logprobs"][1]["token"],
        "Hey"
    );
}

#[test]
fn translate_response_gemini_to_responses_preserves_response_logprobs() {
    let body = json!({
        "response": {
            "responseId": "gem_resp_logprobs_responses",
            "modelVersion": "gemini-2.5",
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{ "text": "Hi" }]
                },
                "finishReason": "STOP",
                "avgLogprobs": -0.1,
                "logprobsResult": {
                    "logProbabilitySum": -0.1,
                    "chosenCandidates": [{
                        "token": "Hi",
                        "tokenId": 42,
                        "logProbability": -0.1
                    }],
                    "topCandidates": [{
                        "candidates": [
                            { "token": "Hi", "tokenId": 42, "logProbability": -0.1 },
                            { "token": "Hey", "tokenId": 43, "logProbability": -0.4 }
                        ]
                    }]
                }
            }]
        }
    });

    let out = translate_response(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiResponses,
        &body,
    )
    .expect("Gemini candidate logprobs should map to Responses output_text.logprobs");

    let content = out["output"][0]["content"]
        .as_array()
        .expect("responses content");
    assert_eq!(content[0]["type"], "output_text");
    assert_eq!(content[0]["text"], "Hi");
    assert_eq!(content[0]["logprobs"][0]["token"], "Hi");
    assert_eq!(content[0]["logprobs"][0]["top_logprobs"][1]["token"], "Hey");
}

#[test]
fn translate_response_gemini_to_non_gemini_rejects_avg_logprobs_without_token_detail() {
    let body = json!({
        "response": {
            "responseId": "gem_resp_avg_only",
            "modelVersion": "gemini-2.5",
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{ "text": "Hi" }]
                },
                "finishReason": "STOP",
                "avgLogprobs": -0.1
            }]
        }
    });

    for client_format in [
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
    ] {
        let err = translate_response(UpstreamFormat::Google, client_format, &body)
            .expect_err("Gemini avgLogprobs without logprobsResult should fail closed");
        assert!(err.contains("avgLogprobs"), "err = {err}");
        assert!(err.contains("logprobsResult"), "err = {err}");
    }
}

#[test]
fn translate_response_gemini_prompt_feedback_without_candidates_is_not_an_error() {
    let body = json!({
        "promptFeedback": {
            "blockReason": "SAFETY"
        },
        "usageMetadata": {
            "promptTokenCount": 3,
            "totalTokenCount": 3
        },
        "modelVersion": "gemini-2.5"
    });
    let out = translate_response(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .expect("translated response");

    assert_eq!(out["object"], "chat.completion");
    assert_eq!(out["choices"][0]["finish_reason"], "content_filter");
    assert_eq!(out["usage"]["prompt_tokens"], 3);
}

#[test]
fn translate_response_gemini_to_openai_rejects_multiple_candidates() {
    let body = json!({
        "response": {
            "responseId": "gem_resp_multi",
            "modelVersion": "gemini-2.5",
            "candidates": [
                {
                    "content": { "role": "model", "parts": [{ "text": "Hi" }] },
                    "finishReason": "STOP"
                },
                {
                    "content": { "role": "model", "parts": [{ "text": "Hello" }] },
                    "finishReason": "STOP"
                }
            ]
        }
    });

    let err = translate_response(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .expect_err("multiple Gemini candidates should fail closed");

    assert!(err.contains("candidates"), "err = {err}");
    assert!(err.contains("single"), "err = {err}");
}

#[test]
fn translate_response_gemini_non_success_finish_reasons_do_not_collapse_to_success() {
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
        let body = json!({
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
        let out = translate_response(
            UpstreamFormat::Google,
            UpstreamFormat::OpenAiCompletion,
            &body,
        )
        .unwrap();
        assert_eq!(
            out["choices"][0]["finish_reason"], expected,
            "reason = {reason}, body = {out:?}"
        );
    }
}

#[test]
fn translate_response_openai_to_responses_maps_error_finish_to_failed() {
    let body = json!({
        "id": "chatcmpl_1",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "" },
            "finish_reason": "error"
        }]
    });
    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        &body,
    )
    .unwrap();
    assert_eq!(out["status"], "failed");
    assert_eq!(out["error"]["code"], "error");
}

#[test]
fn translate_response_openai_to_responses_maps_tool_error_finish_to_failed() {
    let body = json!({
        "id": "chatcmpl_1",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "" },
            "finish_reason": "tool_error"
        }]
    });
    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::OpenAiResponses,
        &body,
    )
    .unwrap();
    assert_eq!(out["status"], "failed");
    assert_eq!(out["error"]["code"], "tool_error");
}

#[test]
fn translate_response_openai_to_gemini_maps_portable_finish_reasons() {
    let body = json!({
        "id": "chatcmpl_1",
        "object": "chat.completion",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "Hi" },
            "finish_reason": "content_filter"
        }]
    });
    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        &body,
    )
    .unwrap();
    assert_eq!(out["responseId"], "chatcmpl_1");
    assert_eq!(out["modelVersion"], "gpt-4o");
    assert_eq!(out["candidates"][0]["finishReason"], "SAFETY");
}

#[test]
fn translate_response_openai_refusal_to_gemini_preserves_text_part_and_safety_finish() {
    let body = json!({
        "id": "chatcmpl_refusal",
        "object": "chat.completion",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": null,
                "refusal": "I can't help with that."
            },
            "finish_reason": "content_filter"
        }]
    });

    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        &body,
    )
    .unwrap();

    let parts = out["candidates"][0]["content"]["parts"]
        .as_array()
        .expect("gemini parts");
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0]["text"], "I can't help with that.");
    assert_eq!(out["candidates"][0]["finishReason"], "SAFETY");
}

#[test]
fn translate_response_openai_text_and_refusal_to_gemini_preserves_both() {
    let body = json!({
        "id": "chatcmpl_text_refusal",
        "object": "chat.completion",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "Visible answer.",
                "refusal": "But I can't help with the unsafe part."
            },
            "finish_reason": "content_filter"
        }]
    });

    let out = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        &body,
    )
    .unwrap();

    let parts = out["candidates"][0]["content"]["parts"]
        .as_array()
        .expect("gemini parts");
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0]["text"], "Visible answer.");
    assert_eq!(parts[1]["text"], "But I can't help with the unsafe part.");
    assert_eq!(out["candidates"][0]["finishReason"], "SAFETY");
}

#[test]
fn translate_response_openai_to_gemini_rejects_multiple_choices() {
    let body = json!({
        "id": "chatcmpl_multi",
        "object": "chat.completion",
        "model": "gpt-4o",
        "choices": [
            {
                "index": 0,
                "message": { "role": "assistant", "content": "Hi" },
                "finish_reason": "stop"
            },
            {
                "index": 1,
                "message": { "role": "assistant", "content": "Hello" },
                "finish_reason": "stop"
            }
        ]
    });

    let err = translate_response(
        UpstreamFormat::OpenAiCompletion,
        UpstreamFormat::Google,
        &body,
    )
    .expect_err("multiple OpenAI choices should fail closed");

    assert!(err.contains("choices"), "err = {err}");
    assert!(err.contains("single"), "err = {err}");
}

#[test]
fn translate_response_gemini_to_openai_rejects_multimodal_assistant_output_parts() {
    let body = json!({
        "response": {
            "responseId": "gem_resp_multimodal",
            "modelVersion": "gemini-2.5",
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [
                        { "text": "Look at this." },
                        { "inlineData": { "mimeType": "image/png", "data": "iVBORw0KGgo" } },
                        { "inlineData": { "mimeType": "audio/wav", "data": "AAAA" } },
                        { "inlineData": { "mimeType": "application/pdf", "data": "JVBERi0x" } },
                        { "fileData": { "mimeType": "application/pdf", "fileUri": "gs://bucket/doc.pdf", "displayName": "doc.pdf" } }
                    ]
                },
                "finishReason": "STOP"
            }]
        }
    });

    let out = translate_response(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .expect_err("Gemini assistant multimodal output should fail closed");

    assert!(out.contains("Gemini"), "err = {out}");
    assert!(out.contains("assistant"), "err = {out}");
}

#[test]
fn translate_response_gemini_to_openai_rejects_unrepresentable_output_part() {
    let body = json!({
        "response": {
            "responseId": "gem_resp_code",
            "modelVersion": "gemini-2.5",
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{
                        "executableCode": {
                            "code": "print('hi')",
                            "language": "PYTHON"
                        }
                    }]
                },
                "finishReason": "STOP"
            }]
        }
    });

    let err = translate_response(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .expect_err("unrepresentable Gemini output parts should fail closed");

    assert!(err.contains("executableCode"), "err = {err}");
    assert!(err.contains("OpenAI"), "err = {err}");
}

#[test]
fn translate_response_gemini_to_openai_allows_text_and_function_call_output() {
    let body = json!({
        "response": {
            "responseId": "gem_resp_text_tool",
            "modelVersion": "gemini-2.5",
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [
                        { "text": "Need a tool." },
                        {
                            "functionCall": {
                                "id": "call_1",
                                "name": "lookup_weather",
                                "args": { "city": "Tokyo" }
                            }
                        }
                    ]
                },
                "finishReason": "STOP"
            }]
        }
    });

    let out = translate_response(
        UpstreamFormat::Google,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .unwrap();

    assert_eq!(out["choices"][0]["message"]["content"], "Need a tool.");
    assert_eq!(
        out["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
        "lookup_weather"
    );
}

#[test]
fn translate_response_claude_thinking_signature_provenance_drops_signature_for_non_anthropic_clients(
) {
    let body = json!({
        "id": "msg_sig",
        "content": [
            {
                "type": "thinking",
                "thinking": "internal reasoning",
                "signature": "sig_123"
            },
            { "type": "text", "text": "Visible answer" }
        ],
        "stop_reason": "end_turn",
        "model": "claude-3"
    });

    let openai = translate_response(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .expect("signed thinking should degrade for OpenAI clients");
    assert_eq!(
        openai["choices"][0]["message"]["reasoning_content"],
        "internal reasoning"
    );
    assert_eq!(openai["choices"][0]["message"]["content"], "Visible answer");
    assert!(openai["choices"][0]["message"]
        .get("_anthropic_reasoning_replay")
        .is_none());

    let gemini = translate_response(UpstreamFormat::Anthropic, UpstreamFormat::Google, &body)
        .expect("signed thinking should degrade for Gemini clients");
    let parts = gemini["candidates"][0]["content"]["parts"]
        .as_array()
        .expect("gemini parts");
    assert_eq!(parts[0]["thought"], true);
    assert_eq!(parts[0]["text"], "internal reasoning");
    assert_eq!(parts[1]["text"], "Visible answer");
    assert!(parts[0].get("thoughtSignature").is_none());
    assert!(parts[0].get("thought_signature").is_none());
}

#[test]
fn translate_response_claude_thinking_signature_provenance_maps_to_responses_carrier() {
    let body = json!({
        "id": "msg_sig",
        "content": [
            {
                "type": "thinking",
                "thinking": "internal reasoning",
                "signature": "sig_123"
            },
            { "type": "text", "text": "Visible answer" }
        ],
        "stop_reason": "end_turn",
        "model": "claude-3"
    });

    let out = translate_response(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &body,
    )
    .expect("Anthropic signed thinking should translate to Responses");

    let output = out["output"].as_array().expect("responses output");
    assert_eq!(output[0]["type"], "reasoning");
    assert_eq!(output[0]["summary"][0]["text"], "internal reasoning");
    assert!(
        output[0]["encrypted_content"].is_string(),
        "output = {output:?}"
    );
    assert_ne!(
        output[0]["encrypted_content"].as_str().unwrap_or(""),
        "",
        "output = {output:?}"
    );
    assert_eq!(output[1]["type"], "message");
    assert_eq!(output[1]["content"][0]["text"], "Visible answer");
}

#[test]
fn translate_response_claude_request_scoped_tool_use_restores_responses_custom_tool_call() {
    let exact_input = "first line\n{\"patch\":\"*** Begin Patch\"}\n\"quoted\"";
    let body = json!({
        "_llmup_tool_bridge_context": {
            "compatibility_mode": "balanced",
            "entries": {
                "code_exec": {
                    "source_kind": "custom_text",
                    "transport_kind": "function_object_wrapper",
                    "wrapper_field": "input",
                    "expected_canonical_shape": "single_required_string"
                }
            }
        },
        "id": "msg_custom_tool",
        "content": [{
            "type": "tool_use",
            "id": "toolu_custom",
            "name": "code_exec",
            "input": { "input": exact_input }
        }],
        "stop_reason": "tool_use",
        "model": "claude-3"
    });

    let out = translate_response(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &body,
    )
    .expect("Anthropic bridged tool_use should restore Responses custom tool calls");

    let output = out["output"].as_array().expect("responses output");
    assert_eq!(
        output[0],
        json!({
            "type": "custom_tool_call",
            "call_id": "toolu_custom",
            "name": "code_exec",
            "input": exact_input
        })
    );
}

#[test]
fn translate_response_claude_omitted_thinking_drops_reasoning_for_non_anthropic_clients() {
    let body = json!({
        "id": "msg_omitted",
        "content": [
            {
                "type": "thinking",
                "thinking": { "display": "omitted" },
                "signature": "sig_123"
            },
            { "type": "text", "text": "Visible answer" }
        ],
        "stop_reason": "end_turn",
        "model": "claude-3"
    });

    let openai = translate_response(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .expect("omitted thinking should be dropped for OpenAI clients");
    assert_eq!(openai["choices"][0]["message"]["content"], "Visible answer");
    assert!(openai["choices"][0]["message"]
        .get("reasoning_content")
        .is_none());

    let gemini = translate_response(UpstreamFormat::Anthropic, UpstreamFormat::Google, &body)
        .expect("omitted thinking should be dropped for Gemini clients");
    let parts = gemini["candidates"][0]["content"]["parts"]
        .as_array()
        .expect("gemini parts");
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0]["text"], "Visible answer");
    assert!(parts[0].get("thought").is_none());
}

#[test]
fn translate_response_claude_omitted_thinking_maps_to_responses_carrier() {
    let body = json!({
        "id": "msg_omitted",
        "content": [
            {
                "type": "thinking",
                "thinking": { "display": "omitted" },
                "signature": "sig_123"
            },
            { "type": "text", "text": "Visible answer" }
        ],
        "stop_reason": "end_turn",
        "model": "claude-3"
    });

    let out = translate_response(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiResponses,
        &body,
    )
    .expect("Anthropic omitted thinking should translate to Responses");

    let output = out["output"].as_array().expect("responses output");
    assert_eq!(output[0]["type"], "reasoning");
    assert_eq!(output[0]["summary"], json!([]));
    assert!(
        output[0]["encrypted_content"].is_string(),
        "output = {output:?}"
    );
    assert_ne!(
        output[0]["encrypted_content"].as_str().unwrap_or(""),
        "",
        "output = {output:?}"
    );
    assert_eq!(output[1]["type"], "message");
    assert_eq!(output[1]["content"][0]["text"], "Visible answer");
}

#[test]
fn translate_response_claude_plain_thinking_without_provenance_still_translates() {
    let body = json!({
        "id": "msg_plain_thinking",
        "content": [
            { "type": "thinking", "thinking": "think" },
            { "type": "text", "text": "Hi" }
        ],
        "stop_reason": "end_turn",
        "model": "claude-3"
    });

    let out = translate_response(
        UpstreamFormat::Anthropic,
        UpstreamFormat::OpenAiCompletion,
        &body,
    )
    .unwrap();

    assert_eq!(out["choices"][0]["message"]["reasoning_content"], "think");
    assert_eq!(out["choices"][0]["message"]["content"], "Hi");
}
