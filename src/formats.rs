//! Format identifiers for the four supported LLM API shapes.

use serde::{Deserialize, Serialize};
use std::fmt;

/// The four supported endpoint formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpstreamFormat {
    /// Google (Gemini) — e.g. generateContent, contents[].
    Google,
    /// Anthropic (Claude) — e.g. /v1/messages, messages[], content blocks.
    Anthropic,
    /// OpenAI Chat Completions — /v1/chat/completions, messages[].
    OpenAiCompletion,
    /// OpenAI Responses API — /v1/responses, input[], instructions.
    OpenAiResponses,
}

impl fmt::Display for UpstreamFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UpstreamFormat::Google => write!(f, "google"),
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
            "google" | "gemini" => Ok(UpstreamFormat::Google),
            "anthropic" | "claude" => Ok(UpstreamFormat::Anthropic),
            "openai" | "openai-completion" | "chat" => Ok(UpstreamFormat::OpenAiCompletion),
            "openai-responses" | "responses" => Ok(UpstreamFormat::OpenAiResponses),
            _ => Err(format!("unknown format: {}", s)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_str_google_gemini() {
        assert_eq!(
            "google".parse::<UpstreamFormat>().unwrap(),
            UpstreamFormat::Google
        );
        assert_eq!(
            "gemini".parse::<UpstreamFormat>().unwrap(),
            UpstreamFormat::Google
        );
        assert_eq!(
            "GOOGLE".parse::<UpstreamFormat>().unwrap(),
            UpstreamFormat::Google
        );
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
        assert_eq!(UpstreamFormat::Google.to_string(), "google");
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
