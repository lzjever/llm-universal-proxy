//! Request-side translation facade.
//!
//! Provider-specific request helpers now live under `translate/internal/*`; this module keeps the
//! stable outward seam without pretending there are provider implementations here yet.

pub use super::internal::translate_request;
