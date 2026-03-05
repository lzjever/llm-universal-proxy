//! LLM Universal Proxy — single binary, single URL, 4 request/response formats.
//!
//! See README and docs/DESIGN.md.

#[tokio::main]
async fn main() {
    if let Err(e) = llm_universal_proxy::run().await {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}
