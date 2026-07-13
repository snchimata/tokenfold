use clap::ValueEnum;
use serde::Deserialize;
use tokenfold_core::{CompressionMode, TaskScope};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModeArg {
    Conservative,
    Balanced,
    Aggressive,
}

impl ModeArg {
    pub fn to_core(self) -> CompressionMode {
        match self {
            ModeArg::Conservative => CompressionMode::Conservative,
            ModeArg::Balanced => CompressionMode::Balanced,
            ModeArg::Aggressive => CompressionMode::Aggressive,
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        <ModeArg as ValueEnum>::from_str(s, true)
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
