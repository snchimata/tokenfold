use crate::args::Input;
use clap::Args;
use std::path::PathBuf;
use tokenfold_adapters::select::ApprovedScorer;
use tokenfold_core::TokenFoldError;
use tokenfold_core::generative::{Source, SummaryArtifact, compile, skip_generation};
use tokenfold_core::token_estimator::TokenEstimator;

#[derive(Args)]
pub struct SummarizeArgs {
    #[arg(default_value = "-")]
    input: Input,
    #[arg(long)]
    query: String,
    #[arg(long)]
    target_tokens: usize,
    /// Explicit approval for a local direct-child model runtime (same fields as scorer-config).
    #[arg(long)]
    model_config: PathBuf,
    #[arg(long, default_value_t = 5000, value_parser = clap::value_parser!(u64).range(1..=30000))]
    inference_timeout_ms: u64,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(long)]
    receipt_file: Option<PathBuf>,
    /// Save an accepted unverified candidate for explicit same-input reuse. Refuses overwrite.
    #[arg(long, conflicts_with = "reuse_summary")]
    save_summary: Option<PathBuf>,
    /// Revalidate a caller-owned artifact; never invokes a model or silently regenerates.
    #[arg(long, conflicts_with = "save_summary")]
    reuse_summary: Option<PathBuf>,
}

pub fn run(
    args: SummarizeArgs,
    experimental: bool,
    estimator: &dyn TokenEstimator,
) -> Result<(), TokenFoldError> {
    if !experimental {
        return Err(TokenFoldError::ConfigError(
            "generative summarization requires --experimental; generated text is unverified".into(),
        ));
    }
    let bytes = crate::select_cmd::bounded(&args.input, 2 * 1024 * 1024)?;
    if tokenfold_core::transforms::redaction::contains_secret(&bytes) {
        return Err(TokenFoldError::SafetyViolation(
            "secret-shaped summarizer document refused".into(),
        ));
    }
    let source: Source = serde_json::from_slice(&bytes)
        .map_err(|_| TokenFoldError::InvalidInput("invalid summarizer source document".into()))?;
    let baseline = source.baseline(&args.query)?;
    let bytes = crate::select_cmd::bounded(&Input::Path(args.model_config), 65536)?;
    let approval_bytes = bytes;
    let runtime: ApprovedScorer = serde_json::from_slice(&approval_bytes)
        .map_err(|_| TokenFoldError::ConfigError("invalid model approval document".into()))?;
    if tokenfold_core::transforms::redaction::contains_secret(runtime.model_revision.as_bytes()) {
        return Err(TokenFoldError::SafetyViolation(
            "secret-shaped model revision refused".into(),
        ));
    }
    if let Some(artifact) = args.save_summary.as_ref().or(args.reuse_summary.as_ref()) {
        let normalize = |path: &std::path::Path| -> Result<PathBuf, TokenFoldError> {
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(std::path::Path::new("."));
            let name = path.file_name().ok_or_else(|| {
                TokenFoldError::ConfigError("artifact path must name a file".into())
            })?;
            Ok(parent.canonicalize()?.join(name))
        };
        let artifact = match normalize(artifact) {
            Ok(path) => Some(path),
            Err(error) if args.save_summary.is_some() => return Err(error),
            Err(_) => None, // A missing replay parent follows the declared raw fallback.
        };
        for output in [args.output.as_ref(), args.receipt_file.as_ref()]
            .into_iter()
            .flatten()
        {
            if artifact
                .as_ref()
                .is_some_and(|artifact| normalize(output).is_ok_and(|output| output == *artifact))
            {
                return Err(TokenFoldError::ConfigError(
                    "artifact and output/receipt paths must differ".into(),
                ));
            }
        }
        if args.save_summary.is_some() && artifact.as_ref().is_some_and(|path| path.exists()) {
            return Err(TokenFoldError::ConfigError(
                "summary artifact already exists".into(),
            ));
        }
    }
    let started = std::time::Instant::now();
    let mut inference_usage = None;
    let mut reused_inference_usage = None;
    let mut artifact_reused = false;
    let mut runtime_invoked = false;
    let mut artifact_to_save = None;
    let skip = skip_generation(&source, &baseline, args.target_tokens, estimator);
    let candidate = if let Some(path) = &args.reuse_summary {
        (|| {
            let bytes = crate::select_cmd::bounded(&Input::Path(path.clone()), 65536)
                .map_err(|_| "artifact_unavailable")?;
            let artifact: SummaryArtifact =
                serde_json::from_slice(&bytes).map_err(|_| "malformed_artifact")?;
            if !artifact.matches(
                &source,
                &args.query,
                &approval_bytes,
                args.target_tokens,
                estimator,
            ) {
                return Err("artifact_context_mismatch");
            }
            reused_inference_usage = artifact.candidate.inference_usage.clone();
            Ok(artifact.candidate)
        })()
    } else if let Some(reason) = skip {
        Err(reason)
    } else {
        runtime_invoked = true;
        tokenfold_adapters::generative::generate(
            &source,
            &args.query,
            args.target_tokens,
            &runtime,
            std::time::Duration::from_millis(args.inference_timeout_ms),
        )
    };
    let result = candidate.and_then(|candidate| {
        if runtime_invoked {
            inference_usage = candidate.inference_usage.clone();
        }
        let output = compile(
            &source,
            &candidate,
            &runtime.model_revision,
            &baseline,
            args.target_tokens,
            estimator,
        )?;
        artifact_reused = args.reuse_summary.is_some();
        if args.save_summary.is_some() {
            artifact_to_save = Some(SummaryArtifact::new(
                &source,
                &args.query,
                &approval_bytes,
                args.target_tokens,
                estimator,
                candidate,
            ));
        }
        Ok(output)
    });
    let (text, reason) = match result {
        Ok(text) => (text, None),
        Err(reason) => (baseline.clone(), Some(reason)),
    };
    let output_tokens = estimator.count_bytes(text.as_bytes());
    let receipt = serde_json::json!({"schema_version":"1.0","kind":"generative_summary",
        "disposition":if reason.is_none(){"generated_unverified"}else{"raw_fallback"},
        "semantic_verification":"unverified-not-production-admissible",
        "citation_origin":"native-complete-source-groups-not-model-quotes",
        "fallback_reason":reason,"model_revision":runtime.model_revision,
        "baseline_tokens":estimator.count_bytes(baseline.as_bytes()),"output_tokens":output_tokens,
        "target_tokens":args.target_tokens,"budget_met":output_tokens<=args.target_tokens,
        "estimator":estimator.info(),"wall_ms":started.elapsed().as_millis(),
        "runtime_invoked":runtime_invoked,"artifact_reused":artifact_reused,
        "reused_inference_usage":reused_inference_usage,
        "inference_usage":inference_usage,"usage_provenance":if inference_usage.is_some(){"approved-runtime-reported"}else{"unknown"},"billed_cost":null});
    if let (Some(path), Some(artifact)) = (&args.save_summary, artifact_to_save) {
        let encoded = serde_json::to_vec(&artifact)
            .map_err(|_| TokenFoldError::InternalError("artifact serialization failed".into()))?;
        if encoded.len() > 65536 {
            return Err(TokenFoldError::InvalidInput(
                "artifact exceeds byte limit".into(),
            ));
        }
        use std::io::Write;
        // User explicitly opts into persistence. Private parent-directory permissions
        // are caller-owned; never overwrite a prior artifact or arbitrary existing file.
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?
            .write_all(&encoded)?;
    }
    crate::write_payload(args.output.as_deref(), text.as_bytes())?;
    let receipt = crate::json_pretty(&receipt)? + "\n";
    if let Some(path) = args.receipt_file {
        std::fs::write(path, receipt)?;
    } else {
        use std::io::Write;
        std::io::stderr().write_all(receipt.as_bytes())?;
    }
    Ok(())
}
