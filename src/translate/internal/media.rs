use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use serde_json::Value;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenAiFileMimeSource {
    Explicit,
    FileData,
    Filename,
}

struct OpenAiFileMimeCandidate {
    source: OpenAiFileMimeSource,
    source_label: &'static str,
    mime_type: String,
}

pub(super) enum OpenAiFileDataReference<'a> {
    InlineData {
        mime_type: String,
        data: &'a str,
    },
    HttpRemoteUrl {
        mime_type: String,
        url: &'a str,
    },
    ProviderOrLocalUri {
        mime_type: String,
        uri: &'a str,
    },
    BareBase64 {
        mime_type: Option<String>,
        data: &'a str,
    },
}

pub(super) enum MediaSourceReference<'a> {
    MimeDataUri { mime_type: &'a str, data: &'a str },
    HttpRemoteUrl { url: &'a str },
    ProviderOrLocalUri { uri: &'a str },
    BareBase64 { data: &'a str },
    Unsupported { value: &'a str },
}

pub(super) enum OpenAiFileSource<'a> {
    ProviderFileId {
        file_id: Option<&'a str>,
    },
    Payload {
        payload: &'a str,
        field: &'static str,
    },
    ConflictingPayloads,
    InvalidPayload {
        field: &'static str,
    },
    Missing,
}

fn canonical_base64_payload(value: &str, allow_empty: bool) -> bool {
    if value.is_empty() {
        return allow_empty;
    }
    if value
        .chars()
        .any(|ch| ch.is_whitespace() || ch.is_control())
    {
        return false;
    }
    let Ok(decoded) = BASE64_STANDARD.decode(value) else {
        return false;
    };
    BASE64_STANDARD.encode(decoded) == value
}

pub(super) fn validate_inline_base64_payload(value: &str) -> Option<&str> {
    canonical_base64_payload(value, false).then_some(value)
}

pub(super) fn base64_data_uri_parts(value: &str) -> Option<(&str, &str)> {
    if !value.get(..5)?.eq_ignore_ascii_case("data:") {
        return None;
    }
    let rest = value.get(5..)?;
    let (metadata, data) = rest.split_once(',')?;
    let metadata = clean_uri_like_source_reference(metadata)?;
    validate_inline_base64_payload(data)?;
    let mut metadata_parts = metadata.split(';');
    let mime_type = metadata_parts.next()?;
    if mime_type.is_empty() || !metadata_parts.any(|part| part.eq_ignore_ascii_case("base64")) {
        return None;
    }
    Some((mime_type, data))
}

pub(super) fn normalized_mime_type(value: &str) -> Option<String> {
    let mime_type = value
        .split_once(';')
        .map_or(value, |(mime_type, _)| mime_type)
        .trim()
        .to_ascii_lowercase();
    (!mime_type.is_empty()).then_some(mime_type)
}

pub(super) fn mime_type_from_filename(filename: &str) -> Option<&'static str> {
    let extension = filename.rsplit('.').next()?.trim().to_ascii_lowercase();
    match extension.as_str() {
        "pdf" => Some("application/pdf"),
        "json" => Some("application/json"),
        "txt" => Some("text/plain"),
        "csv" => Some("text/csv"),
        "md" => Some("text/markdown"),
        "html" | "htm" => Some("text/html"),
        "xml" => Some("application/xml"),
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "tif" | "tiff" => Some("image/tiff"),
        "heic" => Some("image/heic"),
        "heif" => Some("image/heif"),
        "svg" => Some("image/svg+xml"),
        "wav" => Some("audio/wav"),
        "mp3" => Some("audio/mpeg"),
        "m4a" => Some("audio/mp4"),
        "aac" => Some("audio/aac"),
        "ogg" | "oga" => Some("audio/ogg"),
        "flac" => Some("audio/flac"),
        "opus" => Some("audio/opus"),
        "mp4" => Some("video/mp4"),
        "m4v" => Some("video/x-m4v"),
        "mov" => Some("video/quicktime"),
        "webm" => Some("video/webm"),
        "mpeg" | "mpg" => Some("video/mpeg"),
        "avi" => Some("video/x-msvideo"),
        "mkv" => Some("video/x-matroska"),
        _ => None,
    }
}

fn uri_scheme(value: &str) -> Option<&str> {
    let (scheme, _) = value.split_once(':')?;
    let mut chars = scheme.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    chars
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))
        .then_some(scheme)
}

fn forbidden_uri_format_char(ch: char) -> bool {
    matches!(ch, '\u{200B}')
}

fn has_raw_forbidden_uri_char(value: &str) -> bool {
    value
        .chars()
        .any(|ch| ch.is_whitespace() || ch.is_control() || forbidden_uri_format_char(ch))
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn has_percent_encoded_c0_or_del(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index + 2 < bytes.len() {
        if bytes[index] == b'%' {
            if let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                let decoded = (high << 4) | low;
                if decoded <= 0x1f || decoded == 0x7f {
                    return true;
                }
            }
        }
        index += 1;
    }
    false
}

pub(super) fn clean_uri_like_source_reference(value: &str) -> Option<&str> {
    if value.is_empty() {
        return None;
    }
    if value
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
    {
        return None;
    }
    if has_raw_forbidden_uri_char(value) || has_percent_encoded_c0_or_del(value) {
        return None;
    }
    Some(value)
}

pub(super) fn http_or_https_remote_url(value: &str) -> Option<&str> {
    let value = clean_uri_like_source_reference(value)?;
    let parsed = Url::parse(value).ok()?;
    matches!(parsed.scheme(), "http" | "https")
        .then_some(())
        .and_then(|_| parsed.host_str())
        .map(|_| value)
}

pub(super) fn classify_media_source_reference(value: &str) -> MediaSourceReference<'_> {
    if let Some((mime_type, data)) = base64_data_uri_parts(value) {
        return MediaSourceReference::MimeDataUri { mime_type, data };
    }
    if value
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"))
    {
        return MediaSourceReference::Unsupported { value };
    }
    if let Some(url) = http_or_https_remote_url(value) {
        return MediaSourceReference::HttpRemoteUrl { url };
    }
    if let Some(value) = clean_uri_like_source_reference(value) {
        if let Some(scheme) = uri_scheme(value) {
            if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") {
                return MediaSourceReference::Unsupported { value };
            }
            return MediaSourceReference::ProviderOrLocalUri { uri: value };
        }
    }
    if looks_like_base64_payload(value) {
        return MediaSourceReference::BareBase64 { data: value };
    }
    MediaSourceReference::Unsupported { value }
}

pub(super) fn validate_media_source_reference(value: &str) -> bool {
    !matches!(
        classify_media_source_reference(value),
        MediaSourceReference::Unsupported { .. }
    )
}

pub(super) fn looks_like_base64_payload(value: &str) -> bool {
    validate_inline_base64_payload(value).is_some()
}

pub(super) fn openai_file_part_maps(
    part: &Value,
) -> impl Iterator<Item = &serde_json::Map<String, Value>> {
    part.as_object()
        .into_iter()
        .chain(part.get("file").and_then(Value::as_object))
}

pub(super) fn openai_file_part_field<'a>(part: &'a Value, field: &str) -> Option<&'a Value> {
    openai_file_part_maps(part).find_map(|map| map.get(field))
}

fn collect_openai_file_mime_candidates_from_maps<'a>(
    maps: impl IntoIterator<Item = &'a serde_json::Map<String, Value>>,
) -> Vec<OpenAiFileMimeCandidate> {
    let mut candidates = Vec::new();
    for map in maps {
        for field in ["mime_type", "mimeType"] {
            if let Some(mime_type) = map
                .get(field)
                .and_then(Value::as_str)
                .and_then(normalized_mime_type)
            {
                candidates.push(OpenAiFileMimeCandidate {
                    source: OpenAiFileMimeSource::Explicit,
                    source_label: "`mime_type`/`mimeType`",
                    mime_type,
                });
            }
        }
        if let Some((mime_type, _)) = map
            .get("file_data")
            .and_then(Value::as_str)
            .and_then(base64_data_uri_parts)
            .and_then(|(mime_type, data)| normalized_mime_type(mime_type).map(|mime| (mime, data)))
        {
            candidates.push(OpenAiFileMimeCandidate {
                source: OpenAiFileMimeSource::FileData,
                source_label: "`file_data` data URI",
                mime_type,
            });
        }
        if let Some((mime_type, _)) = map
            .get("file_url")
            .and_then(Value::as_str)
            .and_then(base64_data_uri_parts)
            .and_then(|(mime_type, data)| normalized_mime_type(mime_type).map(|mime| (mime, data)))
        {
            candidates.push(OpenAiFileMimeCandidate {
                source: OpenAiFileMimeSource::FileData,
                source_label: "`file_url` data URI",
                mime_type,
            });
        }
        if let Some(mime_type) = map
            .get("filename")
            .and_then(Value::as_str)
            .and_then(mime_type_from_filename)
            .and_then(normalized_mime_type)
        {
            candidates.push(OpenAiFileMimeCandidate {
                source: OpenAiFileMimeSource::Filename,
                source_label: "`filename`",
                mime_type,
            });
        }
    }
    candidates
}

fn openai_file_mime_conflict_message(
    left: &OpenAiFileMimeCandidate,
    right: &OpenAiFileMimeCandidate,
) -> String {
    format!(
        "OpenAI file MIME conflict: {} MIME `{}` conflicts with {} MIME `{}`; `mime_type`/`mimeType`, `file_data`, `file_url`, and `filename` MIME provenance must agree.",
        left.source_label, left.mime_type, right.source_label, right.mime_type
    )
}

fn resolve_openai_file_mime(
    candidates: &[OpenAiFileMimeCandidate],
) -> Result<Option<String>, String> {
    let Some(first) = candidates.first() else {
        return Ok(None);
    };
    for candidate in candidates.iter().skip(1) {
        if candidate.mime_type != first.mime_type {
            return Err(openai_file_mime_conflict_message(first, candidate));
        }
    }
    Ok([
        OpenAiFileMimeSource::FileData,
        OpenAiFileMimeSource::Explicit,
        OpenAiFileMimeSource::Filename,
    ]
    .iter()
    .find_map(|source| {
        candidates
            .iter()
            .find(|candidate| candidate.source == *source)
            .map(|candidate| candidate.mime_type.clone())
    }))
}

pub(super) fn openai_file_part_resolved_mime_type(part: &Value) -> Result<Option<String>, String> {
    resolve_openai_file_mime(&collect_openai_file_mime_candidates_from_maps(
        openai_file_part_maps(part),
    ))
}

pub(super) fn openai_file_part_mime_conflict_message(part: &Value) -> Option<String> {
    openai_file_part_resolved_mime_type(part).err()
}

pub(super) fn openai_file_reference_payload(
    part: &Value,
) -> Result<Option<(&str, &'static str)>, String> {
    match openai_file_source(part) {
        OpenAiFileSource::ProviderFileId {
            file_id: Some(file_id),
        } => Err(format!(
            "OpenAI provider file_id `{file_id}` cannot be treated as bytes or a URI for cross-provider media translation."
        )),
        OpenAiFileSource::ProviderFileId { file_id: None } => Err(
            "OpenAI provider file_id fields cannot be treated as bytes or a URI for cross-provider media translation.".to_string(),
        ),
        OpenAiFileSource::Payload { payload, field } => Ok(Some((payload, field))),
        OpenAiFileSource::ConflictingPayloads => Err(
            "OpenAI file parts must include exactly one of file_data or file_url; multiple source fields across the part and nested file object are ambiguous.".to_string(),
        ),
        OpenAiFileSource::InvalidPayload { field } => Err(format!(
            "OpenAI file `{field}` must be a string source payload for cross-provider media translation."
        )),
        OpenAiFileSource::Missing => Ok(None),
    }
}

pub(super) fn openai_file_source(part: &Value) -> OpenAiFileSource<'_> {
    let mut payloads = Vec::new();

    for map in openai_file_part_maps(part) {
        if let Some(file_id) = map.get("file_id") {
            return OpenAiFileSource::ProviderFileId {
                file_id: file_id.as_str(),
            };
        }
        for field in ["file_data", "file_url"] {
            if let Some(value) = map.get(field) {
                payloads.push((field, value));
            }
        }
    }

    if payloads.is_empty() {
        return OpenAiFileSource::Missing;
    }

    if payloads.len() > 1 {
        return OpenAiFileSource::ConflictingPayloads;
    }

    let (field, value) = payloads[0];
    if let Some(payload) = value.as_str() {
        return OpenAiFileSource::Payload { payload, field };
    }

    OpenAiFileSource::InvalidPayload { field }
}

pub(super) fn openai_file_data_reference_from_part<'a>(
    part: &'a Value,
) -> Result<OpenAiFileDataReference<'a>, String> {
    let Some((payload, field)) = openai_file_reference_payload(part)? else {
        return Err(
            "OpenAI file parts require file_data or file_url to translate to Gemini.".to_string(),
        );
    };
    let resolved_mime_type = openai_file_part_resolved_mime_type(part)?;
    openai_file_data_reference_from_payload(payload, field, resolved_mime_type)
}

fn openai_file_data_reference_from_payload<'a>(
    payload: &'a str,
    field: &str,
    resolved_mime_type: Option<String>,
) -> Result<OpenAiFileDataReference<'a>, String> {
    match classify_media_source_reference(payload) {
        MediaSourceReference::MimeDataUri { mime_type, data } => {
            let mime_type = normalized_mime_type(mime_type).ok_or_else(|| {
                format!("OpenAI {field} data URIs need a non-empty MIME type to translate media.")
            })?;
            Ok(OpenAiFileDataReference::InlineData { mime_type, data })
        }
        MediaSourceReference::HttpRemoteUrl { url } => {
            let Some(mime_type) = resolved_mime_type else {
                return Err(format!(
                    "OpenAI {field} HTTP(S) URLs need mime_type/mimeType or filename provenance to translate media; include file.mime_type/file.mimeType or a filename with a known extension."
                ));
            };
            Ok(OpenAiFileDataReference::HttpRemoteUrl { mime_type, url })
        }
        MediaSourceReference::ProviderOrLocalUri { uri } => {
            let Some(mime_type) = resolved_mime_type else {
                return Err(format!(
                    "OpenAI {field} provider/local URI references need mime_type/mimeType or filename provenance to translate media; include file.mime_type/file.mimeType or a filename with a known extension."
                ));
            };
            Ok(OpenAiFileDataReference::ProviderOrLocalUri { mime_type, uri })
        }
        MediaSourceReference::BareBase64 { data } => Ok(OpenAiFileDataReference::BareBase64 {
            mime_type: resolved_mime_type,
            data,
        }),
        MediaSourceReference::Unsupported { value } => Err(format!(
            "OpenAI {field} must be a MIME-bearing data URI, an HTTP(S) URL, a provider/local URI reference, or a base64 payload with filename provenance to translate media; got unsupported source `{value}`."
        )),
    }
}

pub(super) fn is_pdf_mime(mime_type: &str) -> bool {
    normalized_mime_type(mime_type).as_deref() == Some("application/pdf")
}
