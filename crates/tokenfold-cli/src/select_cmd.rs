use crate::args::Input;
use clap::Args;
use std::io::Read;
use std::path::PathBuf;
use tokenfold_adapters::select::{ApprovedScorer, Context, ContextGroup, SelectPolicy};
use tokenfold_core::TokenFoldError;
use tokenfold_core::token_estimator::TokenEstimator;

#[derive(Args)]
pub struct SelectArgs {
    #[arg(default_value = "-")]
    input: Input,
    #[arg(long)]
    query: String,
    #[arg(long)]
    target_tokens: usize,
    /// Explicitly approved direct-child runtime configuration; absent means declared fallback.
    #[arg(long)]
    scorer_config: Option<PathBuf>,
    #[arg(long, default_value_t = 5000, value_parser = clap::value_parser!(u64).range(1..=30000))]
    inference_timeout_ms: u64,
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Separate selection receipt (not a compression receipt); defaults to stderr.
    #[arg(long)]
    receipt_file: Option<PathBuf>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    #[serde(default)]
    prefix: String,
    #[serde(default)]
    suffix: String,
    groups: Vec<ContextGroup>,
}

fn bounded(input: &Input, max: u64) -> Result<Vec<u8>, TokenFoldError> {
    let reader: Box<dyn Read> = match input {
        Input::Stdin => Box::new(std::io::stdin()),
        Input::Path(path) => Box::new(std::fs::File::open(path)?),
    };
    let mut bytes = Vec::new();
    reader.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(TokenFoldError::InvalidInput(
            "Select input exceeds its byte limit".into(),
        ));
    }
    Ok(bytes)
}

pub fn run(
    args: SelectArgs,
    experimental: bool,
    estimator: &dyn TokenEstimator,
) -> Result<(), TokenFoldError> {
    if !experimental {
        return Err(TokenFoldError::ConfigError(
            "context selection requires --experimental; task quality is not qualified".into(),
        ));
    }
    let bytes = bounded(&args.input, 2 * 1024 * 1024)?;
    if tokenfold_core::transforms::redaction::contains_secret(&bytes) {
        return Err(TokenFoldError::SafetyViolation(
            "secret-shaped context refused before selection or scorer invocation".into(),
        ));
    }
    let source: Source = serde_json::from_slice(&bytes)
        .map_err(|_| TokenFoldError::InvalidInput("invalid Select context document".into()))?;
    // Check decoded/assembled text too: JSON escapes or split groups must not bypass redaction.
    let decoded = format!(
        "{}{}{}",
        source.prefix,
        source
            .groups
            .iter()
            .map(|g| g.text.as_str())
            .collect::<String>(),
        source.suffix
    );
    if tokenfold_core::transforms::redaction::contains_secret(decoded.as_bytes()) {
        return Err(TokenFoldError::SafetyViolation(
            "secret-shaped decoded context refused before output".into(),
        ));
    }
    let runtime: Option<ApprovedScorer> = args
        .scorer_config
        .as_ref()
        .map(|path| {
            let bytes = bounded(&Input::Path(path.clone()), 65536)?;
            serde_json::from_slice(&bytes)
                .map_err(|_| TokenFoldError::ConfigError("invalid scorer approval document".into()))
        })
        .transpose()?;
    let context = Context {
        prefix: &source.prefix,
        suffix: &source.suffix,
        groups: &source.groups,
    };
    let policy = SelectPolicy {
        enabled: true,
        target_tokens: args.target_tokens,
        inference_timeout: std::time::Duration::from_millis(args.inference_timeout_ms),
    };
    let result = tokenfold_adapters::select::select_context(
        &context,
        &args.query,
        &policy,
        estimator,
        runtime.as_ref(),
        None,
    );
    let receipt = serde_json::json!({"schema_version":"1.0", "kind":"context_selection",
        "selected":result.selected,"used_scorer":result.used_scorer,"fallback_reason":result.fallback_reason,
        "baseline_tokens":result.baseline_tokens,"output_tokens":result.output_tokens,
        "target_tokens":args.target_tokens,"budget_met":result.output_tokens <= args.target_tokens,
        "kept_group_count":result.kept_group_ids.len(),"estimator":estimator.info(),
        "scorer_revision":runtime.as_ref().filter(|_| result.used_scorer).map(|r| &r.model_revision)});
    crate::write_payload(args.output.as_deref(), result.text.as_bytes())?;
    let bytes = crate::json_pretty(&receipt)? + "\n";
    if let Some(path) = args.receipt_file {
        std::fs::write(path, bytes)?;
    } else {
        use std::io::Write;
        std::io::stderr().write_all(bytes.as_bytes())?;
    }
    Ok(())
}
