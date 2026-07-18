mod args;
mod config;
mod diff;
mod format;
mod mcp;
mod render;
mod rtk;
mod stats_cmd;

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use tokenfold_core::report::{CommandReport, EstimatorInfo, PipelineReport, PipelineStageReport};
use tokenfold_core::token_estimator::{ByteHeuristicEstimator, TiktokenEstimator, TokenEstimator};
use tokenfold_core::{CompressionInput, CompressionPolicy, InputFormat, TokenFoldError};

use args::{Input, ModeArg, TaskScopeArg};
use config::CliOverrides;
use format::FormatArg;

#[derive(Parser)]
#[command(
    name = "tokenfold",
    version,
    about = "Token-aware compression for LLM payloads"
)]
struct Cli {
    #[arg(long, global = true)]
    json: bool,
    #[arg(long = "no-color", global = true)]
    no_color: bool,
    #[arg(long, global = true)]
    quiet: bool,
    #[arg(long = "unsafe-disable-redaction", global = true)]
    unsafe_disable_redaction: bool,
    #[arg(long, global = true)]
    experimental: bool,
    #[arg(long = "task-scope", global = true)]
    task_scope: Option<TaskScopeArg>,
    #[arg(long = "enable", global = true, value_delimiter = ',')]
    enable: Vec<String>,
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,
    #[arg(long = "no-truncate", global = true)]
    no_truncate: bool,
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Dry-run preview of achievable savings (previews per-transform even with no target).
    Inspect {
        #[arg(default_value = "-")]
        input: Input,
        #[arg(long)]
        format: Option<FormatArg>,
        #[arg(long)]
        target_tokens: Option<usize>,
        #[arg(long)]
        mode: Option<ModeArg>,
        #[arg(long)]
        list_transforms: bool,
    },
    /// Compress; payload -> stdout, report -> stderr (or --json).
    Compress {
        #[arg(default_value = "-")]
        input: Input,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long)]
        format: Option<FormatArg>,
        #[arg(long)]
        target_tokens: Option<usize>,
        #[arg(long)]
        mode: Option<ModeArg>,
        #[arg(long, value_delimiter = ',')]
        disable: Vec<String>,
        /// Routes to the same code path as `inspect`: no stdout payload, report only.
        #[arg(long)]
        dry_run: bool,
        /// F-045: persist the original payload to the reversible evidence store, keyed by its
        /// SHA-256 hash, unless it contains secret-shaped content.
        #[arg(long = "store-originals")]
        store_originals: bool,
        /// F-045: namespace stored originals are keyed under (see `tokenfold retrieve`).
        #[arg(long = "retrieve-namespace")]
        retrieve_namespace: Option<String>,
    },
    /// Compression-aware diff of two payloads.
    Diff { raw: Input, compressed: Input },
    /// Run a command and compress its captured output. `shell` is a visible alias.
    #[command(visible_alias = "shell", alias = "exec")]
    Wrap {
        /// F-045: persist the captured output to the reversible evidence store.
        #[arg(long = "store-originals")]
        store_originals: bool,
        /// F-045: namespace stored originals are keyed under (see `tokenfold retrieve`).
        #[arg(long = "retrieve-namespace")]
        retrieve_namespace: Option<String>,
        /// F-054: route command output through an external RTK before tokenfold's generic
        /// pipeline. Falls open to the tokenfold-only path if RTK is missing/incompatible.
        #[arg(long = "rtk")]
        rtk: bool,
        /// F-055: opt into CCR — hand RTK a permission-restricted per-run tee dir, ingest the
        /// pre-RTK raw capture through redaction/persistence, then delete it. Requires `--rtk`.
        #[arg(long = "rtk-capture-raw", requires = "rtk")]
        rtk_capture_raw: bool,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        argv: Vec<String>,
    },
    /// Compress each fixture and report before/after tokens.
    Benchmark {
        fixtures: Vec<PathBuf>,
        #[arg(long)]
        format: Option<FormatArg>,
    },
    /// Install a durable agent/host integration.
    Init {
        #[arg(long)]
        agent: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove a durable agent/host integration.
    Uninit {
        #[arg(long)]
        agent: String,
    },
    /// Verify estimator availability, config validity, and host integration status.
    Doctor {
        #[arg(long)]
        agent: Option<String>,
    },
    /// Model Context Protocol server surface.
    Mcp {
        #[command(subcommand)]
        action: McpAction,
    },
    /// F-045: restore an original payload stored via `--store-originals`.
    Retrieve {
        /// A raw hex SHA-256 hash, a `[tokenfold:retrieve ...]` marker, or a path to a
        /// `CompressionReport` JSON file.
        reference: String,
        /// Namespace to look the hash up under; defaults to the resolved `[retrieval]`
        /// namespace (or the marker's own `namespace=` field, when the reference is a marker).
        #[arg(long = "retrieve-namespace")]
        retrieve_namespace: Option<String>,
    },
    /// F-046: aggregate ad-hoc `CompressionReport` JSON files and/or the local ledger.
    Stats {
        /// Glob(s) matching `CompressionReport` JSON files (e.g. `reports/*.json`). A bare
        /// existing file path also works. Aggregation always additionally includes the local
        /// ledger when `[analytics].enabled` is true.
        report_globs: Vec<String>,
        #[arg(long)]
        csv: bool,
        #[arg(long)]
        scope: Option<String>,
        #[arg(long)]
        window: Option<String>,
        /// Overrides the resolved `[analytics].ledger_db` path for this run only.
        #[arg(long)]
        ledger: Option<PathBuf>,
    },
    /// F-046: realized token/cost savings summary from the local ledger.
    Gain {
        #[arg(long)]
        scope: Option<String>,
        /// A duration shorthand like "30d", "24h", "90m"; defaults to "30d".
        #[arg(long)]
        since: Option<String>,
        #[arg(long)]
        csv: bool,
    },
    /// F-046: host-session command-wrapping coverage from the local ledger.
    Session {
        #[arg(long)]
        recent: Option<usize>,
    },
    /// F-047: declarative command-output filter registry.
    Filters {
        #[command(subcommand)]
        action: FiltersAction,
    },
    /// Phase 6: output-token-shaping report -- measured (give SHAPED) or estimated (give
    /// --ratio, projected from a known input compression ratio).
    OutputSavings {
        baseline: Input,
        shaped: Option<Input>,
        #[arg(long)]
        ratio: Option<f64>,
    },
    /// Phase 6: mine the local ledger for policy-change proposals. Never silently changes
    /// tokenfold.toml -- prints proposals only, unless --apply is passed.
    #[command(visible_alias = "discover")]
    Learn {
        /// Write the first proposed change into tokenfold.toml instead of only printing it.
        #[arg(long)]
        apply: bool,
    },
}

#[derive(Subcommand)]
enum McpAction {
    /// Start the MCP stdio server (blocks until stdin closes); see INTERFACES.md §4.
    Serve,
}

#[derive(Subcommand)]
enum FiltersAction {
    /// List built-in, project, and user filters with their trust status.
    List,
    /// Validate schema, regex safety, and inline fixtures for every discovered filter pack.
    Verify {
        /// CI contract (INTERFACES.md §7.3): any failure becomes a non-zero exit.
        #[arg(long = "require-all")]
        require_all: bool,
    },
    /// Record a filter pack's canonical path + SHA-256 + schema_version into the trust store.
    Trust { path: PathBuf },
}

struct GlobalFlags {
    json: bool,
    no_color: bool,
    quiet: bool,
    unsafe_disable_redaction: bool,
    experimental: bool,
    task_scope: Option<TaskScopeArg>,
    enable: Vec<String>,
    config: Option<PathBuf>,
    no_truncate: bool,
}

impl GlobalFlags {
    fn from_cli(cli: &Cli) -> Self {
        GlobalFlags {
            json: cli.json,
            no_color: cli.no_color,
            quiet: cli.quiet,
            unsafe_disable_redaction: cli.unsafe_disable_redaction,
            experimental: cli.experimental,
            task_scope: cli.task_scope,
            enable: cli.enable.clone(),
            config: cli.config.clone(),
            no_truncate: cli.no_truncate,
        }
    }
}

fn main() {
    let cli = Cli::parse();
    let global = GlobalFlags::from_cli(&cli);
    let result = match cli.cmd {
        Command::Inspect {
            input,
            format,
            target_tokens,
            mode,
            list_transforms,
        } => cmd_inspect(&global, input, format, target_tokens, mode, list_transforms),
        Command::Compress {
            input,
            output,
            format,
            target_tokens,
            mode,
            disable,
            dry_run,
            store_originals,
            retrieve_namespace,
        } => {
            if dry_run {
                cmd_inspect(&global, input, format, target_tokens, mode, false)
            } else {
                cmd_compress(
                    &global,
                    input,
                    output,
                    format,
                    target_tokens,
                    mode,
                    disable,
                    store_originals,
                    retrieve_namespace,
                )
            }
        }
        Command::Diff { raw, compressed } => cmd_diff(&global, raw, compressed),
        Command::Wrap {
            store_originals,
            retrieve_namespace,
            rtk,
            rtk_capture_raw,
            argv,
        } => cmd_wrap(
            &global,
            argv,
            store_originals,
            retrieve_namespace,
            rtk,
            rtk_capture_raw,
        ),
        Command::Benchmark { fixtures, format } => cmd_benchmark(&global, fixtures, format),
        Command::Init { agent, dry_run } => cmd_init(&global, agent, dry_run),
        Command::Uninit { agent } => cmd_uninit(&global, agent),
        Command::Doctor { agent } => cmd_doctor(&global, agent),
        Command::Retrieve {
            reference,
            retrieve_namespace,
        } => cmd_retrieve(&global, reference, retrieve_namespace),
        Command::Mcp {
            action: McpAction::Serve,
        } => mcp::serve(),
        Command::Stats {
            report_globs,
            csv,
            scope,
            window,
            ledger,
        } => cmd_stats(&global, report_globs, csv, scope, window, ledger),
        Command::Gain { scope, since, csv } => cmd_gain(&global, scope, since, csv),
        Command::Session { recent } => cmd_session(&global, recent),
        Command::Filters { action } => cmd_filters(&global, action),
        Command::OutputSavings {
            baseline,
            shaped,
            ratio,
        } => cmd_output_savings(&global, baseline, shaped, ratio),
        Command::Learn { apply } => cmd_learn(&global, apply),
    };
    match result {
        Ok(code) => std::process::exit(code),
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(err.exit_code());
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn overrides_for(
    global: &GlobalFlags,
    mode: Option<ModeArg>,
    target_tokens: Option<usize>,
    format: Option<FormatArg>,
    disable: Vec<String>,
    store_originals: bool,
    retrieve_namespace: Option<String>,
) -> CliOverrides {
    CliOverrides {
        mode,
        target_tokens,
        format,
        disable,
        json: global.json,
        no_color: global.no_color,
        quiet: global.quiet,
        unsafe_disable_redaction: global.unsafe_disable_redaction,
        experimental: global.experimental,
        task_scope: global.task_scope,
        enable: global.enable.clone(),
        store_originals,
        retrieve_namespace,
    }
}

fn validate_enable_requires_experimental(
    effective: &config::Effective,
) -> Result<(), TokenFoldError> {
    for id in &effective.enable {
        if let Some(entry) = tokenfold_core::modes::ALL_ENTRIES
            .iter()
            .find(|e| e.transform_id.as_str() == id)
            && entry.experimental
            && !effective.experimental
        {
            return Err(TokenFoldError::InvalidInput(format!(
                "{id} is experimental; also pass --experimental"
            )));
        }
    }
    Ok(())
}

fn build_policy(effective: &config::Effective) -> Result<CompressionPolicy, TokenFoldError> {
    let mut builder = CompressionPolicy::builder()
        .mode(effective.mode)
        .task_scope(effective.task_scope)
        .preserve_latest_user_message(effective.preserve_latest_user_message)
        .unsafe_disable_redaction(effective.unsafe_disable_redaction)
        .experimental(effective.experimental)
        .store_originals(effective.retrieval_store_originals)
        .retrieval_namespace(effective.retrieval_namespace.clone())
        .retrieval_ttl_seconds(effective.retrieval_ttl_seconds)
        .retrieval_backend(effective.retrieval_backend.clone())
        .retrieval_store_path(effective.retrieval_store_path.clone());
    if let Some(t) = effective.target_tokens {
        builder = builder.target_tokens(t);
    }
    for id in &effective.disabled {
        builder = builder.disable(id.clone());
    }
    for id in &effective.enable {
        builder = builder.enable(id.clone());
    }
    builder.build()
}

fn resolve_format(
    effective_format: Option<InputFormat>,
    bytes: &[u8],
    from_wrap: bool,
) -> InputFormat {
    effective_format.unwrap_or_else(|| format::detect_format(bytes, from_wrap))
}

fn write_payload(output: Option<&std::path::Path>, bytes: &[u8]) -> Result<(), TokenFoldError> {
    if let Some(path) = output {
        std::fs::write(path, bytes)?;
    } else {
        use std::io::Write;
        std::io::stdout().write_all(bytes)?;
    }
    Ok(())
}

fn print_human_report(
    report: &tokenfold_core::report::CompressionReport,
    target_tokens: Option<usize>,
    is_inspect: bool,
    no_color: bool,
    no_truncate: bool,
) {
    let colors = render::stderr_colors(no_color);
    eprintln!(
        "{}",
        render::render_verdict(report, target_tokens, is_inspect, &colors)
    );
    eprintln!();
    eprint!(
        "{}",
        render::render_transform_table(report, &colors, no_truncate)
    );
    eprintln!("{}", render::render_totals(report));
    let warnings = render::render_warnings(report, &colors);
    if !warnings.is_empty() {
        eprintln!();
        eprint!("{warnings}");
    }
}

fn default_estimator() -> Box<dyn TokenEstimator> {
    if let Ok(est) = TiktokenEstimator::o200k_base() {
        return Box::new(est);
    }
    Box::new(ByteHeuristicEstimator)
}

fn read_input(input: &Input, label: &str) -> Result<Vec<u8>, TokenFoldError> {
    input
        .read()
        .map_err(|e| TokenFoldError::InvalidInput(format!("failed to read {label}: {e}")))
}

fn cmd_inspect(
    global: &GlobalFlags,
    input: Input,
    format: Option<FormatArg>,
    target_tokens: Option<usize>,
    mode: Option<ModeArg>,
    list_transforms: bool,
) -> Result<i32, TokenFoldError> {
    if list_transforms {
        if global.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&render::render_transform_list_json()).unwrap()
            );
        } else {
            print!("{}", render::render_transform_list());
        }
        return Ok(0);
    }

    // `inspect` never stores originals: it's a dry-run preview, and per INTERFACES.md
    // `tokenfold_inspect` also defaults `store_originals` to false.
    let overrides = overrides_for(global, mode, target_tokens, format, Vec::new(), false, None);
    let resolved = config::resolve(&overrides, global.config.as_deref())?;
    validate_enable_requires_experimental(&resolved.effective)?;
    let policy = build_policy(&resolved.effective)?;

    let bytes = read_input(&input, "input")?;
    let resolved_format = resolve_format(resolved.effective.format, &bytes, false);
    let compression_input = CompressionInput {
        format: resolved_format,
        bytes,
    };

    let output = tokenfold_core::compress(compression_input, &policy)?;

    if resolved.effective.json {
        println!("{}", serde_json::to_string_pretty(&output.report).unwrap());
    } else if !resolved.effective.quiet {
        print_human_report(
            &output.report,
            resolved.effective.target_tokens,
            true,
            resolved.effective.no_color,
            global.no_truncate,
        );
    }
    Ok(0)
}

#[allow(clippy::too_many_arguments)]
fn cmd_compress(
    global: &GlobalFlags,
    input: Input,
    output_path: Option<PathBuf>,
    format: Option<FormatArg>,
    target_tokens: Option<usize>,
    mode: Option<ModeArg>,
    disable: Vec<String>,
    store_originals: bool,
    retrieve_namespace: Option<String>,
) -> Result<i32, TokenFoldError> {
    let overrides = overrides_for(
        global,
        mode,
        target_tokens,
        format,
        disable,
        store_originals,
        retrieve_namespace,
    );
    let resolved = config::resolve(&overrides, global.config.as_deref())?;
    validate_enable_requires_experimental(&resolved.effective)?;
    let policy = build_policy(&resolved.effective)?;

    let bytes = read_input(&input, "input")?;
    let resolved_format = resolve_format(resolved.effective.format, &bytes, false);
    let compression_input = CompressionInput {
        format: resolved_format,
        bytes,
    };

    let output = tokenfold_core::compress(compression_input, &policy)?;

    write_payload(output_path.as_deref(), &output.bytes)?;

    // F-046: record redacted ledger metadata for this run, best-effort (see `record_to_ledger`).
    let input_path = match &input {
        Input::Path(p) => Some(p.as_path()),
        Input::Stdin => None,
    };
    record_to_ledger(&resolved.effective, &output.report, input_path, "stdin");

    if resolved.effective.json {
        eprintln!("{}", serde_json::to_string_pretty(&output.report).unwrap());
    } else if !resolved.effective.quiet {
        print_human_report(
            &output.report,
            resolved.effective.target_tokens,
            false,
            resolved.effective.no_color,
            global.no_truncate,
        );
    }
    Ok(0)
}

fn cmd_diff(global: &GlobalFlags, raw: Input, compressed: Input) -> Result<i32, TokenFoldError> {
    let raw_bytes = read_input(&raw, "raw input")?;
    let compressed_bytes = read_input(&compressed, "compressed input")?;

    let estimator = default_estimator();
    let raw_tokens = estimator.count_bytes(&raw_bytes);
    let compressed_tokens = estimator.count_bytes(&compressed_bytes);
    let info = estimator.info();
    let savings_pct = if raw_tokens == 0 {
        0.0
    } else {
        raw_tokens.saturating_sub(compressed_tokens) as f64 / raw_tokens as f64 * 100.0
    };

    let raw_text = String::from_utf8_lossy(&raw_bytes);
    let compressed_text = String::from_utf8_lossy(&compressed_bytes);
    let lines = diff::diff_lines(&raw_text, &compressed_text);

    if global.json {
        let payload = serde_json::json!({
            "raw_tokens": raw_tokens,
            "compressed_tokens": compressed_tokens,
            "saved_tokens": raw_tokens.saturating_sub(compressed_tokens),
            "savings_pct": savings_pct,
            "estimator": { "backend": info.backend, "model": info.model, "is_exact": info.is_exact },
            "hunks": diff::to_json(&lines),
        });
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
    } else {
        let colors = render::stdout_colors(global.no_color);
        println!(
            "{}",
            diff::render_header(raw_tokens, compressed_tokens, savings_pct, info.is_exact)
        );
        print!("{}", diff::render_body(&lines, &colors));
    }
    Ok(0)
}

fn cmd_output_savings(
    global: &GlobalFlags,
    baseline: Input,
    shaped: Option<Input>,
    ratio: Option<f64>,
) -> Result<i32, TokenFoldError> {
    let baseline_bytes = read_input(&baseline, "baseline output")?;
    let estimator = default_estimator();

    let report = if let Some(shaped_input) = shaped {
        let shaped_bytes = read_input(&shaped_input, "shaped output")?;
        tokenfold_output::measure_output_savings(&baseline_bytes, &shaped_bytes, &*estimator)
    } else if let Some(ratio) = ratio {
        let hint = estimator.count_bytes(&baseline_bytes);
        tokenfold_output::estimate_output_savings(ratio, hint)
    } else {
        return Err(TokenFoldError::InvalidInput(
            "output-savings requires either a SHAPED file argument or --ratio <f64>".to_string(),
        ));
    };

    if global.json {
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
    } else {
        println!("profile: {}", report.profile);
        if let Some(n) = report.measured_output_tokens_saved {
            println!("measured_output_tokens_saved: {n}");
        }
        if let Some(n) = report.estimated_output_tokens_saved {
            println!("estimated_output_tokens_saved: {n}");
        }
        println!("provenance: {}", report.provenance);
    }
    Ok(0)
}

fn cmd_learn(global: &GlobalFlags, apply: bool) -> Result<i32, TokenFoldError> {
    let overrides = overrides_for(global, None, None, None, Vec::new(), false, None);
    let resolved = config::resolve(&overrides, global.config.as_deref())?;

    let records = if resolved.effective.analytics_enabled {
        let store =
            tokenfold_core::stats::LedgerStore::new(resolved.effective.analytics_ledger_path);
        store.read_all()?
    } else {
        Vec::new()
    };

    let proposals = tokenfold_learn::propose_policy_changes(&records);

    if global.json {
        let payload: Vec<_> = proposals
            .iter()
            .map(|p| {
                serde_json::json!({
                    "scope": p.scope,
                    "observation": p.observation,
                    "suggestion": p.suggestion,
                    "confidence": p.confidence,
                    "sample_size": p.sample_size,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
    } else if proposals.is_empty() {
        println!(
            "No policy changes proposed (not enough ledger history yet, or current settings already perform well)."
        );
    } else {
        for p in &proposals {
            println!("[{}] {}", p.scope, p.observation);
            println!(
                "  suggestion: {} (confidence {:.0}%, n={})",
                p.suggestion,
                p.confidence * 100.0,
                p.sample_size
            );
        }
        if !apply {
            println!(
                "\nNo changes written. Re-run with --apply to write the first proposal into tokenfold.toml."
            );
        }
    }

    if apply {
        match proposals.first() {
            Some(proposal) if proposal.scope == "conservative" => {
                apply_mode_change(global, resolved.config_path.as_deref(), "balanced")?;
                if !global.json {
                    println!(
                        "Applied: [compression] mode = \"balanced\" written to tokenfold.toml"
                    );
                }
            }
            _ => {
                if !global.json {
                    println!("Nothing to apply.");
                }
            }
        }
    }

    Ok(0)
}

/// Writes `[compression].mode = mode` into the resolved config file (or a new `tokenfold.toml`
/// in the current directory if none exists yet), preserving every other key already there.
/// Round-trips through `toml::Value` rather than a comment-preserving editor -- this is a
/// single local config file, not a shared document, so losing comments on a `--apply` write is
/// an acceptable trade for not adding a TOML-editing dependency.
fn apply_mode_change(
    global: &GlobalFlags,
    config_path: Option<&Path>,
    mode: &str,
) -> Result<(), TokenFoldError> {
    let target = config_path
        .map(Path::to_path_buf)
        .or_else(|| global.config.clone())
        .unwrap_or_else(|| PathBuf::from("tokenfold.toml"));

    let mut doc: toml::Value = if target.exists() {
        let text = std::fs::read_to_string(&target)?;
        text.parse().map_err(|e| {
            TokenFoldError::ConfigError(format!("{} is not valid TOML: {e}", target.display()))
        })?
    } else {
        toml::Value::Table(toml::map::Map::new())
    };

    let table = doc.as_table_mut().ok_or_else(|| {
        TokenFoldError::ConfigError(format!(
            "{} is not a TOML table at its root",
            target.display()
        ))
    })?;
    let compression = table
        .entry("compression")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    let compression_table = compression
        .as_table_mut()
        .ok_or_else(|| TokenFoldError::ConfigError("[compression] is not a table".to_string()))?;
    compression_table.insert("mode".to_string(), toml::Value::String(mode.to_string()));

    let rendered = toml::to_string_pretty(&doc).map_err(|e| {
        TokenFoldError::InternalError(format!("failed to serialize tokenfold.toml: {e}"))
    })?;
    std::fs::write(&target, rendered)?;
    Ok(())
}

/// Builds a token estimator matching `info`, so per-stage token counts use the same backend as
/// `compress()` (a precondition for reporting `total_saved_tokens`).
fn estimator_for(info: &EstimatorInfo) -> Box<dyn TokenEstimator> {
    if info.backend != "tiktoken" {
        return Box::new(ByteHeuristicEstimator);
    }
    match TiktokenEstimator::o200k_base() {
        Ok(e) => Box::new(e),
        Err(_) => Box::new(ByteHeuristicEstimator),
    }
}

/// `(combined_output, exit_code, duration_ms, stdout_bytes, stderr_bytes)` from a wrapped child.
type ChildRun = (Vec<u8>, Option<i32>, u64, usize, usize);

/// Runs a child command directly (the tokenfold-only path).
fn run_child(program: &str, args: &[String]) -> Result<ChildRun, TokenFoldError> {
    let start = std::time::Instant::now();
    let child_output = std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|e| TokenFoldError::InvalidInput(format!("failed to launch `{program}`: {e}")))?;
    let duration_ms = start.elapsed().as_millis() as u64;
    // ponytail: combines stdout+stderr by concatenation (stdout then stderr), not true
    // chronological interleaving. Good enough for compression; upgrade to a real merged pipe
    // (or add `--passthrough-stderr` to skip the merge) if interleaving order ever matters.
    let mut raw = child_output.stdout.clone();
    raw.extend_from_slice(&child_output.stderr);
    Ok((
        raw,
        child_output.status.code(),
        duration_ms,
        child_output.stdout.len(),
        child_output.stderr.len(),
    ))
}

/// Inputs to [`build_pipeline_report`]. Grouped into a struct because there are too many for a
/// readable positional signature.
struct BuildPipeline<'a> {
    rtk_ran: bool,
    rtk_version: Option<&'a str>,
    rtk_duration_ms: Option<f64>,
    rtk_stage_status: &'a str,
    rtk_bypass_reason: Option<&'a str>,
    rtk_capture_raw: bool,
    raw_capture_completeness: Option<&'a str>,
    raw_input_bytes: Option<usize>,
    raw_input_tokens: Option<usize>,
    raw_stored: bool,
    rtk_evidence_ref: Option<String>,
    post_rtk_bytes: usize,
    final_output_bytes: usize,
    store_originals: bool,
    compress_ms: f64,
    report: &'a tokenfold_core::report::CompressionReport,
}

/// F-055: assemble the staged `raw -> RTK -> tokenfold` receipt. RTK's savings live only in the
/// RTK stage and in `total_saved_tokens`; they are never folded into the top-level
/// `saved_tokens`, which stays scoped to tokenfold_core.
fn build_pipeline_report(p: BuildPipeline) -> PipelineReport {
    let report = p.report;

    // --- RTK stage ---
    // Post-RTK bytes/tokens are what enters compress(); tokens == report.original_tokens.
    let (rtk_output_bytes, rtk_output_tokens) = if p.rtk_ran {
        (Some(p.post_rtk_bytes), Some(report.original_tokens))
    } else {
        (None, None)
    };
    let rtk_saved_bytes = match (p.raw_input_bytes, rtk_output_bytes) {
        (Some(i), Some(o)) => Some(i.saturating_sub(o)),
        _ => None,
    };
    let rtk_saved_tokens = match (p.raw_input_tokens, rtk_output_tokens) {
        (Some(i), Some(o)) => Some(i.saturating_sub(o)),
        _ => None,
    };
    let rtk_recoverability = if p.raw_stored {
        "full"
    } else if p.rtk_ran && p.rtk_capture_raw {
        "none"
    } else {
        "not_applicable"
    };
    let rtk_stage = PipelineStageReport {
        id: "rtk".to_string(),
        version: p.rtk_version.map(str::to_string),
        input_bytes: p.raw_input_bytes,
        output_bytes: rtk_output_bytes,
        saved_bytes: rtk_saved_bytes,
        input_tokens: p.raw_input_tokens,
        output_tokens: rtk_output_tokens,
        saved_tokens: rtk_saved_tokens,
        estimator: p.raw_input_tokens.map(|_| report.estimator.clone()),
        status: p.rtk_stage_status.to_string(),
        duration_ms: p.rtk_duration_ms,
        bypass_reason: p.rtk_bypass_reason.map(str::to_string),
        provenance: match p.rtk_version {
            Some(v) => format!("external:rtk@{v}"),
            None => "external:rtk".to_string(),
        },
        recoverability: rtk_recoverability.to_string(),
        evidence_ref: p.rtk_evidence_ref,
    };

    // --- tokenfold stage ---
    let tf_status = if report.compressed_tokens < report.original_tokens {
        "applied"
    } else {
        "passthrough"
    };
    let tf_stage = PipelineStageReport {
        id: "tokenfold".to_string(),
        version: Some(env!("CARGO_PKG_VERSION").to_string()),
        input_bytes: Some(p.post_rtk_bytes),
        output_bytes: Some(p.final_output_bytes),
        saved_bytes: Some(p.post_rtk_bytes.saturating_sub(p.final_output_bytes)),
        input_tokens: Some(report.original_tokens),
        output_tokens: Some(report.compressed_tokens),
        saved_tokens: Some(report.saved_tokens),
        estimator: Some(report.estimator.clone()),
        status: tf_status.to_string(),
        duration_ms: Some(p.compress_ms),
        bypass_reason: None,
        provenance: "tokenfold_core".to_string(),
        recoverability: if p.store_originals { "full" } else { "none" }.to_string(),
        evidence_ref: None,
    };

    let raw_capture = if !p.rtk_ran || !p.rtk_capture_raw {
        "not_applicable"
    } else {
        p.raw_capture_completeness.unwrap_or("unavailable")
    };
    let upstream_recoverability = if !p.rtk_ran {
        "not_applicable"
    } else if p.raw_stored {
        "full"
    } else if p.store_originals {
        "tokenfold_only"
    } else {
        "none"
    };

    PipelineReport {
        raw_input_bytes: p.raw_input_bytes,
        raw_input_tokens: p.raw_input_tokens,
        final_output_bytes: p.final_output_bytes,
        final_output_tokens: report.compressed_tokens,
        // Same estimator across raw and final (estimator_for mirrors report.estimator).
        total_saved_tokens: p
            .raw_input_tokens
            .map(|r| r.saturating_sub(report.compressed_tokens)),
        raw_capture: raw_capture.to_string(),
        upstream_recoverability: upstream_recoverability.to_string(),
        stages: vec![rtk_stage, tf_stage],
    }
}

#[allow(clippy::too_many_arguments)]
fn cmd_wrap(
    global: &GlobalFlags,
    argv: Vec<String>,
    store_originals: bool,
    retrieve_namespace: Option<String>,
    use_rtk: bool,
    rtk_capture_raw: bool,
) -> Result<i32, TokenFoldError> {
    let Some((program, args)) = argv.split_first() else {
        return Err(TokenFoldError::InvalidInput(
            "wrap requires a command after `--`, e.g. `tokenfold wrap -- git diff`".to_string(),
        ));
    };

    let overrides = overrides_for(
        global,
        None,
        None,
        None,
        Vec::new(),
        store_originals,
        retrieve_namespace,
    );
    let resolved = config::resolve(&overrides, global.config.as_deref())?;
    validate_enable_requires_experimental(&resolved.effective)?;
    let policy = build_policy(&resolved.effective)?;

    // --- Stage 1: acquire command output, RTK-composed or direct. ---
    // F-054: RTK preflight runs *before* the child. If RTK is missing/incompatible we fail open
    // to the tokenfold-only path here, before any side-effectful command has started. Once RTK
    // (or the child) has been spawned we never rerun it — its output and exit code are final.
    let mut rtk_ran = false;
    let mut rtk_version: Option<String> = None;
    let mut rtk_duration_ms: Option<f64> = None;
    let mut rtk_raw_capture: Option<rtk::RawCapture> = None;
    let mut rtk_stage_status = "not_applicable".to_string();
    let mut rtk_bypass_reason: Option<String> = None;

    let (raw, child_exit_code, duration_ms, stdout_bytes, stderr_bytes) = if use_rtk {
        match rtk::preflight() {
            rtk::Preflight::Ready { path, version } => {
                let run = rtk::run(&path, &version, &argv, rtk_capture_raw)?;
                rtk_ran = true;
                rtk_version = Some(run.version.clone());
                rtk_duration_ms = Some(run.duration_ms);
                rtk_raw_capture = run.raw_capture;
                rtk_stage_status = run.stage_status.clone();
                let n = run.output.len();
                (run.output, run.exit_code, run.duration_ms as u64, n, 0)
            }
            rtk::Preflight::FailOpen { status, note } => {
                rtk_stage_status = status;
                rtk_bypass_reason = Some(note);
                run_child(program, args)?
            }
        }
    } else {
        run_child(program, args)?
    };

    // F-047: check for a trusted filter pack matching the invoked argv *before* the generic
    // compress() pipeline runs. Composition choice (see ROADMAP.md F-047, "runs before or
    // alongside generic log_compaction"): the filter stage-pipeline runs first, its own
    // never_worse guard ensures it never hands compress() anything worse than the true raw
    // bytes, and its (possibly reduced) output is simply what compress() then sees as its
    // input — no special bypass path in `pipeline.rs`. One side effect of this choice:
    // `CompressionReport.original_tokens`/`saved_tokens` reflect the post-filter input to
    // compress(), not the true pre-filter raw size — `CommandReport.raw_output_bytes` below
    // still reports the true raw byte count for that visibility.
    //
    // F-054 double-filtering avoidance: when RTK ran, it already owns command-specific
    // filtering, so tokenfold's overlapping filter pack is skipped. Mandatory redaction,
    // generic compression, safety rollback, and the compress-level never_worse guard still run
    // (they live inside compress()).
    let (pipeline_input, filter_pack_id, filter_version, filter_never_worse_reverted) = if rtk_ran {
        (raw.clone(), None, None, false)
    } else {
        let filter_match = tokenfold_core::filters::resolve_matching_filter(
            &tokenfold_core::filters::FilterLookup {
                argv: &argv,
                raw_output: &raw,
                enabled: resolved.effective.filters_enabled,
                project_filters_path: Some(&resolved.effective.filters_project_filters_path),
                user_filters_path: Some(&resolved.effective.filters_user_filters_path),
                trust_store_path: &resolved.effective.filters_trust_store_path,
                trust_project_filters: resolved.effective.filters_trust_project_filters,
            },
        );
        match &filter_match {
            Some(matched) => {
                let filtered = matched.filter.apply(&raw)?;
                let guarded = tokenfold_core::filters::never_worse(&raw, &filtered);
                (
                    guarded.bytes,
                    Some(matched.pack_id.clone()),
                    Some(matched.filter.version.clone()),
                    !guarded.used_filtered,
                )
            }
            None => (raw.clone(), None, None, false),
        }
    };

    let resolved_format = resolve_format(resolved.effective.format, &pipeline_input, true);
    let compression_input = CompressionInput {
        format: resolved_format,
        bytes: pipeline_input.clone(),
    };
    let compress_start = std::time::Instant::now();
    let mut output = tokenfold_core::compress(compression_input, &policy)?;
    let compress_ms = compress_start.elapsed().as_secs_f64() * 1000.0;

    let compress_never_worse = output.report.compressed_tokens > output.report.original_tokens;
    if compress_never_worse {
        output.bytes = pipeline_input.clone();
    }
    let never_worse_applied = filter_never_worse_reverted || compress_never_worse;

    // --- Stage 2 accounting: CCR raw-capture ingestion (F-055). ---
    // The transient tee file was already deleted by rtk::run's RAII guard once its bytes were
    // read into memory; here we only persist those in-memory bytes through the normal
    // redaction/persistence gate. `RetrievalStore::store` refuses secret-matched bytes, so a
    // secret capture is never persisted.
    let mut raw_input_bytes: Option<usize> = None;
    let mut raw_input_tokens: Option<usize> = None;
    let mut rtk_evidence_ref: Option<String> = None;
    let mut raw_stored = false;
    if let Some(cap) = &rtk_raw_capture {
        if cap.completeness == "complete" {
            let est = estimator_for(&output.report.estimator);
            raw_input_bytes = Some(cap.bytes.len());
            raw_input_tokens = Some(est.count_bytes(&cap.bytes));
            let store = tokenfold_core::retrieval_store::RetrievalStore::open(
                &resolved.effective.retrieval_backend,
                "sha256",
                resolved.effective.retrieval_store_path.clone(),
            )?;
            match store.store(
                &cap.bytes,
                &resolved.effective.retrieval_namespace,
                resolved.effective.retrieval_ttl_seconds,
            ) {
                Ok(marker) => {
                    raw_stored = true;
                    rtk_evidence_ref = Some(marker.hash);
                }
                Err(TokenFoldError::SafetyViolation(_)) => {
                    // Never persisted; the receipt labels the pre-RTK stage unrecoverable.
                    rtk_bypass_reason = Some(
                        "pre-RTK raw capture matched a secret pattern and was not persisted"
                            .to_string(),
                    );
                }
                Err(e) => {
                    rtk_bypass_reason = Some(format!("failed to persist pre-RTK raw capture: {e}"));
                }
            }
        } else {
            rtk_bypass_reason = Some(format!(
                "pre-RTK raw capture is {}; upstream detail is unrecoverable",
                cap.completeness
            ));
        }
    }

    if use_rtk {
        let final_output_bytes = output.bytes.len();
        let pipeline = build_pipeline_report(BuildPipeline {
            rtk_ran,
            rtk_version: rtk_version.as_deref(),
            rtk_duration_ms,
            rtk_stage_status: &rtk_stage_status,
            rtk_bypass_reason: rtk_bypass_reason.as_deref(),
            rtk_capture_raw,
            raw_capture_completeness: rtk_raw_capture.as_ref().map(|c| c.completeness.as_str()),
            raw_input_bytes,
            raw_input_tokens,
            raw_stored,
            rtk_evidence_ref,
            post_rtk_bytes: raw.len(),
            final_output_bytes,
            store_originals,
            compress_ms,
            report: &output.report,
        });
        output.report.pipeline = Some(pipeline);
    }

    output.report.command = Some(CommandReport {
        command_family: None,
        child_exit_code,
        duration_ms,
        raw_output_bytes: raw.len(),
        stdout_bytes,
        stderr_bytes,
        stderr_mode: "captured".to_string(),
        stderr_truncated: false,
        compressed_output_bytes: output.bytes.len(),
        filter_pack_id,
        filter_version,
        never_worse_applied,
        bypass_reason: None,
    });

    write_payload(None, &output.bytes)?;

    // F-046: record redacted ledger metadata for this run, best-effort (see `record_to_ledger`).
    // Wrapped commands have no file-path attribution to hash, hence the "wrap" placeholder.
    record_to_ledger(&resolved.effective, &output.report, None, "wrap");

    if resolved.effective.json {
        eprintln!("{}", serde_json::to_string_pretty(&output.report).unwrap());
    } else if !resolved.effective.quiet {
        print_human_report(
            &output.report,
            resolved.effective.target_tokens,
            false,
            resolved.effective.no_color,
            global.no_truncate,
        );
    }

    Ok(child_exit_code.unwrap_or(1))
}

fn cmd_benchmark(
    global: &GlobalFlags,
    fixtures: Vec<PathBuf>,
    format: Option<FormatArg>,
) -> Result<i32, TokenFoldError> {
    if fixtures.is_empty() {
        return Err(TokenFoldError::InvalidInput(
            "benchmark requires at least one fixture path".to_string(),
        ));
    }
    let policy = CompressionPolicy::builder().build()?;

    let mut rows = Vec::new();
    for path in &fixtures {
        let bytes = std::fs::read(path)?;
        let resolved_format = format
            .map(FormatArg::to_input_format)
            .unwrap_or_else(|| format::detect_format(&bytes, false));
        let start = std::time::Instant::now();
        let output = tokenfold_core::compress(
            CompressionInput {
                format: resolved_format,
                bytes,
            },
            &policy,
        )?;
        rows.push((path.clone(), output.report, start.elapsed()));
    }

    if global.json {
        let arr: Vec<_> = rows
            .iter()
            .map(|(path, report, elapsed)| {
                serde_json::json!({
                    "fixture": path.display().to_string(),
                    "report": report,
                    "elapsed_micros": elapsed.as_micros() as u64,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&arr).unwrap());
    } else {
        println!(
            "{:<32} {:>10} {:>10} {:>9} {:>8} {:>10}",
            "FIXTURE", "BEFORE", "AFTER", "SAVED", "%", "ELAPSED"
        );
        for (path, report, elapsed) in &rows {
            println!(
                "{:<32} {:>10} {:>10} {:>9} {:>7.1}% {:>10?}",
                path.display(),
                report.original_tokens,
                report.compressed_tokens,
                report.saved_tokens,
                report.savings_pct,
                elapsed
            );
        }
    }
    Ok(0)
}

// ponytail: no v0.1 agent host has been chosen yet (roadmap.md D-002/D-004 leave the "first
// supported agent host" undecided), so every `--agent` value is honestly reported as
// unsupported rather than pretending to patch a host config that doesn't exist. Add real
// host integrations here once a first host is picked.
fn cmd_init(global: &GlobalFlags, agent: String, dry_run: bool) -> Result<i32, TokenFoldError> {
    let message = format!(
        "agent '{agent}' is not a supported host yet (v0.1 has not shipped a host integration)"
    );
    if global.json {
        println!(
            "{}",
            serde_json::json!({ "agent": agent, "supported": false, "dry_run": dry_run, "message": message })
        );
    } else {
        eprintln!("{message}");
    }
    Ok(2)
}

fn cmd_uninit(global: &GlobalFlags, agent: String) -> Result<i32, TokenFoldError> {
    let message = format!("agent '{agent}' is not a supported host yet; nothing to remove");
    if global.json {
        println!(
            "{}",
            serde_json::json!({ "agent": agent, "supported": false, "message": message })
        );
    } else {
        eprintln!("{message}");
    }
    Ok(2)
}

fn cmd_doctor(global: &GlobalFlags, agent: Option<String>) -> Result<i32, TokenFoldError> {
    let tiktoken_available = TiktokenEstimator::o200k_base().is_ok();
    let (config_path, config_error) =
        match config::resolve(&CliOverrides::default(), global.config.as_deref()) {
            Ok(r) => (r.config_path, None),
            Err(e) => (None, Some(e.to_string())),
        };

    // F-054: additive RTK health. RTK is optional, so `missing` is a warning, never a failure.
    let rtk = rtk::doctor_probe();

    if global.json {
        println!(
            "{}",
            serde_json::json!({
                "estimator": { "tiktoken_available": tiktoken_available },
                "config_path": config_path.as_ref().map(|p| p.display().to_string()),
                "config_error": config_error,
                "agent": agent.as_ref().map(|a| serde_json::json!({ "name": a, "supported": false })),
                "rtk": {
                    "status": rtk.status,
                    "path": rtk.path,
                    "version": rtk.version,
                    "compatible": rtk.compatible,
                    "raw_capture": rtk.raw_capture,
                    "notes": rtk.notes,
                },
            })
        );
    } else {
        println!("tokenfold doctor");
        println!(
            "  estimator: tiktoken {}",
            if tiktoken_available {
                "OK"
            } else {
                "UNAVAILABLE (falling back to heuristic)"
            }
        );
        match &config_path {
            Some(p) => println!("  config: {}", p.display()),
            None => println!("  config: none (using built-in defaults)"),
        }
        if let Some(e) = &config_error {
            println!("  config error: {e}");
        }
        if let Some(a) = &agent {
            println!("  agent '{a}': not supported yet (no v0.1 host integration has shipped)");
        }
        match rtk.version {
            Some(v) => println!("  rtk: {} ({})", rtk.status, v),
            None => println!("  rtk: {} (optional)", rtk.status),
        }
        println!("  rtk raw-capture: {}", rtk.raw_capture);
        for note in &rtk.notes {
            println!("    - {note}");
        }
    }
    Ok(if config_error.is_some() { 5 } else { 0 })
}

/// F-045: `tokenfold retrieve <hash-or-marker-or-report-path>`.
fn cmd_retrieve(
    global: &GlobalFlags,
    reference: String,
    namespace_flag: Option<String>,
) -> Result<i32, TokenFoldError> {
    let overrides = overrides_for(global, None, None, None, Vec::new(), false, None);
    let resolved = config::resolve(&overrides, global.config.as_deref())?;

    // A path to an existing `CompressionReport` JSON file: this pass's `RetrievalReport`
    // shape has no per-entry content hash, so there is nothing to recover from a report alone
    // (see `report.rs::RetrievalReport`) — say so clearly instead of guessing.
    let path = std::path::Path::new(&reference);
    if path.is_file() {
        let bytes = std::fs::read(path)?;
        return if serde_json::from_slice::<tokenfold_core::report::CompressionReport>(&bytes)
            .is_ok()
        {
            eprintln!(
                "{} is a CompressionReport, but this report has no storable hash in the \
                 current schema (RetrievalReport does not carry a per-entry content hash yet); \
                 retrieve by the original hash or `[tokenfold:retrieve ...]` marker instead",
                path.display()
            );
            Ok(1)
        } else {
            Err(TokenFoldError::InvalidInput(format!(
                "{} is not a valid CompressionReport JSON file",
                path.display()
            )))
        };
    }

    let (hash, marker_namespace) = parse_retrieve_reference(&reference)?;
    let namespace = namespace_flag
        .or(marker_namespace)
        .unwrap_or(resolved.effective.retrieval_namespace);

    let store = tokenfold_core::retrieval_store::RetrievalStore::open(
        &resolved.effective.retrieval_backend,
        "sha256",
        resolved.effective.retrieval_store_path.clone(),
    )?;

    match store.retrieve(&hash, &namespace) {
        tokenfold_core::retrieval_store::RetrievalOutcome::Found(bytes) => {
            use std::io::Write;
            std::io::stdout().write_all(&bytes)?;
            Ok(0)
        }
        tokenfold_core::retrieval_store::RetrievalOutcome::Missing => {
            eprintln!("no stored original found for hash {hash} in namespace {namespace:?}");
            Ok(1)
        }
        tokenfold_core::retrieval_store::RetrievalOutcome::Expired => {
            eprintln!("stored original for hash {hash} in namespace {namespace:?} has expired");
            Ok(1)
        }
    }
}

/// Accepts a raw hex SHA-256 hash or a `[tokenfold:retrieve hash=<hex> ... namespace=<ns> ...]`
/// marker, returning the hash and (when the input was a marker carrying one) its namespace.
fn parse_retrieve_reference(reference: &str) -> Result<(String, Option<String>), TokenFoldError> {
    if reference.contains("tokenfold:retrieve") {
        let hash = extract_marker_field(reference, "hash").ok_or_else(|| {
            TokenFoldError::InvalidInput("retrieval marker has no hash=<hex> field".to_string())
        })?;
        let namespace = extract_marker_field(reference, "namespace");
        return Ok((hash, namespace));
    }
    if reference.is_empty() || !reference.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(TokenFoldError::InvalidInput(format!(
            "{reference:?} is not a valid sha256 hex hash, retrieval marker, or existing report file path"
        )));
    }
    Ok((reference.to_ascii_lowercase(), None))
}

fn extract_marker_field(marker: &str, field: &str) -> Option<String> {
    let needle = format!("{field}=");
    let start = marker.find(&needle)? + needle.len();
    let rest = &marker[start..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == ']')
        .unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

/// F-046: appends redacted ledger metadata for one successful compress/wrap run when
/// `[analytics].enabled` is true. Best-effort — a ledger write failure must never fail the
/// command it's recording, so errors are silently dropped here (the compression itself already
/// succeeded by the time this is called).
fn record_to_ledger(
    effective: &config::Effective,
    report: &tokenfold_core::report::CompressionReport,
    project_source: Option<&std::path::Path>,
    placeholder: &str,
) {
    if !effective.analytics_enabled {
        return;
    }
    let project_hash = Some(project_attribution(
        project_source,
        effective.analytics_hash_project_paths,
        placeholder,
    ));
    let request_id = tokenfold_core::stats::generate_request_id();
    let timestamp = tokenfold_core::stats::format_unix_timestamp(tokenfold_core::stats::now_unix());
    let record =
        tokenfold_core::stats::record_from_report(report, request_id, timestamp, project_hash);
    let store = tokenfold_core::stats::LedgerStore::new(effective.analytics_ledger_path.clone());
    if store.append(&record).is_ok() {
        // Opportunistic retention cleanup: cheap at local-CLI ledger sizes, and means
        // `[analytics].retention_days` takes effect without a separate maintenance command.
        let _ = store.gc(effective.analytics_retention_days);
    }
}

/// Hashes a project/path attribution rather than storing it raw when `hash_project_paths` is
/// true (the default). There is no project-name flag anywhere else in this CLI yet, so the only
/// thing to attribute is the input file path (`compress <path>`), or a stable placeholder when
/// there is none (`stdin` for `compress -`, `wrap` for wrapped commands).
fn project_attribution(
    path: Option<&std::path::Path>,
    hash_project_paths: bool,
    placeholder: &str,
) -> String {
    match path {
        Some(p) => {
            let raw = p.to_string_lossy().to_string();
            if hash_project_paths {
                format!(
                    "sha256:{}",
                    tokenfold_core::retrieval_store::hex_sha256(raw.as_bytes())
                )
            } else {
                raw
            }
        }
        None => placeholder.to_string(),
    }
}

/// F-046: `tokenfold stats [report-glob...] [--json|--csv] [--scope] [--window]`. Aggregates
/// ad-hoc `CompressionReport` JSON files matched by `report_globs` plus the local ledger (when
/// `[analytics].enabled`), through the one shared `tokenfold_core::stats::aggregate` path.
fn cmd_stats(
    global: &GlobalFlags,
    report_globs: Vec<String>,
    csv: bool,
    scope: Option<String>,
    window: Option<String>,
    ledger_override: Option<PathBuf>,
) -> Result<i32, TokenFoldError> {
    let overrides = overrides_for(global, None, None, None, Vec::new(), false, None);
    let resolved = config::resolve(&overrides, global.config.as_deref())?;

    let mut records = Vec::new();
    let mut retrieval_markers = 0usize;
    for pattern in &report_globs {
        for path in stats_cmd::expand_glob(pattern)? {
            let (record, markers) = stats_cmd::record_from_report_file(&path)?;
            retrieval_markers += markers;
            records.push(record);
        }
    }

    if resolved.effective.analytics_enabled {
        let ledger_path = ledger_override
            .clone()
            .unwrap_or(resolved.effective.analytics_ledger_path.clone());
        let store = tokenfold_core::stats::LedgerStore::new(ledger_path);
        records.extend(store.read_all()?);
    }

    let mut summary = tokenfold_core::stats::aggregate(&records);
    summary.scope = scope.unwrap_or_else(|| "project".to_string());
    summary.window = window.unwrap_or_else(|| "all".to_string());
    // ponytail: real per-request retrieval hit/miss/expiry data doesn't exist yet (see
    // `tokenfold_core::stats` module doc) — only the store-time marker count, summed here
    // straight from each ad-hoc report file's own `CompressionReport.retrieval`.
    summary.retrieval.markers = retrieval_markers;

    stats_cmd::print_summary(&summary, global.json, csv);
    Ok(0)
}

/// F-046: `tokenfold gain [--scope project|user] [--since 30d] [--json|--csv]`. Summarizes
/// realized token savings from the local ledger over a recency window.
fn cmd_gain(
    global: &GlobalFlags,
    scope: Option<String>,
    since: Option<String>,
    csv: bool,
) -> Result<i32, TokenFoldError> {
    let overrides = overrides_for(global, None, None, None, Vec::new(), false, None);
    let resolved = config::resolve(&overrides, global.config.as_deref())?;

    let since_arg = since.unwrap_or_else(|| "30d".to_string());
    let window_secs = tokenfold_core::stats::parse_duration_secs(&since_arg)?;

    let records = if resolved.effective.analytics_enabled {
        let store =
            tokenfold_core::stats::LedgerStore::new(resolved.effective.analytics_ledger_path);
        let all = store.read_all()?;
        tokenfold_core::stats::filter_since(&all, tokenfold_core::stats::now_unix(), window_secs)
    } else {
        Vec::new()
    };

    let mut summary = tokenfold_core::stats::aggregate(&records);
    // `scope` is a framing label only: there is no multi-project/user attribution registry to
    // filter by yet, only per-record project hashes (see module doc).
    summary.scope = scope.unwrap_or_else(|| "project".to_string());
    summary.window = since_arg;

    stats_cmd::print_summary(&summary, global.json, csv);
    Ok(0)
}

/// F-046: `tokenfold session [--recent N] [--json]`. Host-session command-wrapping coverage:
/// total/wrapped/raw commands, bypasses, and `coverage_pct`.
fn cmd_session(global: &GlobalFlags, recent: Option<usize>) -> Result<i32, TokenFoldError> {
    let overrides = overrides_for(global, None, None, None, Vec::new(), false, None);
    let resolved = config::resolve(&overrides, global.config.as_deref())?;

    let records = if resolved.effective.analytics_enabled {
        let store =
            tokenfold_core::stats::LedgerStore::new(resolved.effective.analytics_ledger_path);
        store.read_all()?
    } else {
        Vec::new()
    };

    let mut summary = tokenfold_core::stats::aggregate(&records);
    summary.scope = "session".to_string();
    summary.window = "all".to_string();
    if let Some(n) = recent {
        summary.recent_requests.truncate(n);
    }

    stats_cmd::print_summary(&summary, global.json, false);
    Ok(0)
}

/// F-047: `tokenfold filters list|verify|trust`.
fn cmd_filters(global: &GlobalFlags, action: FiltersAction) -> Result<i32, TokenFoldError> {
    let overrides = overrides_for(global, None, None, None, Vec::new(), false, None);
    let resolved = config::resolve(&overrides, global.config.as_deref())?;
    match action {
        FiltersAction::List => cmd_filters_list(global, &resolved.effective),
        FiltersAction::Verify { require_all } => {
            cmd_filters_verify(global, &resolved.effective, require_all)
        }
        FiltersAction::Trust { path } => cmd_filters_trust(global, &resolved.effective, &path),
    }
}

/// Every discovered filter across all three tiers, alongside whether it's currently trusted. A
/// project/user pack file that's missing is silently absent from the list; one that exists but
/// fails to parse is still listed, with its parse error, so `list` doubles as a quick diagnostic.
fn discovered_filter_rows(effective: &config::Effective) -> Vec<serde_json::Value> {
    use tokenfold_core::filters::{self, FilterTier};

    let trust_store = filters::TrustStore::load(&effective.filters_trust_store_path);
    let mut rows = Vec::new();

    let tiers: [(FilterTier, &std::path::Path, bool); 2] = [
        (
            FilterTier::Project,
            effective.filters_project_filters_path.as_path(),
            effective.filters_trust_project_filters,
        ),
        (
            FilterTier::User,
            effective.filters_user_filters_path.as_path(),
            false,
        ),
    ];

    for (tier, path, bypass_trust) in tiers {
        if !path.is_file() {
            continue;
        }
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        match filters::FilterPack::parse(&String::from_utf8_lossy(&bytes)) {
            Ok(pack) => {
                let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
                let trusted = bypass_trust
                    || trust_store.is_trusted(&canonical, &bytes, &pack.schema_version);
                for filter in &pack.filters {
                    rows.push(serde_json::json!({
                        "tier": tier.as_str(),
                        "pack_id": pack.pack.id,
                        "pack_version": pack.pack.version,
                        "filter_id": filter.id,
                        "filter_version": filter.version,
                        "match_command": filter.match_command,
                        "path": path.display().to_string(),
                        "trusted": trusted,
                    }));
                }
            }
            Err(e) => {
                rows.push(serde_json::json!({
                    "tier": tier.as_str(),
                    "path": path.display().to_string(),
                    "error": e.to_string(),
                }));
            }
        }
    }

    for pack in filters::built_in_packs() {
        for filter in &pack.filters {
            rows.push(serde_json::json!({
                "tier": FilterTier::BuiltIn.as_str(),
                "pack_id": pack.pack.id,
                "pack_version": pack.pack.version,
                "filter_id": filter.id,
                "filter_version": filter.version,
                "match_command": filter.match_command,
                "path": serde_json::Value::Null,
                "trusted": true,
            }));
        }
    }

    rows
}

fn cmd_filters_list(
    global: &GlobalFlags,
    effective: &config::Effective,
) -> Result<i32, TokenFoldError> {
    let rows = discovered_filter_rows(effective);

    if global.json {
        println!("{}", serde_json::to_string_pretty(&rows).unwrap());
        return Ok(0);
    }

    println!(
        "{:<10} {:<16} {:<24} {:<8} MATCH_COMMAND",
        "TIER", "PACK", "FILTER", "TRUSTED"
    );
    for row in &rows {
        if let Some(err) = row.get("error").and_then(|v| v.as_str()) {
            println!(
                "{:<10} parse error at {}: {err}",
                row["tier"].as_str().unwrap_or("?"),
                row["path"].as_str().unwrap_or("?"),
            );
            continue;
        }
        let match_command = row["match_command"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        println!(
            "{:<10} {:<16} {:<24} {:<8} {match_command}",
            row["tier"].as_str().unwrap_or("?"),
            row["pack_id"].as_str().unwrap_or("?"),
            row["filter_id"].as_str().unwrap_or("?"),
            row["trusted"].as_bool().unwrap_or(false),
        );
    }
    Ok(0)
}

/// Validates schema + regex safety + inline fixtures for every discovered filter pack
/// (built-in, project, user — regardless of trust: `verify` is the pre-trust CI check per
/// INTERFACES.md §7.3, not a report on what's currently applied). `--require-all` is the
/// documented CI contract: any failure becomes a non-zero exit; without it, failures are still
/// reported but the command exits `0`.
fn cmd_filters_verify(
    global: &GlobalFlags,
    effective: &config::Effective,
    require_all: bool,
) -> Result<i32, TokenFoldError> {
    use tokenfold_core::filters::{self, FilterTier};

    let mut packs: Vec<(
        FilterTier,
        Option<PathBuf>,
        Result<filters::FilterPack, TokenFoldError>,
    )> = Vec::new();

    for (tier, path) in [
        (FilterTier::Project, &effective.filters_project_filters_path),
        (FilterTier::User, &effective.filters_user_filters_path),
    ] {
        if path.is_file() {
            packs.push((tier, Some(path.clone()), filters::parse_pack_file(path)));
        }
    }
    for pack in filters::built_in_packs() {
        packs.push((FilterTier::BuiltIn, None, Ok(pack.clone())));
    }

    let mut any_failed = false;
    let mut results = Vec::new();

    for (tier, path, parsed) in packs {
        let pack = match parsed {
            Ok(pack) => pack,
            Err(e) => {
                any_failed = true;
                results.push(serde_json::json!({
                    "tier": tier.as_str(),
                    "path": path.as_ref().map(|p| p.display().to_string()),
                    "ok": false,
                    "error": e.to_string(),
                }));
                continue;
            }
        };
        if let Err(e) = pack.validate() {
            any_failed = true;
            results.push(serde_json::json!({
                "tier": tier.as_str(),
                "pack_id": pack.pack.id,
                "ok": false,
                "error": e.to_string(),
            }));
            continue;
        }
        let fixture_checks = pack.run_fixtures()?;
        let pack_ok = fixture_checks.iter().all(|c| c.passed());
        any_failed |= !pack_ok;
        results.push(serde_json::json!({
            "tier": tier.as_str(),
            "pack_id": pack.pack.id,
            "pack_version": pack.pack.version,
            "ok": pack_ok,
            "fixtures": fixture_checks.iter().map(|c| serde_json::json!({
                "filter_id": c.filter_id,
                "fixture": c.fixture_name,
                "output_matches": c.output_matches,
                "expected_token_delta": c.expected_token_delta,
                "actual_token_delta": c.actual_token_delta,
                "passed": c.passed(),
            })).collect::<Vec<_>>(),
        }));
    }

    if global.json {
        println!("{}", serde_json::to_string_pretty(&results).unwrap());
    } else {
        for r in &results {
            println!("{}", serde_json::to_string(r).unwrap());
        }
        println!(
            "{}",
            if any_failed {
                "FAIL: one or more filters failed verification"
            } else {
                "OK: all filters passed verification"
            }
        );
    }

    Ok(if require_all && any_failed { 1 } else { 0 })
}

/// Records `path`'s canonical form + current SHA-256 + `schema_version` into the trust store.
/// Refuses to trust a pack that doesn't even parse/validate — an explicit `trust` action should
/// never mark a malformed filter as safe to run.
fn cmd_filters_trust(
    global: &GlobalFlags,
    effective: &config::Effective,
    path: &Path,
) -> Result<i32, TokenFoldError> {
    use tokenfold_core::filters;

    let canonical = std::fs::canonicalize(path).map_err(|e| {
        TokenFoldError::InvalidInput(format!("failed to canonicalize {}: {e}", path.display()))
    })?;
    let bytes = std::fs::read(&canonical)?;
    let pack = filters::FilterPack::parse(&String::from_utf8_lossy(&bytes))?;
    pack.validate()?;

    let mut store = filters::TrustStore::load(&effective.filters_trust_store_path);
    store.trust(&canonical, &bytes, &pack.schema_version);
    store.save(&effective.filters_trust_store_path)?;

    if global.json {
        println!(
            "{}",
            serde_json::json!({
                "trusted": true,
                "path": canonical.display().to_string(),
                "schema_version": pack.schema_version,
                "pack_id": pack.pack.id,
                "pack_version": pack.pack.version,
            })
        );
    } else {
        println!(
            "trusted {} (pack {} v{})",
            canonical.display(),
            pack.pack.id,
            pack.pack.version
        );
    }
    Ok(0)
}
