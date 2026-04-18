use super::*;

pub fn take_one_sse_event(buffer: &mut Vec<u8>) -> Option<Value> {
    loop {
        // Try CRLF first (\r\n\r\n), then LF (\n\n)
        let pos = buffer
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|p| p + 2)
            .or_else(|| buffer.windows(2).position(|w| w == b"\n\n"))?;
        let event_bytes = buffer.drain(..=pos + 1).collect::<Vec<_>>();
        let event_str = String::from_utf8_lossy(&event_bytes);
        let mut data_lines = Vec::new();
        for raw_line in event_str.lines() {
            if let Some(data) = raw_line.strip_prefix("data:") {
                data_lines.push(data.strip_prefix(' ').unwrap_or(data));
            }
        }

        if data_lines.is_empty() {
            continue;
        }

        let data = data_lines.join("\n");
        if data == "[DONE]" {
            return Some(serde_json::json!({ "_done": true }));
        }
        if data.trim().is_empty() {
            continue;
        }
        return serde_json::from_str(&data).ok();
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
