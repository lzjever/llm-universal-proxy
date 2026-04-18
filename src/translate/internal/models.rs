use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TranslationIssueLevel {
    Warning,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TranslationIssue {
    pub level: TranslationIssueLevel,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TranslationAssessment {
    pub issues: Vec<TranslationIssue>,
}

impl TranslationAssessment {
    pub(crate) fn warning(&mut self, message: impl Into<String>) {
        self.issues.push(TranslationIssue {
            level: TranslationIssueLevel::Warning,
            message: message.into(),
        });
    }

    pub(crate) fn reject(&mut self, message: impl Into<String>) {
        self.issues.push(TranslationIssue {
            level: TranslationIssueLevel::Reject,
            message: message.into(),
        });
    }

    pub(crate) fn decision(&self) -> TranslationDecision {
        let mut warnings = Vec::new();
        for issue in &self.issues {
            match issue.level {
                TranslationIssueLevel::Reject => {
                    return TranslationDecision::Reject(issue.message.clone());
                }
                TranslationIssueLevel::Warning => warnings.push(issue.message.clone()),
            }
        }
        if warnings.is_empty() {
            TranslationDecision::Allow
        } else {
            TranslationDecision::AllowWithWarnings(warnings)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TranslationDecision {
    Allow,
    AllowWithWarnings(Vec<String>),
    Reject(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SemanticToolKind {
    Function,
    OpenAiCustom,
    AnthropicServerTool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SemanticToolResultContent {
    Text(String),
    Json(Value),
    TypedBlocks(Vec<Value>),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SemanticTextPart {
    pub text: String,
    pub annotations: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NormalizedToolPolicy {
    Auto,
    None,
    Required,
    ForcedFunction(String),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NormalizedJsonSchemaOutputShape {
    pub name: String,
    pub schema: Value,
    pub description: Option<String>,
    pub strict: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NormalizedOutputShape {
    Text,
    JsonObject,
    JsonSchema(NormalizedJsonSchemaOutputShape),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct NormalizedDecodingControls {
    pub stop: Option<Value>,
    pub seed: Option<Value>,
    pub presence_penalty: Option<Value>,
    pub frequency_penalty: Option<Value>,
    pub top_k: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NormalizedLogprobsControls {
    pub enabled: bool,
    pub top_logprobs: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NormalizedResponseLogprobCandidate {
    pub raw: Value,
    pub token: String,
    pub logprob: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NormalizedResponseTokenLogprob {
    pub raw: Value,
    pub token: String,
    pub logprob: f64,
    pub top_logprobs: Vec<NormalizedResponseLogprobCandidate>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct NormalizedRequestControls {
    pub tool_policy: Option<NormalizedToolPolicy>,
    pub restricted_tool_names: Option<Vec<String>>,
    pub output_shape: Option<NormalizedOutputShape>,
    pub decoding: NormalizedDecodingControls,
    pub logprobs: Option<NormalizedLogprobsControls>,
    pub metadata: Option<Value>,
    pub user: Option<Value>,
    pub service_tier: Option<Value>,
    pub stream_include_obfuscation: Option<Value>,
    pub verbosity: Option<Value>,
    pub reasoning_effort: Option<Value>,
    pub prompt_cache_key: Option<Value>,
    pub prompt_cache_retention: Option<Value>,
    pub safety_identifier: Option<Value>,
    pub parallel_tool_calls: Option<Value>,
    pub store: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NormalizedOpenAiFamilyFunctionTool {
    pub name: String,
    pub description: Option<Value>,
    pub parameters: Option<Value>,
    pub strict: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NormalizedOpenAiFamilyCustomTool {
    pub name: String,
    pub description: Option<Value>,
    pub format: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NormalizedOpenAiFamilyNamespaceTool {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NormalizedOpenAiFamilyToolDef {
    Function(NormalizedOpenAiFamilyFunctionTool),
    Custom(NormalizedOpenAiFamilyCustomTool),
    Namespace(NormalizedOpenAiFamilyNamespaceTool),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NormalizedOpenAiFamilyToolCall {
    Function {
        id: Option<Value>,
        name: String,
        arguments: String,
        namespace: Option<String>,
        proxied_tool_kind: Option<Value>,
    },
    Custom {
        id: Option<Value>,
        name: String,
        input: String,
        namespace: Option<String>,
        proxied_tool_kind: Option<Value>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NormalizedOpenAiAudioContract {
    pub response_modalities: Vec<String>,
    pub voice_name: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SharedControlProfile {
    pub metadata: bool,
    pub user: bool,
    pub service_tier: bool,
    pub stream_include_obfuscation: bool,
    pub verbosity: bool,
    pub reasoning_effort: bool,
    pub prompt_cache_key: bool,
    pub prompt_cache_retention: bool,
    pub safety_identifier: bool,
    pub top_logprobs: bool,
    pub parallel_tool_calls: bool,
    pub logit_bias: bool,
}
