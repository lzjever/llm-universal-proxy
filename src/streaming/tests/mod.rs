use super::gemini_source::*;
use super::openai_sink::*;
use super::state::*;
use super::*;
use crate::formats::UpstreamFormat;
use crate::translate::translate_response;

fn parse_sse_json(bytes: &[u8]) -> Value {
    let mut buf = bytes.to_vec();
    take_one_sse_event(&mut buf).expect("parse sse event")
}

mod anthropic_sink;
mod anthropic_source;
mod gemini_sink;
mod gemini_source;
mod responses_sink;
mod responses_source;
mod stream;
mod wire;
