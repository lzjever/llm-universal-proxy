//! Library for the LLM Universal Proxy.
//!
//! Exposes format detection, request/response translation, and HTTP server.

pub mod config;
pub mod detect;
pub mod discovery;
pub mod formats;
pub mod server;
pub mod streaming;
pub mod translate;
pub mod upstream;

pub use config::Config;
pub use server::{run_with_config, run_with_config_path};
