//! Library for the LLM Universal Proxy.
//!
//! Exposes format detection, request/response translation, and HTTP server.

pub mod config;
pub mod dashboard;
pub(crate) mod dashboard_logs;
pub mod debug_trace;
pub mod detect;
pub mod discovery;
pub(crate) mod downstream;
pub mod formats;
pub mod hooks;
pub(crate) mod internal_artifacts;
pub mod server;
pub mod streaming;
pub mod telemetry;
pub mod translate;
pub mod upstream;

pub use config::Config;
pub use server::{
    run_with_config, run_with_config_and_dashboard, run_with_config_path,
    run_with_config_path_and_dashboard,
};
