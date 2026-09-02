use clap::ValueEnum;
use serde::Deserialize;
use tokenfold_core::{DecodeFormat, OutputEncoding, Preset, TaskScope};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetArg {
    Conservative,
    Balanced,
    Aggressive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum EncodingArg {
    Json,
    Toon,
}

impl EncodingArg {
    pub fn to_core(self) -> OutputEncoding {
        match self {
            Self::Json => OutputEncoding::Json,
            Self::Toon => OutputEncoding::Toon,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum DecodeFormatArg {
    #[default]
    Auto,
    Json,
    Toon,
    Text,
}

impl DecodeFormatArg {
    pub fn to_core(self) -> DecodeFormat {
        match self {
            Self::Auto => DecodeFormat::Auto,
            Self::Json => DecodeFormat::Json,
            Self::Toon => DecodeFormat::Toon,
            Self::Text => DecodeFormat::Text,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum ReceiptFormatArg {
    #[default]
    Json,
    Text,
}

impl PresetArg {
    pub fn to_core(self) -> Preset {
        match self {
            PresetArg::Conservative => Preset::Conservative,
            PresetArg::Balanced => Preset::Balanced,
            PresetArg::Aggressive => Preset::Aggressive,
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        <PresetArg as ValueEnum>::from_str(s, true)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Deserialize)]
#[value(rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum TaskScopeArg {
    All,
    General,
    CodeReview,
    ChangeSummary,
    Debugging,
    Generation,
    ApiOverview,
    RetrievalQa,
    AgentHistory,
}

impl TaskScopeArg {
    pub fn to_core(self) -> TaskScope {
        match self {
            TaskScopeArg::All => TaskScope::All,
            TaskScopeArg::General => TaskScope::General,
            TaskScopeArg::CodeReview => TaskScope::CodeReview,
            TaskScopeArg::ChangeSummary => TaskScope::ChangeSummary,
            TaskScopeArg::Debugging => TaskScope::Debugging,
            TaskScopeArg::Generation => TaskScope::Generation,
            TaskScopeArg::ApiOverview => TaskScope::ApiOverview,
            TaskScopeArg::RetrievalQa => TaskScope::RetrievalQa,
            TaskScopeArg::AgentHistory => TaskScope::AgentHistory,
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        <TaskScopeArg as ValueEnum>::from_str(s, true)
    }
}

/// "-" (or the flag being absent, via `#[arg(default_value = "-")]`) reads stdin.
#[derive(Debug, Clone)]
pub enum Input {
    Stdin,
    Path(std::path::PathBuf),
}

impl std::str::FromStr for Input {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(if s == "-" {
            Input::Stdin
        } else {
            Input::Path(std::path::PathBuf::from(s))
        })
    }
}

impl Input {
    pub fn read(&self) -> std::io::Result<Vec<u8>> {
        use std::io::Read;
        match self {
            Input::Stdin => {
                let mut buf = Vec::new();
                std::io::stdin().read_to_end(&mut buf)?;
                Ok(buf)
            }
            Input::Path(path) => std::fs::read(path),
        }
    }
}
