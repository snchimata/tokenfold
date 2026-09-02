import { binaryPath } from "./binary.js";
import { TokenFoldProcessError } from "./errors.js";
import { run, type Input, type ProcessResult, type RunOptions } from "./process.js";
export { binaryPath, run, TokenFoldProcessError };
export type { Input, ProcessResult, RunOptions };

export type Preset = "conservative" | "balanced" | "aggressive";
export type OutputEncoding = "json" | "toon";
export type InputFormat = "auto" | "openai" | "anthropic" | "json" | "text" | "command" | "diff";
export type DecodeFormat = "auto" | "json" | "toon" | "text";
export interface PruningPolicy { keepRatio?: number; preservePaths?: readonly string[]; retrievalStore?: string; retrievalNamespace?: string }
export interface CompressionOptions {
  format?: InputFormat; preset?: Preset; targetTokens?: number; requireTarget?: boolean;
  encoding?: OutputEncoding; pruning?: PruningPolicy; configPath?: string; signal?: AbortSignal;
}
export interface EstimatorInfo { backend: string; model: string | null; is_exact: boolean }
export interface Warning { code: string; severity: "info" | "warn" | "critical"; transform: string | null; message: string }
export interface TransformReport {
  id: string; version: string; tokens_before: number; tokens_after: number; saved_tokens: number;
  savings_ratio: number; elapsed_micros: number | null; status: "applied" | "no_op" | "skipped" | "rolled_back";
  skipped_reason: string | null; warnings: readonly Warning[];
}
export interface BudgetReport { status: "not_requested" | "met" | "best_effort" | "unreachable"; target_tokens: number | null; protected_floor: number; achieved_tokens: number }
export interface EncodingReport { codec: string; version: string; roundtrip_verified: boolean; tokens_before: number; tokens_after: number; token_delta: number; warnings: readonly Warning[] }
export interface PruningReport { requested: boolean; applied: boolean; preview: boolean; candidate_items: number; retained_items: number; pruned_items: number; evidence_refs: number; preserve_paths: readonly string[] }
export interface QualityReport { eval_profile_id: string; task_scope: string; validated_ratio_band: string | null; quality_retention: number | null; contrastive_failure_rate: number | null; gate_passed: boolean }
export interface RetrievalReport { store_namespace: string; hash_algorithm: string; marker_count: number; ttl_seconds: number | null; persisted_original_bytes: number; skipped_original_bytes: number }
export interface PipelineStageReport { id: string; version: string | null; input_bytes: number | null; output_bytes: number | null; saved_bytes: number | null; input_tokens: number | null; output_tokens: number | null; saved_tokens: number | null; estimator: EstimatorInfo | null; status: string; duration_ms: number | null; bypass_reason: string | null; provenance: string; recoverability: string; evidence_ref: string | null }
export interface PipelineReport { raw_input_bytes: number | null; raw_input_tokens: number | null; final_output_bytes: number; final_output_tokens: number; total_saved_tokens: number | null; raw_capture: string; upstream_recoverability: string; stages: readonly PipelineStageReport[] }
export interface CompressionReceipt {
  schema_version: string; status: "compressed" | "passthrough"; original_tokens: number;
  compressed_tokens: number; saved_tokens: number; savings_ratio: number; savings_pct: number;
  estimator: EstimatorInfo; preset: Preset; format: string; output_encoding: string; task_scope: string;
  request_id: string | null; pipeline: PipelineReport | null; quality: QualityReport | null;
  budget: BudgetReport | null; encoding: EncodingReport | null; pruning: PruningReport | null;
  retrieval: RetrievalReport | null; transforms: readonly TransformReport[]; warnings: readonly Warning[];
  cache: unknown; output_savings: unknown; bypass: unknown; command: unknown; ledger: unknown;
}
export type CompressionReport = CompressionReceipt;
export interface CompressionResult { payload: Uint8Array; readonly text: string; report: CompressionReceipt }
export class BudgetUnmetError extends Error {
  readonly receipt: CompressionReceipt;
  constructor(receipt: CompressionReceipt) { super(`token budget unmet: achieved ${receipt.compressed_tokens} tokens`); this.name = "BudgetUnmetError"; this.receipt = receipt; }
}
function argumentsFor(command: "compress" | "inspect", options: CompressionOptions): string[] {
  const args = [command, "--receipt-format", "json"];
  if (options.format) args.push("--format", options.format);
  if (options.preset) args.push("--preset", options.preset);
  if (options.targetTokens !== undefined) args.push("--target-tokens", String(options.targetTokens));
  if (options.requireTarget) args.push("--require-target");
  if (options.encoding) args.push("--encoding", options.encoding);
  if (options.pruning) {
    args.push("--prune");
    if (options.pruning.keepRatio !== undefined) args.push("--keep-ratio", String(options.pruning.keepRatio));
    for (const path of options.pruning.preservePaths ?? []) args.push("--preserve", path);
    if (options.pruning.retrievalStore) args.push("--retrieval-store", options.pruning.retrievalStore);
    if (options.pruning.retrievalNamespace) args.push("--retrieval-namespace", options.pruning.retrievalNamespace);
  }
  if (options.configPath) args.push("--config", options.configPath);
  return args;
}
function parseReceipt(bytes: Uint8Array, result: ProcessResult): CompressionReceipt {
  const text = Buffer.from(bytes).toString("utf8");
  const json = text.split("\ntokenfold:", 1)[0] ?? "";
  try { return JSON.parse(json) as CompressionReceipt; }
  catch (cause) { throw new TokenFoldProcessError("tokenfold returned an invalid JSON receipt", { code: "invalid_report", exitCode: result.exitCode, signal: result.signal, stderr: result.stderr, cause }); }
}
function optionsFor(input: Input | undefined, signal?: AbortSignal): RunOptions {
  const options: RunOptions = { env: { TOKENFOLD_ANALYTICS_ENABLED: "false" } };
  if (input !== undefined) options.stdin = input;
  if (signal) options.signal = signal;
  return options;
}
function withText(payload: Uint8Array, report: CompressionReceipt): CompressionResult {
  return { payload, report, get text() { return new TextDecoder("utf-8", { fatal: true }).decode(payload); } };
}
export async function compress(input: Input, options: CompressionOptions = {}): Promise<CompressionResult> {
  const result = await run(argumentsFor("compress", options), optionsFor(input, options.signal));
  if (result.exitCode !== 0 && result.exitCode !== 7) throwProcess(result);
  const receipt = parseReceipt(result.stderr, result);
  if (result.exitCode === 7) throw new BudgetUnmetError(receipt);
  return withText(result.stdout, receipt);
}
export async function inspect(input: Input, options: CompressionOptions = {}): Promise<CompressionReceipt> {
  const result = await run(argumentsFor("inspect", options), optionsFor(input, options.signal));
  if (result.exitCode !== 0 && result.exitCode !== 7) throwProcess(result);
  const receipt = parseReceipt(result.stdout, result);
  if (result.exitCode === 7) throw new BudgetUnmetError(receipt);
  return receipt;
}
export async function decode(input: Input, options: { from?: DecodeFormat; signal?: AbortSignal } = {}): Promise<Uint8Array> {
  const args = ["decode"]; if (options.from) args.push("--from", options.from);
  const result = await run(args, optionsFor(input, options.signal)); if (result.exitCode !== 0) throwProcess(result); return result.stdout;
}
export interface RetrieveOptions { retrievalStore?: string; namespace?: string; configPath?: string; signal?: AbortSignal }
export async function retrieve(reference: string | Record<string, unknown>, options: RetrieveOptions = {}): Promise<Uint8Array> {
  const args = ["retrieve", typeof reference === "string" ? reference : JSON.stringify(reference)];
  if (options.retrievalStore) args.push("--retrieval-store", options.retrievalStore);
  if (options.namespace) args.push("--retrieval-namespace", options.namespace);
  if (options.configPath) args.push("--config", options.configPath);
  const result = await run(args, optionsFor(undefined, options.signal)); if (result.exitCode !== 0) throwProcess(result); return result.stdout;
}
function throwProcess(result: ProcessResult): never {
  throw new TokenFoldProcessError(`tokenfold exited with status ${result.exitCode ?? result.signal}`, { code: "tokenfold_exit", exitCode: result.exitCode, signal: result.signal, stderr: result.stderr });
}
