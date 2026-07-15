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

export interface CompressionOptions {
  format?: "auto" | "openai" | "anthropic" | "json" | "text" | "command" | "diff";
  mode?: CompressionMode;
  targetTokens?: number;
  disable?: readonly string[];
  taskScope?: TaskScope;
  experimental?: boolean;
  storeOriginals?: boolean;
  retrieveNamespace?: string;
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
  const args = [command, "--json"];
  if (options.format) args.push("--format", options.format);
  if (options.mode) args.push("--mode", options.mode);
  if (options.targetTokens !== undefined) args.push("--target-tokens", String(options.targetTokens));
  if (options.disable?.length && command === "compress") args.push("--disable", options.disable.join(","));
  if (options.taskScope) args.push("--task-scope", options.taskScope);
  if (options.experimental) args.push("--experimental");
  if (options.storeOriginals && command === "compress") args.push("--store-originals");
  if (options.retrieveNamespace && command === "compress") {
    args.push("--retrieve-namespace", options.retrieveNamespace);
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
  if (result.exitCode !== 0) {
    throw new TokenFoldProcessError(`tokenfold exited with status ${result.exitCode ?? result.signal}`, {
      code: "tokenfold_exit",
      exitCode: result.exitCode,
      signal: result.signal,
      stderr: result.stderr,
    });
  }

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
