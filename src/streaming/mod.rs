//! SSE streaming: passthrough when formats match, otherwise transform chunks (upstream -> openai -> client).
//!
//! Reference: 9router open-sse/handlers/chatCore/streamingHandler.js, utils/stream.js.

use std::pin::Pin;
use std::task::{Context, Poll};

use futures_util::Stream;
use serde_json::Value;

use crate::formats::UpstreamFormat;
use crate::internal_artifacts::{
    contains_internal_artifact_text, contains_internal_context_field,
    sanitize_public_error_message, INTERNAL_ARTIFACT_ERROR_MESSAGE,
};
use crate::translate::{
    anthropic_tool_use_type_for_openai_tool_call, classify_openai_finish_for_anthropic,
    classify_portable_non_success_terminal, responses_failed_code_to_openai_finish,
    validate_public_selector_visible_identity, validate_public_tool_name_not_reserved,
    validate_responses_public_output_tool_identity,
    validate_responses_public_response_object_tool_identity,
    validate_responses_public_stream_event_tool_identity,
    validate_responses_public_tool_call_item_identity, AnthropicTerminal,
};

mod anthropic_source;
mod openai_sink;
mod responses_source;
mod state;
mod stream;
mod wire;

pub use anthropic_source::claude_event_to_openai_chunks;
pub use responses_source::responses_event_to_openai_chunks;
pub use state::{ClaudeToolUseState, StreamFatalRejection, StreamState, ToolCallState};
pub use stream::{
    needs_stream_translation, openai_event_as_chunk, translate_response_chunk, translate_sse_event,
    GuardedSseStream, TranslateSseStream,
};
pub use wire::{format_sse_data, format_sse_event, take_one_sse_event};

#[cfg(test)]
mod tests;
