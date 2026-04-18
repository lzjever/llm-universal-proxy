use super::*;

pub fn take_one_sse_event(buffer: &mut Vec<u8>) -> Option<Value> {
    // Try CRLF first (\r\n\r\n), then LF (\n\n)
    let pos = buffer
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| p + 2) // point at the second \r\n so drain removes all 4 bytes
        .or_else(|| buffer.windows(2).position(|w| w == b"\n\n"))?;
    let event_bytes = buffer.drain(..=pos + 1).collect::<Vec<_>>();
    let event_str = String::from_utf8_lossy(&event_bytes);
    for line in event_str.lines() {
        let line = line.trim();
        if line.starts_with("data: ") {
            let data = line.strip_prefix("data: ").unwrap_or("").trim();
            if data == "[DONE]" || data.is_empty() {
                return Some(serde_json::json!({ "_done": true }));
            }
            return serde_json::from_str(data).ok();
        }
    }
    None
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
