import { binaryPath } from "./binary.js";
import { TokenFoldProcessError } from "./errors.js";
import { run, type Input, type ProcessResult, type RunOptions } from "./process.js";

export { binaryPath, run, TokenFoldProcessError };
export type { Input, ProcessResult, RunOptions };

export type CompressionMode = "conservative" | "balanced" | "aggressive";
export type TaskScope =
  | "all"
  | "general"
  | "code_review"
  | "change_summary"
  | "debugging"
  | "generation"
  | "api_overview"
  | "retrieval_qa"
  | "agent_history";

/** Selection backend for opt-in lossy pruning. `heuristic` is the only Phase 1 path. */
export type LossyPath = "heuristic";

export interface CompressionOptions {
  format?: "auto" | "openai" | "anthropic" | "json" | "text" | "command" | "diff";
  mode?: CompressionMode;
  targetTokens?: number;
  disable?: readonly string[];
  taskScope?: TaskScope;
  experimental?: boolean;
  storeOriginals?: boolean;
  retrieveNamespace?: string;
  /**
   * Opt-in LOSSY JSON array-item pruning (`--lossy`): drops whole array items to hit a token
   * budget instead of only restructuring them. Generic JSON only — on any other format the run
   * is a no-op and `json_prune` comes back as `status: "skipped"` with
   * `skipped_reason: "not_applicable_to_format"`, persisting nothing. Dropped items are replaced
   * by `$tf_ref` markers and stay recoverable via {@link retrieve}; pruning needs a durable
   * filesystem retrieval store, so pass `configPath` when you want one other than the default.
   */
  lossy?: LossyPath;
  /**
   * `--lossy-ratio`: a best-effort selection hint (0.0..=1.0), not an enforced budget — the
   * achieved ratio differs by design. Use `targetTokens` for a real ceiling.
   */
  lossyRatio?: number;
  /** `--lossy-preserve`: dot-separated paths (e.g. `data.results`) whose arrays are never pruned. */
  lossyPreserve?: readonly string[];
  configPath?: string;
  signal?: AbortSignal;
}

export interface EstimatorInfo {
  backend: string;
  model: string | null;
  is_exact: boolean;
}

export interface BudgetReport {
  target_tokens: number | null;
  protected_floor: number;
  achieved_tokens: number;
}

export interface QualityReport {
  eval_profile_id: string;
  task_scope: string;
  validated_ratio_band: string | null;
  quality_retention: number;
  contrastive_failure_rate: number;
  gate_passed: boolean;
}

export interface Warning {
  code: string;
  severity: "info" | "warn" | "critical";
  transform: string | null;
  message: string;
}

export interface TransformReport {
  id: string;
  version: string;
  tokens_before: number;
  tokens_after: number;
  saved_tokens: number;
  savings_ratio: number;
  elapsed_micros: number | null;
  status: "applied" | "no_op" | "skipped" | "rolled_back";
  skipped_reason: string | null;
  warnings: readonly Warning[];
}

export interface CacheReport {
  boundary_kind: string | null;
  protected_bytes: number;
  prefix_byte_identical: boolean;
  warnings: readonly Warning[];
}

export interface RetrievalReport {
  store_namespace: string;
  hash_algorithm: string;
  marker_count: number;
  ttl_seconds: number | null;
  persisted_original_bytes: number;
  skipped_original_bytes: number;
}

export interface OutputSavingsReport {
  profile: string;
  estimated_output_tokens_saved: number | null;
  measured_output_tokens_saved: number | null;
  provenance: string;
}

export interface BypassReport {
  reason: string;
  source: string;
}

export interface CommandReport {
  command_family: string | null;
  child_exit_code: number | null;
  duration_ms: number;
  raw_output_bytes: number;
  stdout_bytes: number;
  stderr_bytes: number;
  stderr_mode: string;
  stderr_truncated: boolean;
  compressed_output_bytes: number;
  filter_pack_id: string | null;
  filter_version: string | null;
  never_worse_applied: boolean;
  bypass_reason: string | null;
}

export interface LedgerReport {
  recorded: boolean;
  scope: string | null;
  project_hash: string | null;
  record_id: string | null;
}

export interface CompressionReport {
  schema_version: string;
  original_tokens: number;
  compressed_tokens: number;
  saved_tokens: number;
  savings_ratio: number;
  savings_pct: number;
  estimator: EstimatorInfo;
  status: "compressed" | "passthrough" | "best_effort" | "unreachable_target";
  mode: string;
  format: string;
  task_scope: string;
  request_id: string | null;
  quality: QualityReport | null;
  budget: BudgetReport | null;
  cache: CacheReport | null;
  retrieval: RetrievalReport | null;
  output_savings: OutputSavingsReport | null;
  bypass: BypassReport | null;
  command: CommandReport | null;
  ledger: LedgerReport | null;
  transforms: readonly TransformReport[];
  warnings: readonly Warning[];
}

export interface CompressionResult {
  payload: Uint8Array;
  report: CompressionReport;
}

function argumentsFor(command: "compress" | "inspect", options: CompressionOptions): string[] {
  // The `inspect` subcommand has no --lossy flags of its own; the CLI's lossy preview is
  // `compress --dry-run`, which routes to the same code path and, exactly like `inspect --json`,
  // writes the report to stdout and no payload. So a lossy inspect() becomes that instead.
  const wantsLossy =
    options.lossy !== undefined ||
    options.lossyRatio !== undefined ||
    (options.lossyPreserve?.length ?? 0) > 0;
  const previewLossy = command === "inspect" && wantsLossy;
  const args = previewLossy ? ["compress", "--json", "--dry-run"] : [command, "--json"];
  const compressing = command === "compress" || previewLossy;
  if (options.format) args.push("--format", options.format);
  if (options.mode) args.push("--mode", options.mode);
  if (options.targetTokens !== undefined) args.push("--target-tokens", String(options.targetTokens));
  if (options.disable?.length && compressing) args.push("--disable", options.disable.join(","));
  if (options.taskScope) args.push("--task-scope", options.taskScope);
  if (options.experimental) args.push("--experimental");
  if (options.storeOriginals && compressing) args.push("--store-originals");
  if (options.retrieveNamespace && compressing) {
    args.push("--retrieve-namespace", options.retrieveNamespace);
  }
  if (wantsLossy && compressing) {
    // `--lossy-ratio`/`--lossy-preserve` are `requires = "lossy"` in the CLI. Forward them even
    // when `lossy` is unset rather than dropping them: silently ignoring a pruning-aggression
    // knob is worse than the CLI's own "the following required arguments were not provided".
    if (options.lossy) args.push("--lossy", options.lossy);
    if (options.lossyRatio !== undefined) args.push("--lossy-ratio", String(options.lossyRatio));
    for (const path of options.lossyPreserve ?? []) args.push("--lossy-preserve", path);
  }
  if (options.configPath) args.push("--config", options.configPath);
  return args;
}

function parseReport(bytes: Uint8Array, result: ProcessResult): CompressionReport {
  try {
    return JSON.parse(Buffer.from(bytes).toString("utf8")) as CompressionReport;
  } catch (cause) {
    throw new TokenFoldProcessError("tokenfold returned an invalid JSON report", {
      code: "invalid_report",
      exitCode: result.exitCode,
      signal: result.signal,
      stderr: result.stderr,
      cause,
    });
  }
}

function throwIfFailed(result: ProcessResult): void {
  if (result.exitCode === 0) return;
  throw new TokenFoldProcessError(`tokenfold exited with status ${result.exitCode ?? result.signal}`, {
    code: "tokenfold_exit",
    exitCode: result.exitCode,
    signal: result.signal,
    stderr: result.stderr,
  });
}

async function execute(
  command: "compress" | "inspect",
  input: Input,
  options: CompressionOptions,
): Promise<CompressionResult> {
  const runOptions: RunOptions = {
    stdin: input,
    env: { TOKENFOLD_ANALYTICS_ENABLED: "false" },
  };
  if (options.signal) runOptions.signal = options.signal;
  const result = await run(argumentsFor(command, options), runOptions);
  throwIfFailed(result);

  const reportBytes = command === "compress" ? result.stderr : result.stdout;
  return {
    payload: command === "compress" ? result.stdout : Uint8Array.from(Buffer.from(input)),
    report: parseReport(reportBytes, result),
  };
}

export function compress(input: Input, options: CompressionOptions = {}): Promise<CompressionResult> {
  return execute("compress", input, options);
}

export function inspect(input: Input, options: CompressionOptions = {}): Promise<CompressionResult> {
  return execute("inspect", input, options);
}

export interface RetrieveOptions {
  /** `--retrieve-namespace`: the namespace the item was stored under. */
  namespace?: string;
  configPath?: string;
  signal?: AbortSignal;
}

/**
 * Restores the original bytes of something a lossy run dropped, or a `storeOriginals` run saved,
 * mirroring `tokenfold retrieve`. `reference` is either a raw hex SHA-256 hash — a compressed
 * payload's `$tf_ref.hash` — or a `[tokenfold:retrieve hash=... namespace=...]` text marker,
 * whose embedded namespace is used when `options.namespace` is omitted. A CompressionReport
 * path is NOT a valid reference: the current report schema carries no per-entry hash, and the
 * CLI rejects it rather than guessing.
 *
 * Throws `TokenFoldProcessError` (`code: "tokenfold_exit"`) when the hash is unknown, its TTL
 * has elapsed, or the reference is malformed — the CLI reports all of those as a non-zero exit.
 */
export async function retrieve(reference: string, options: RetrieveOptions = {}): Promise<Uint8Array> {
  const args = ["retrieve", reference];
  if (options.namespace) args.push("--retrieve-namespace", options.namespace);
  if (options.configPath) args.push("--config", options.configPath);
  const runOptions: RunOptions = { env: { TOKENFOLD_ANALYTICS_ENABLED: "false" } };
  if (options.signal) runOptions.signal = options.signal;
  const result = await run(args, runOptions);
  throwIfFailed(result);
  return result.stdout;
}
