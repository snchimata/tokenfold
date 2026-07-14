use clap::ValueEnum;
use tokenfold_core::InputFormat;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FormatArg {
    Auto,
    #[value(alias = "openai_json")]
    Openai,
    #[value(alias = "anthropic_json")]
    Anthropic,
    /// Generic JSON data (not an LLM message payload).
    Json,
    #[value(alias = "plain_text")]
    Text,
    #[value(alias = "command_output")]
    Command,
    #[value(alias = "git_diff")]
    Diff,
}

impl FormatArg {
    pub fn to_input_format(self) -> InputFormat {
        match self {
            FormatArg::Auto => InputFormat::Auto,
            FormatArg::Openai => InputFormat::OpenAiJson,
            FormatArg::Anthropic => InputFormat::AnthropicJson,
            FormatArg::Json => InputFormat::Json,
            FormatArg::Text => InputFormat::PlainText,
            FormatArg::Command => InputFormat::CommandOutput,
            FormatArg::Diff => InputFormat::GitDiff,
        }
    }

    /// Accepts both the short CLI flag spellings ("openai") and the long report-label
    /// spellings ("openai_json") that appear in `tokenfold.toml`/env docs.
    pub fn parse(s: &str) -> Result<Self, String> {
        <FormatArg as ValueEnum>::from_str(s, true)
    }
}

/// Resolves `InputFormat::Auto` per INTERFACES.md §"InputFormat::Auto Detection": a
/// deliberately conservative sniff, JSON shape first, then diff markers, then plain text.
/// `from_wrap` is the one caller-supplied hint (rule 4: unformatted `wrap` output defaults to
/// `CommandOutput` rather than `PlainText`).
pub fn detect_format(bytes: &[u8], from_wrap: bool) -> InputFormat {
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) {
        if let Some(obj) = value.as_object()
            && let Some(messages) = obj.get("messages").and_then(|m| m.as_array())
            && !messages.is_empty()
        {
            return if obj.contains_key("system") {
                InputFormat::AnthropicJson
            } else {
                InputFormat::OpenAiJson
            };
        }
        // Valid JSON that isn't a message payload → generic JSON data. Only structured
        // objects/arrays are worth the JSON transforms; a bare JSON scalar (`42`, `"hi"`)
        // is indistinguishable from plain text, so let it fall through.
        if value.is_object() || value.is_array() {
            return InputFormat::Json;
        }
    }

    let text = String::from_utf8_lossy(bytes);
    let first_line = text.lines().next().unwrap_or("");
    if first_line.starts_with("diff --git")
        || first_line.starts_with("@@")
        || text.contains("\n+++ ")
    {
        return InputFormat::GitDiff;
    }

    if from_wrap {
        InputFormat::CommandOutput
    } else {
        InputFormat::PlainText
    }
}
