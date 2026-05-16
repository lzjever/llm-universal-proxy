//! Response-side translation facade.
//!
//! Provider-specific response helpers live in `translate/internal/response_*`; this module keeps
//! the stable outward seam without a layer of empty leaf modules.

pub(crate) use super::internal::classify_portable_non_success_terminal;
pub(crate) use super::internal::response_protocols::{
    classify_openai_finish_for_anthropic, responses_failed_code_to_openai_finish, AnthropicTerminal,
};
pub use super::internal::translate_response;
pub(crate) use super::internal::{translate_response_with_context, ResponseTranslationContext};
