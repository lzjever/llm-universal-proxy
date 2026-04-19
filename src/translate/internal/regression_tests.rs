use super::{
    can_attach_cache_control_to_content_block, convert_claude_message_to_openai,
    openai_message_to_claude_blocks, openai_to_claude,
};
use serde_json::json;

#[test]
fn assistant_string_content_preserves_tool_calls_for_claude() {
    let msg = json!({
        "role": "assistant",
        "content": "Let me check that.",
        "tool_calls": [
            {
                "id": "call_123",
                "type": "function",
                "function": {
                    "name": "exec_command",
                    "arguments": "{\"cmd\":\"pwd\"}"
                }
            }
        ]
    });

    let blocks = openai_message_to_claude_blocks(&msg)
        .expect("translate blocks")
        .expect("assistant blocks");
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0]["type"], "text");
    assert_eq!(blocks[1]["type"], "tool_use");
    assert_eq!(blocks[1]["id"], "call_123");
    assert_eq!(blocks[1]["name"], "exec_command");
}

#[test]
fn assistant_reasoning_content_without_provenance_is_preserved_as_unsigned_thinking_for_claude_blocks(
) {
    let msg = json!({
        "role": "assistant",
        "reasoning_content": "I should call a tool.",
        "content": "Let me check that.",
        "tool_calls": [
            {
                "id": "call_123",
                "type": "function",
                "function": {
                    "name": "exec_command",
                    "arguments": "{\"cmd\":\"pwd\"}"
                }
            }
        ]
    });

    let blocks = openai_message_to_claude_blocks(&msg)
        .expect("reasoning without provenance should still translate")
        .expect("assistant blocks");
    assert_eq!(blocks[0]["type"], "thinking");
    assert_eq!(blocks[0]["thinking"], "I should call a tool.");
    assert!(blocks[0].get("signature").is_none());
    assert_eq!(blocks[1]["type"], "text");
    assert_eq!(blocks[1]["text"], "Let me check that.");
    assert_eq!(blocks[2]["type"], "tool_use");
    assert_eq!(blocks[2]["id"], "call_123");
}

#[test]
fn claude_server_tool_use_is_preserved_as_marked_openai_tool_call() {
    let message = json!({
        "role": "assistant",
        "content": [{
            "type": "server_tool_use",
            "id": "toolu_server_1",
            "name": "web_search",
            "input": { "query": "rust" }
        }]
    });

    let translated = convert_claude_message_to_openai(&message)
        .expect("translated message")
        .expect("openai messages");
    assert_eq!(translated.len(), 1);
    let tool_calls = translated[0]["tool_calls"].as_array().expect("tool calls");
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0]["function"]["name"], "web_search");
    assert_eq!(
        tool_calls[0]["proxied_tool_kind"],
        "anthropic_server_tool_use"
    );
}

#[test]
fn marked_openai_server_tool_call_restores_server_tool_use_block() {
    let message = json!({
        "role": "assistant",
        "tool_calls": [{
            "id": "toolu_server_1",
            "type": "function",
            "proxied_tool_kind": "anthropic_server_tool_use",
            "function": {
                "name": "web_search",
                "arguments": "{\"query\":\"rust\"}"
            }
        }]
    });

    let blocks = openai_message_to_claude_blocks(&message)
        .expect("translate blocks")
        .expect("assistant blocks");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0]["type"], "server_tool_use");
    assert_eq!(blocks[0]["name"], "web_search");
}

#[test]
fn openai_to_claude_merges_tool_results_into_single_user_message() {
    let mut body = json!({
        "model": "codex-anthropic",
        "messages": [
            {
                "role": "assistant",
                "content": "I'll run the commands.",
                "tool_calls": [
                    {
                        "id": "call_a",
                        "type": "function",
                        "function": { "name": "cmd_a", "arguments": "{}" }
                    },
                    {
                        "id": "call_b",
                        "type": "function",
                        "function": { "name": "cmd_b", "arguments": "{}" }
                    }
                ]
            },
            {
                "role": "tool",
                "tool_call_id": "call_a",
                "content": "result-a"
            },
            {
                "role": "tool",
                "tool_call_id": "call_b",
                "content": "result-b"
            }
        ]
    });

    openai_to_claude(&mut body).expect("translate to claude");
    let messages = body["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], "assistant");
    assert_eq!(messages[1]["role"], "user");
    let content = messages[1]["content"].as_array().expect("user content");
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["type"], "tool_result");
    assert_eq!(content[0]["tool_use_id"], "call_a");
    assert_eq!(content[1]["type"], "tool_result");
    assert_eq!(content[1]["tool_use_id"], "call_b");
}

#[test]
fn openai_to_claude_puts_user_text_after_tool_results() {
    let mut body = json!({
        "model": "codex-anthropic",
        "messages": [
            {
                "role": "assistant",
                "tool_calls": [
                    {
                        "id": "call_a",
                        "type": "function",
                        "function": { "name": "cmd_a", "arguments": "{}" }
                    }
                ]
            },
            {
                "role": "tool",
                "tool_call_id": "call_a",
                "content": "result-a"
            },
            {
                "role": "user",
                "content": "continue"
            }
        ]
    });

    openai_to_claude(&mut body).expect("translate to claude");
    let messages = body["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 2);
    let user_content = messages[1]["content"].as_array().expect("user content");
    assert_eq!(user_content[0]["type"], "tool_result");
    assert_eq!(user_content[1]["type"], "text");
    assert_eq!(user_content[1]["text"], "continue");
}

#[test]
fn assistant_tool_use_block_does_not_get_cache_control() {
    let mut body = json!({
        "model": "codex-anthropic",
        "messages": [
            {
                "role": "assistant",
                "content": "Let me check.",
                "tool_calls": [
                    {
                        "id": "call_a",
                        "type": "function",
                        "function": { "name": "cmd_a", "arguments": "{}" }
                    }
                ]
            }
        ]
    });

    openai_to_claude(&mut body).expect("translate to claude");
    let messages = body["messages"].as_array().expect("messages array");
    let assistant_content = messages[0]["content"]
        .as_array()
        .expect("assistant content");
    assert_eq!(assistant_content[1]["type"], "tool_use");
    assert!(assistant_content[1].get("cache_control").is_none());
    assert!(can_attach_cache_control_to_content_block(
        &assistant_content[0]
    ));
    assert!(!can_attach_cache_control_to_content_block(
        &assistant_content[1]
    ));
}
