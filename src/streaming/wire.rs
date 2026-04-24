use super::*;

fn parse_sse_event_json(event_bytes: &[u8]) -> Option<Value> {
    let event_str = String::from_utf8_lossy(event_bytes);
    let mut data_lines = Vec::new();
    for raw_line in event_str.lines() {
        if let Some(data) = raw_line.strip_prefix("data:") {
            data_lines.push(data.strip_prefix(' ').unwrap_or(data));
        }
    }

    if data_lines.is_empty() {
        return None;
    }

    let data = data_lines.join("\n");
    if data == "[DONE]" {
        return Some(serde_json::json!({ "_done": true }));
    }
    if data.trim().is_empty() {
        return None;
    }
    serde_json::from_str(&data).ok()
}

pub(super) fn take_one_sse_frame(buffer: &mut Vec<u8>) -> Option<(Vec<u8>, Option<Value>)> {
    let end = buffer
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| p + 4)
        .or_else(|| buffer.windows(2).position(|w| w == b"\n\n").map(|p| p + 2))?;
    let event_bytes = buffer.drain(..end).collect::<Vec<_>>();
    let event = parse_sse_event_json(&event_bytes);
    Some((event_bytes, event))
}

pub(super) fn sse_frame_event_type(event_bytes: &[u8]) -> Option<String> {
    let event_str = String::from_utf8_lossy(event_bytes);
    for raw_line in event_str.lines() {
        if let Some(event_type) = raw_line.strip_prefix("event:") {
            let event_type = event_type.strip_prefix(' ').unwrap_or(event_type).trim();
            if !event_type.is_empty() {
                return Some(event_type.to_string());
            }
        }
    }
    None
}

pub fn take_one_sse_event(buffer: &mut Vec<u8>) -> Option<Value> {
    loop {
        let (_event_bytes, event) = take_one_sse_frame(buffer)?;
        if let Some(event) = event {
            return Some(event);
        }
    }
}

/// Format one JSON value as SSE "data: {json}\n\n".
pub fn format_sse_data(value: &Value) -> Vec<u8> {
    let s = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string());
    let mut out = b"data: ".to_vec();
    out.extend_from_slice(s.as_bytes());
    out.extend_from_slice(b"\n\n");
    out
}

/// Format SSE with event type line: "event: {ty}\ndata: {json}\n\n".
pub fn format_sse_event(event_type: &str, value: &Value) -> Vec<u8> {
    let s = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string());
    let mut out = format!("event: {event_type}\n").into_bytes();
    out.extend_from_slice(b"data: ");
    out.extend_from_slice(s.as_bytes());
    out.extend_from_slice(b"\n\n");
    out
}
