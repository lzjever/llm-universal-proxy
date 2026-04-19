//! SSE streaming: passthrough when formats match, otherwise transform chunks (upstream -> openai -> client).
//!
//! Reference: 9router open-sse/handlers/chatCore/streamingHandler.js, utils/stream.js.

use std::pin::Pin;
use std::task::{Context, Poll};

use futures_util::Stream;
use serde_json::Value;

use crate::formats::UpstreamFormat;
use crate::translate::{
    anthropic_tool_use_type_for_openai_tool_call, classify_openai_finish_for_anthropic,
    classify_portable_non_success_terminal, custom_tools_not_portable_message,
    gemini_finish_reason_to_openai, responses_failed_code_to_openai_finish, AnthropicTerminal,
};

mod anthropic_source;
mod gemini_source;
mod openai_sink;
mod responses_source;
mod state;
mod stream;
mod wire;

pub use anthropic_source::claude_event_to_openai_chunks;
pub use gemini_source::gemini_event_to_openai_chunks;
pub use responses_source::responses_event_to_openai_chunks;
pub use state::{ClaudeToolUseState, StreamFatalRejection, StreamState, ToolCallState};
pub use stream::{
    needs_stream_translation, openai_event_as_chunk, translate_response_chunk, translate_sse_event,
    TranslateSseStream,
};
pub use wire::{format_sse_data, format_sse_event, take_one_sse_event};

#[cfg(test)]
mod tests;
