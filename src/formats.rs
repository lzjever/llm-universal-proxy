//! Format identifiers for the supported LLM API shapes.

use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;

/// The supported endpoint formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum UpstreamFormat {
    /// Anthropic (Claude) — e.g. /v1/messages, messages[], content blocks.
    #[serde(rename = "anthropic", alias = "claude")]
    Anthropic,
    /// OpenAI Chat Completions — /v1/chat/completions, messages[].
    #[serde(rename = "openai-completion", alias = "openai", alias = "chat")]
    OpenAiCompletion,
    /// OpenAI Responses API — /v1/responses, input[], instructions.
    #[serde(rename = "openai-responses", alias = "responses")]
    OpenAiResponses,
}

pub(crate) fn removed_native_gemini_format_message() -> String {
    "native Gemini format support has been removed; use Google OpenAI-compatible endpoint https://generativelanguage.googleapis.com/v1beta/openai with format: openai-completion".to_string()
}

impl fmt::Display for UpstreamFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UpstreamFormat::Anthropic => write!(f, "anthropic"),
            UpstreamFormat::OpenAiCompletion => write!(f, "openai-completion"),
            UpstreamFormat::OpenAiResponses => write!(f, "openai-responses"),
        }
    }
}

impl std::str::FromStr for UpstreamFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "google" | "gemini" => Err(removed_native_gemini_format_message()),
            "anthropic" | "claude" => Ok(UpstreamFormat::Anthropic),
            "openai" | "openai-completion" | "chat" => Ok(UpstreamFormat::OpenAiCompletion),
            "openai-responses" | "responses" => Ok(UpstreamFormat::OpenAiResponses),
            _ => Err(format!("unknown format: {s}")),
        }
    }
}

impl<'de> Deserialize<'de> for UpstreamFormat {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_str_rejects_removed_native_gemini_formats_with_migration_hint() {
        for removed in ["google", "gemini", "GOOGLE"] {
            let error = removed
                .parse::<UpstreamFormat>()
                .expect_err("native Gemini formats must be removed");
            assert!(
                error.contains("format: openai-completion"),
                "error should explain the OpenAI-compatible migration path: {error}"
            );
            assert!(
                error.contains("generativelanguage.googleapis.com/v1beta/openai"),
                "error should name the Google OpenAI-compatible endpoint: {error}"
            );
        }
    }

    #[test]
    fn from_str_anthropic_claude() {
        assert_eq!(
            "anthropic".parse::<UpstreamFormat>().unwrap(),
            UpstreamFormat::Anthropic
        );
        assert_eq!(
            "claude".parse::<UpstreamFormat>().unwrap(),
            UpstreamFormat::Anthropic
        );
    }

    #[test]
    fn from_str_openai_completion() {
        assert_eq!(
            "openai".parse::<UpstreamFormat>().unwrap(),
            UpstreamFormat::OpenAiCompletion
        );
        assert_eq!(
            "openai-completion".parse::<UpstreamFormat>().unwrap(),
            UpstreamFormat::OpenAiCompletion
        );
        assert_eq!(
            "chat".parse::<UpstreamFormat>().unwrap(),
            UpstreamFormat::OpenAiCompletion
        );
    }

    #[test]
    fn from_str_openai_responses() {
        assert_eq!(
            "openai-responses".parse::<UpstreamFormat>().unwrap(),
            UpstreamFormat::OpenAiResponses
        );
        assert_eq!(
            "responses".parse::<UpstreamFormat>().unwrap(),
            UpstreamFormat::OpenAiResponses
        );
    }

    #[test]
    fn from_str_invalid() {
        assert!("foo".parse::<UpstreamFormat>().is_err());
        assert!("".parse::<UpstreamFormat>().is_err());
    }

    #[test]
    fn display() {
        assert_eq!(UpstreamFormat::Anthropic.to_string(), "anthropic");
        assert_eq!(
            UpstreamFormat::OpenAiCompletion.to_string(),
            "openai-completion"
        );
        assert_eq!(
            UpstreamFormat::OpenAiResponses.to_string(),
            "openai-responses"
        );
    }
}
