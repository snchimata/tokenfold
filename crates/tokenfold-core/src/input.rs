use serde::{Deserialize, Serialize};

use crate::report::CompressionReport;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InputFormat {
    Auto,
    OpenAiJson,
    AnthropicJson,
    PlainText,
    CommandOutput,
    GitDiff,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompressionInput {
    pub format: InputFormat,
    pub bytes: Vec<u8>,
}

impl CompressionInput {
    pub fn openai_json(bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            format: InputFormat::OpenAiJson,
            bytes: bytes.into(),
        }
    }

    pub fn anthropic_json(bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            format: InputFormat::AnthropicJson,
            bytes: bytes.into(),
        }
    }

    pub fn plain_text(bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            format: InputFormat::PlainText,
            bytes: bytes.into(),
        }
    }

    pub fn command_output(bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            format: InputFormat::CommandOutput,
            bytes: bytes.into(),
        }
    }

    pub fn git_diff(bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            format: InputFormat::GitDiff,
            bytes: bytes.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)] // NOT Eq: report holds f64
pub struct CompressionOutput {
    pub bytes: Vec<u8>,
    pub report: CompressionReport,
}
