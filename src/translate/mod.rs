//! Request/response translation between formats (pivot: OpenAI Chat Completions).
//!
//! The translate tree is split into facade modules so the public crate::translate surface stays
//! stable while implementation details remain organized by concern.

mod assessment;
mod internal;
mod request;
mod response;
mod shared;

pub use request::translate_request;
pub use response::translate_response;

pub(crate) use assessment::{assess_request_translation_with_surface, TranslationDecision};
pub(crate) use request::{translate_request_with_policy, RequestTranslationPolicy};
pub(crate) use response::{
    classify_openai_finish_for_anthropic, classify_portable_non_success_terminal,
    responses_failed_code_to_openai_finish, translate_response_with_context, AnthropicTerminal,
    ResponseTranslationContext,
};
pub(crate) use shared::{
    anthropic_tool_use_type_for_openai_tool_call, validate_public_selector_visible_identity,
    validate_public_tool_name_not_reserved, validate_responses_public_output_tool_identity,
    validate_responses_public_request_object_tool_identity,
    validate_responses_public_response_object_tool_identity,
    validate_responses_public_stream_event_tool_identity,
    validate_responses_public_tool_call_item_identity,
};
