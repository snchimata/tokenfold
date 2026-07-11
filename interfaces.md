# tokenfold — Interfaces & Contracts

Authoritative reference for every boundary contract: CLI, `CompressionReport` JSON, proxy routes/headers, MCP tools, Python/TypeScript APIs, stats/filter/ledger, transform execution order, and `tokenfold.toml` schema. Changes to any boundary surface must update this doc in the same PR; implementation details belong in `PLAN.md`.

## Part 1 — CLI, Report JSON, Proxy Headers & Python API

### 1. CLI Command Surface

#### 1.1 Subcommand tree (canonical)

```
tokenfold
  compress    [default]   Read stdin or FILE, emit compressed payload to stdout
  inspect                 Dry-run: show what compress would do; never writes stdout payload
  diff                    Compression-aware diff: raw vs compressed
  wrap                    Run a command and compress its captured output (canonical)
  shell                   Visible compatibility alias of `wrap`
  benchmark               Run criterion/divan benchmarks against fixtures
  retrieve                Restore exact originals from retrieval markers or reports
  stats                   Aggregate report files or local ledger data
  gain                    Summarize realized savings from reports/ledger data
  session                 Report recent host-session coverage and bypasses
  filters                 List, verify, and trust declarative command-output filters
  init                    Install durable agent/host integration
  uninit                  Remove durable agent/host integration and restore backups
  doctor                  Verify hooks, stores, estimators, and proxy/MCP configuration
  auth                    Verify pass-through provider credentials (v0.3; `auth doctor`)
  mcp                     Serve/install/uninstall MCP tools
  discover                Find high-value uncompressed local tool/session outputs
  learn                   Propose compression policy/filter improvements from local evidence
  update                  Verify and install signed internal binary updates
  completions <SHELL>     Emit shell completion script (bash|zsh|fish|powershell)
  version                 Print version (also: --version flag)
```

#### 1.2 Naming decisions

**`wrap`/`shell` subcommands:** `wrap` is the canonical command name; `shell` is a retained visible alias. The name `shell` is ambiguous — users expect `tokenfold shell` to relate to shell environment, not command wrapping — and it follows `squeezer shell` prior art, so it is kept as a compatibility alias rather than the primary name. To reduce confusion:
- The short description in `--help` must be unambiguous: `shell — run a command and compress its output`
- `completions` is a separate subcommand (not `shell completions`)
- `wrap` is the preferred user-facing command name because market leaders use wrap-style language for command execution and agent integration.
- `shell` remains a visible compatibility alias with the same behavior.
- `exec` is a hidden alias for `wrap` (`#[command(alias = "exec", hide_alias = true)]` on the `Wrap`/`Shell` variant in clap v4).

**`inspect` vs `--dry-run`:** `inspect` is the subcommand, but `compress --dry-run` is a near-universal convention. Add:
```
tokenfold compress --dry-run  →  runs as inspect (no stdout payload; report to stderr or --json to stdout)
```
`--dry-run` is a documented (non-hidden) flag on `compress` that routes to the same code path as `inspect`. Show it in `compress --help`.

#### 1.3 Stream contract (the authoritative rule)

```
tokenfold compress:
  stdout  = compressed payload bytes (always)
  stderr  = human-readable report (always)
  --json  = structured CompressionReport JSON to stderr (payload still on stdout)
```

**Rule:** `--json` ALWAYS emits report to stderr. stdout is ALWAYS the payload (safe for pipe composition). To capture only the JSON report:

```bash
tokenfold compress file.json --json 2>report.json 1>/dev/null
```

**`inspect` stream contract** (different because inspect never produces a payload):
```
tokenfold inspect:
  stdout  = nothing (inspect never emits a payload)
  stderr  = human-readable dry-run report
  --json  = CompressionReport JSON to stdout (inspect has no payload to put on stdout)
```

`inspect` is the one subcommand where `--json` on stdout is safe, because there is no payload to compete with.

#### 1.3.1 Stream Matrix

| Command class | Default stdout | Default stderr | `--json` stdout | `--json` stderr | `--quiet` behavior |
|---------------|----------------|----------------|-----------------|-----------------|--------------------|
| `compress`, `wrap`/`shell` | Payload or compressed command output | Human report | Payload/output | `CompressionReport` JSON only | Suppresses human report; errors still stderr |
| `retrieve` | Restored original bytes | Human report | Restored bytes | Retrieval result JSON only | Suppresses human report; errors still stderr |
| `inspect`, `benchmark`, `stats`, `gain`, `session`, `discover`, `doctor`, `filters`, `update` | Human/report output | Warnings/errors | Machine JSON | Warnings/errors only | Suppresses non-error human prose |
| `diff` | Human diff | Warnings/errors | Structured hunk JSON | Warnings/errors only | Suppresses non-error human prose |
| `init`, `uninit`, `mcp install`, `mcp uninstall` | Human summary | Warnings/errors | Planned/applied edit JSON | Warnings/errors only | Suppresses non-error human prose |
| `mcp serve` | JSON-RPC only | Logs/warnings/errors | Not applicable | Logs/warnings/errors | Not applicable |
| `completions` | Completion script | Errors only | Not applicable | Errors only | Not applicable |

Commands that emit transformed payloads never print banners, savings summaries, or analytics to stdout. Squeez-style stdout savings banners are intentionally not copied because they break pipe composition.

`wrap`/`shell` capture child stdout and stderr together by default, compress that combined stream, and write the compressed command output to stdout. In `--json` mode, tokenfold-owned stderr is the `CompressionReport` JSON only; child stderr is included in the captured payload/report metadata, not passed through separately. `--passthrough-stderr` is an explicit opt-in for terminal use and disables the guarantee that stderr is JSON-only.

#### 1.4 Exit codes (explicit map)

| Exit code | Meaning | CLI status |
|-----------|---------|------------|
| 0 | Success (Compressed, Passthrough, BestEffort, **UnreachableTarget**) | `echo $?` → 0 |
| 1 | Reserved for future explicit verification/test failure modes; not used for normal compression outcomes | Do not emit for passthrough/no-op |
| 2 | Invalid input (bad format, missing file, invalid flag) | |
| 3 | Safety error (redaction failed, invariant violated) | |
| 4 | Estimator error (backend unavailable, token count failed) | |
| 5 | Config error (invalid tokenfold.toml, unknown field) | |
| 6 | Internal error (unexpected panic, assertion failed) | |

**`UnreachableTarget` is exit code 0.** It is a first-class compression outcome, not a failure. Callers who need to detect it should parse the `--json` report and check `status == "unreachable_target"`. Making it non-zero would break every pipe that uses `set -e`.

**Under-budget no-op:** Exit code 0 (nothing failed). If callers need to distinguish "compressed" from "already under budget," they should read `status == "passthrough"` from the JSON report. Exit 1 is reserved and must not be used for passthrough/no-op.

**Wrapper command precedence:** when `tokenfold wrap -- <cmd...>` or `tokenfold shell -- <cmd...>` launches a child command and compression succeeds or safely falls back, tokenfold returns the child process exit code. Tokenfold-owned exit codes are used only for launch, config, estimator, safety, or internal failures before/after child execution.

**Generated hook protocol:** host integrations call hidden `tokenfold hook <host>`. Hook exit codes are separate from normal CLI exits: `0` allow/rewrite emitted, `1` no rewrite/pass through, `2` deny/block, `3` ask/approval required. Hook stdout contains only the host-specific rewrite/decision payload; logs go to stderr.

#### 1.5 Transform discovery command

`--disable schema_compaction,log_compaction` requires knowing canonical IDs. Without a discovery command, this is unusable without reading docs.

Add to `tokenfold inspect --help` and `tokenfold --help`:
```
tokenfold inspect --list-transforms
```

Output (human + `--json`):

```
TRANSFORM           MODE      STATUS        FORMAT
json_minify         all       enabled       openai_json, anthropic_json
schema_compaction   all       enabled       openai_json, anthropic_json
table_compaction    all       disabled      openai_json, anthropic_json, plain_text
log_compaction      balanced+ experimental  plain_text, command_output
diff_compaction     balanced+ experimental  plain_text, command_output
```

This is not a separate subcommand — it is a flag on `inspect` (or `tokenfold transforms list` if preferred, but that adds a subcommand for a single use case).

#### 1.6 `--experimental` flag semantics

The plan references `--experimental` but does not specify whether it is a boolean (enable ALL experimental transforms) or a named set.

**Resolution:** Two levels:
- `--experimental` (boolean flag) — enable all non-experimental-gated transforms that have passed fidelity gate but not yet been promoted. Specifically enables `log_compaction` and `diff_compaction` at their validated ratio band.
- `--enable <ID>` (named flag) — enable a specific transform regardless of its `enabled` field in the mode matrix (requires explicit opt-in for transforms still behind `--experimental`).

Do NOT implement: a `--enable` flag that bypasses experimental status without the user also setting `--experimental`. Transforms are behind `--experimental` for a reason; `--enable log_compaction` alone should require `--experimental` to be set first (or error with "log_compaction is experimental; also pass --experimental"). This validation happens before pipeline construction.

#### 1.7 Color and output width

The plan mentions "green reachable / yellow best-effort" but does not specify the full color set. Canonical:

| State | Color | Symbol |
|-------|-------|--------|
| Compressed (target met) | green | ✅ |
| BestEffort (target not met, but improved) | yellow | ~ |
| UnreachableTarget (no improvement) | yellow | ! |
| Passthrough (no-op) | dim/gray | — |
| Warning (critical) | red | ❌ |
| Warning (warn) | yellow | ⚠️ |
| Warning (info) | blue | 🟦 |
| Skipped transform | dim | (gray text) |

Honor `NO_COLOR` env var (no-color.org standard) and `--no-color` flag. Honor `isatty(stderr)` for auto-disabling color when piped.

Table column widths: right-align numeric columns; truncate TRANSFORM names at 22 chars with `…`; emit no wider than 100 chars total. If `--no-truncate` is set, do not truncate.

#### 1.8 Parity command contracts

These commands exist to cover Headroom/RTK surfaces without overloading `compress`.

| Command | Stable behavior |
|---------|-----------------|
| `tokenfold init --agent <host> [--scope user|project] [--dry-run] [--json] [--show] [--auto-patch|--no-patch] [--hook-only]` | Installs a thin host hook/rewrite rule that delegates to `tokenfold hook <host>`; writes a byte-restorable backup before modification; `--show` prints the planned host config patch without applying it. |
| `tokenfold uninit --agent <host> [--scope user|project] [--json]` | Removes the tokenfold-managed block and restores the backed-up host config byte-for-byte when possible. |
| `tokenfold doctor [--agent <host>] [--json]` | Verifies hook install state, estimator availability, local stores, proxy/MCP config, and update metadata. |
| `tokenfold auth doctor [--json]` | Verifies pass-through provider credentials without storing them (v0.3, `ROADMAP.md` F-053). |
| `tokenfold mcp serve` | Runs a stdio MCP server exposing `tokenfold_compress`, `tokenfold_inspect`, and, when enabled, `tokenfold_retrieve` and `tokenfold_stats` (and `tokenfold_read` only when `TOKENFOLD_MCP_READ=on`). |
| `tokenfold mcp install|uninstall|status --client <name>` | Adds/removes/reports MCP client configuration without touching unrelated client settings. |
| `tokenfold retrieve <marker-or-report> [--store <path>]` | Restores exact original spans from the local evidence store. Missing/expired content is a non-zero retrieval error, not a partial silent result. |
| `tokenfold stats <report-glob> [--ledger <path>] [--json|--csv|--serve]` | Aggregates report JSON and optional redacted ledger metadata. `--serve` binds loopback only. |
| `tokenfold gain [--scope project|user] [--since 30d] [--json|--csv]` | Summarizes realized token/cost savings from report and ledger data. |
| `tokenfold session [--recent N] [--json]` | Reports host-session coverage: total commands, wrapped commands, raw commands, bypasses, and coverage/adoption percentage (the `coverage_pct` field of `StatsSummary`). |
| `tokenfold filters list|status|verify|trust|untrust|init-template` | Manages declarative command-output filter packs. `verify` runs inline fixtures and regex-safety checks. |
| `tokenfold discover` | Reports likely savings opportunities from local report/session metadata without reading raw payload bytes by default. |
| `tokenfold learn` | Writes recommendations only; applying them requires explicit user approval. |
| `tokenfold update [--check|--apply|--rollback]` | Uses internal signed release metadata; checksum/signature verification is mandatory. |

`hook` is an internal subcommand used by installed host integrations. It is stable enough for generated hook files but hidden from normal help.

Generated host hooks are thin delegates only. Host config writes are atomic; backups are byte-restorable; unrelated host settings are preserved. Missing binary or incompatible generated-hook version fails open/pass-through unless the host explicitly requested deny/block behavior.

`wrap`/`shell` accepts `-- <cmd...>` as the command separator. Reports include child `exit_code`, `duration_ms`, command-family hint, filter IDs, and whether `never_worse` returned raw output. Raw command args are redacted or hashed by default in persisted ledger data.

### 2. `CompressionReport` JSON Contract

#### 2.1 Complete canonical schema

```json
{
  "schema_version": "1.0",
  "original_tokens": 18400,
  "compressed_tokens": 11900,
  "saved_tokens": 6500,
  "savings_pct": 35.3,
  "savings_ratio": 0.353,
  "estimator": {
    "backend": "tiktoken",
    "model": "o200k_base",
    "is_exact": true
  },
  "status": "compressed",
  "mode": "balanced",
  "format": "openai_json",
  "task_scope": "general",
  "request_id": "tc-7f3a2b1c",
  "quality": null,
  "budget": {
    "target_tokens": 12000,
    "protected_floor": 8600,
    "achieved_tokens": 11900
  },
  "cache": null,
  "retrieval": null,
  "output_savings": null,
  "bypass": null,
  "command": null,
  "ledger": null,
  "transforms": [
    {
      "id": "secret_redaction",
      "version": "1.0.0",
      "tokens_before": 18400,
      "tokens_after": 18400,
      "saved_tokens": 0,
      "savings_ratio": 0.0,
      "elapsed_micros": null,
      "status": "applied",
      "skipped_reason": null,
      "warnings": []
    },
    {
      "id": "json_minify",
      "version": "1.0.0",
      "tokens_before": 18400,
      "tokens_after": 14200,
      "saved_tokens": 4200,
      "savings_ratio": 0.228,
      "elapsed_micros": null,
      "status": "applied",
      "skipped_reason": null,
      "warnings": []
    },
    {
      "id": "schema_compaction",
      "version": "1.0.0",
      "tokens_before": 14200,
      "tokens_after": 11900,
      "saved_tokens": 2300,
      "savings_ratio": 0.162,
      "elapsed_micros": null,
      "status": "applied",
      "skipped_reason": null,
      "warnings": []
    },
    {
      "id": "log_compaction",
      "version": "1.0.0",
      "tokens_before": 11900,
      "tokens_after": 11900,
      "saved_tokens": 0,
      "savings_ratio": 0.0,
      "elapsed_micros": null,
      "status": "skipped",
      "skipped_reason": "not_applicable_to_format",
      "warnings": []
    }
  ],
  "warnings": []
}
```

#### 2.2 Design decisions

**`schema_version`:** Required top-level `"1.0"`. Gate on this before deserializing. Breaking changes → `"2.0"`; non-breaking nullable additions are minor bumps.

**`estimator` as a struct:** The canonical contract uses a struct:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EstimatorInfo {
    pub backend: String,            // "heuristic" | "tiktoken" | "anthropic" | "huggingface"
    pub model: Option<String>,      // "o200k_base" | "claude-3-5-sonnet-20241022" | null for heuristic
    pub is_exact: bool,             // false for heuristic; true when backend returned an exact count
}
```

Human/header rendering: `estimator.is_exact` controls whether `~` prefix and `EST TOKENS` label are shown. JSON callers don't need to parse a string to detect heuristic mode.

**`savings_pct` (positive sign):**
- `savings_ratio: f64` — raw fraction (0.353), kept for backward compat
- `savings_pct: f64` — the number `35.3` (positive, no sign), for human-friendly use
- Human output renders as `35.3% reduction` (not `-35.3%`)
- Proxy headers use `35.3` (positive number, no sign)

**`TransformReport.warnings` (per-transform warnings, new):** Global `warnings: Vec<Warning>` stays, but `TransformReport` also gets `warnings: Vec<Warning>`. A warning like `UnredactedContentPossible` on the redaction transform is actionable when scoped to that transform. Global warnings are for cross-transform or pipeline-level issues.

**`quality: Option<QualityReport>` presence rule (explicit):**
- `None` when no lossy transforms ran (all lossless or no-op)
- `Some(...)` when at least one lossy/evidence-marked transform ran and the fidelity gate data is available at runtime
- `Some(QualityReport { validated_ratio_band: None, ... })` when a lossy transform ran but no gate data was baked in at build time (early dev builds before Phase 2)

Callers must not assume `quality != None` implies gate passed. Check `quality.gate_passed` explicitly.

**`budget: Option<BudgetReport>` (new):** Present when a target is supplied or the protected floor was computed for reporting. It carries target/floor/achieved counts that used to be embedded in `Status::UnreachableTarget`; `status` remains a unit enum so it serializes as a stable snake_case string.

**`cache: Option<CacheReport>` (new):** Present when the input included a prompt-cache boundary, cache-control blocks, or live-zone compression policy. Records `boundary_kind`, `protected_bytes`, `prefix_byte_identical`, and any `cache_policy_warnings`. This is the report-level proof for F-044.

**`retrieval: Option<RetrievalReport>` (new):** Present when F-045 stores originals or emits retrieval markers. Records store namespace, marker count, hash algorithm, TTL, and whether any original span was not persisted because of redaction/privacy policy.

**`output_savings: Option<OutputSavingsReport>` (new):** Present only when F-050 output-shaping or holdout measurement is enabled. It is never merged into `saved_tokens`, which is input-token savings only.

**`bypass: Option<BypassReport>` (new):** Present when compression is skipped by explicit bypass, environment, config, unsupported stream shape, or safety fallback. This is top-level because bypasses can occur outside command wrappers.

**`mode`, `format`, `task_scope`, and `request_id` (new):** These fields make report files self-contained. `format` is the resolved input format after `InputFormat::Auto` detection, not merely the caller's requested value. `request_id` is optional for local CLI calls and required for proxy/MCP requests.

**`command: Option<CommandReport>` (new):** Present for `wrap`/`shell` and hook-managed command output. Records child process metadata, filter provenance, and whether the `never_worse` guard returned raw output.

**`ledger: Option<LedgerReport>` (new):** Present when a report was written to the local ledger. It records only redacted metadata and never contains raw payload bytes or full command arguments.

**`status` values (canonical string form for JSON):**

| Rust variant | JSON string |
|--------------|-------------|
| `Status::Compressed` | `"compressed"` |
| `Status::Passthrough` | `"passthrough"` |
| `Status::BestEffort` | `"best_effort"` |
| `Status::UnreachableTarget` | `"unreachable_target"` |

Use `snake_case` in JSON (not PascalCase variant names). The `serde(rename_all = "snake_case")` attribute handles this.

**`TransformReport.status` values:**

| Status | Meaning |
|--------|---------|
| `"applied"` | Transform ran and modified output |
| `"no_op"` | Transform ran but made no change |
| `"skipped"` | Transform did not run; see `skipped_reason` |
| `"rolled_back"` | Transform ran, failed safety check, was reverted |

**`skipped_reason` values:**

| Reason | Meaning |
|--------|---------|
| `"target_already_met"` | Pipeline exited early |
| `"not_applicable_to_format"` | Transform doesn't handle this input format |
| `"not_enabled_in_mode"` | Transform is off for the active mode |
| `"experimental_flag_required"` | Transform is experimental and `--experimental` was not set |
| `"disabled_by_user"` | `--disable` flag or config |
| `"would_increase_tokens"` | Transform would grow the payload; skipped by the never-worse guard |
| `"filter_untrusted"` | Command-output filter pack is not trusted |
| `"filter_failed_verify"` | Filter pack failed inline fixtures / regex-safety verification |
| `"bypass_env_set"` | `TOKENFOLD_DISABLED`/`TOKENFOLD_PASSTHROUGH` (or request bypass) is set |
| `"unsupported_command_shape"` | Wrapped command shape cannot be safely rewritten |
| `"pipe_or_heredoc_not_rewritten"` | Command uses a pipe/heredoc that is not rewritten |
| `"binary_output_detected"` | Captured output is binary; not compressible as text |
| `"unsafe_command_passthrough"` | Command flagged unsafe; raw output passed through |

The full set of `SkippedReason` values is defined by the enum in §2.3; the first five are the common pipeline cases, the remainder cover filter, command-wrapper, and bypass paths.

#### 2.3 Rust type definitions (updated)

Replace the PLAN.md `CompressionReport` struct definition with:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompressionReport {
    pub schema_version: String,         // always "1.0"
    pub original_tokens: usize,
    pub compressed_tokens: usize,
    pub saved_tokens: usize,
    pub savings_ratio: f64,             // fraction: 0.353
    pub savings_pct: f64,               // positive percent: 35.3
    pub estimator: EstimatorInfo,       // struct, not string
    pub status: Status,
    pub mode: String,
    pub format: String,
    pub task_scope: String,
    pub request_id: Option<String>,
    pub quality: Option<QualityReport>,
    pub budget: Option<BudgetReport>,
    pub cache: Option<CacheReport>,
    pub retrieval: Option<RetrievalReport>,
    pub output_savings: Option<OutputSavingsReport>,
    pub bypass: Option<BypassReport>,
    pub command: Option<CommandReport>,
    pub ledger: Option<LedgerReport>,
    pub transforms: Vec<TransformReport>,
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Compressed,
    Passthrough,
    BestEffort,
    UnreachableTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EstimatorInfo {
    pub backend: String,
    pub model: Option<String>,
    pub is_exact: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BudgetReport {
    pub target_tokens: Option<usize>,
    pub protected_floor: usize,
    pub achieved_tokens: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QualityReport {
    pub eval_profile_id: String,
    pub task_scope: String,
    pub validated_ratio_band: Option<String>,
    pub quality_retention: f64,
    pub contrastive_failure_rate: f64,
    pub gate_passed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransformReport {
    pub id: String,                      // canonical transform ID
    pub version: String,                 // semver string "1.0.0"
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub saved_tokens: usize,
    pub savings_ratio: f64,
    pub elapsed_micros: Option<u64>,     // present in benchmark/profiling builds
    pub status: TransformStatus,
    pub skipped_reason: Option<SkippedReason>,
    pub warnings: Vec<Warning>,          // per-transform warnings (new)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransformStatus {
    Applied,
    NoOp,
    Skipped,
    RolledBack,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkippedReason {
    TargetAlreadyMet,
    NotApplicableToFormat,
    NotEnabledInMode,
    ExperimentalFlagRequired,
    DisabledByUser,
    WouldIncreaseTokens,
    FilterUntrusted,
    FilterFailedVerify,
    BypassEnvSet,
    UnsupportedCommandShape,
    PipeOrHeredocNotRewritten,
    BinaryOutputDetected,
    UnsafeCommandPassthrough,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Warning {
    pub code: WarningCode,
    pub severity: Severity,
    pub transform: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Severity { Info, Warn, Critical }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WarningCode {
    UnreachableTarget,
    UnredactedContentPossible,
    SafetyDowngrade,
    SecurityFieldAltered,
    HeuristicBudgetUsed,
    PrefixModified,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CacheReport {
    pub boundary_kind: Option<String>,       // "byte_offset" | "turn_index" | "provider_cache_control"
    pub protected_bytes: usize,
    pub prefix_byte_identical: bool,
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RetrievalReport {
    pub store_namespace: String,
    pub hash_algorithm: String,              // "sha256" | "blake3"
    pub marker_count: usize,
    pub ttl_seconds: Option<u64>,
    pub persisted_original_bytes: usize,
    pub skipped_original_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutputSavingsReport {
    pub profile: String,                     // "none" | "terse" | "standard"
    pub estimated_output_tokens_saved: Option<usize>,
    pub measured_output_tokens_saved: Option<usize>,
    pub provenance: String,                  // "estimated" | "measured" | "holdout"
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BypassReport {
    pub reason: String,        // "env" | "request_header" | "config" | "unsupported_stream" | "safety_fallback"
    pub source: String,        // "cli" | "proxy" | "mcp" | "hook"
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandReport {
    pub command_family: Option<String>,
    pub child_exit_code: Option<i32>,
    pub duration_ms: u64,
    pub raw_output_bytes: usize,
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
    pub stderr_mode: String,          // "captured" | "passthrough"
    pub stderr_truncated: bool,
    pub compressed_output_bytes: usize,
    pub filter_pack_id: Option<String>,
    pub filter_version: Option<String>,
    pub never_worse_applied: bool,
    pub bypass_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LedgerReport {
    pub recorded: bool,
    pub scope: Option<String>,
    pub project_hash: Option<String>,
    pub record_id: Option<String>,
}
```

### 3. Proxy Header Contract

#### 3.0 Proxy HTTP Route Contract

`tokenfold-proxy` exposes both provider-compatible data-plane routes and tokenfold-native control routes. Health routes are always unauthenticated on loopback. Non-loopback deployments require proxy auth for every route except `/livez`, `/readyz`, and `/health`.

| Route | Method | Auth boundary | Request | Response | Raw content exposure |
|-------|--------|---------------|---------|----------|----------------------|
| `/livez` | `GET` | none | none | liveness JSON/text | none |
| `/readyz` | `GET` | none | none | readiness JSON/text with upstream/config state | no raw payloads |
| `/health` | `GET` | none | none | combined health JSON/text | no raw payloads |
| `/v1/compress` | `POST` | proxy auth if non-loopback | `{ payload/messages, format?, mode?, target_tokens?, store_originals? }` | `{ payload/messages, report }` | input accepted; response may include compressed content |
| `/v1/retrieve` | `POST` | proxy auth | `{ marker | hash | report_ref }` | retrieval result JSON | returns original only when store policy allows |
| `/v1/retrieve/{hash}` | `GET` | proxy auth | path hash | retrieval result JSON | returns original only when store policy allows |
| `/v1/retrieve/stats` | `GET` | proxy auth | optional namespace/window | retrieval store stats | no raw originals |
| `/v1/retrieve/tool_call` | `POST` | proxy auth | tool-call retrieval request | retrieval result JSON | returns original only when store policy allows |
| `/stats` | `GET` | proxy auth if non-loopback | optional scope/window | stats summary JSON | no raw payloads |
| `/stats-history` | `GET` | proxy auth | `format=json|csv`, `series=history|hourly|daily|weekly|monthly` | historical stats JSON/CSV | no raw payloads |
| `/metrics` | `GET` | proxy auth if non-loopback | none | Prometheus metrics | no raw payloads |
| `/dashboard` | `GET` | loopback only by default | none | static dashboard | no raw payloads |
| `/stats/reset` | `POST` | loopback/admin only | optional scope | reset acknowledgement | no raw payloads |
| `/cache/clear` | `POST` | loopback/admin only | optional namespace | clear acknowledgement | no raw payloads |
| provider routes (`/v1/chat/completions`, `/v1/responses`, `/v1/messages`, `/anthropic/v1/messages`, Gemini/Vertex/Bedrock-compatible paths) | provider-native | proxy auth if non-loopback; upstream auth passed through | provider request shape | provider response shape + receipt headers when possible | request content processed; upstream response passed through |

Admin/debug routes such as `/admin/runtime-env`, `/admin/upstream`, `/debug/*`, and `/transformations/feed` are optional development surfaces. They bind loopback only, redact secrets, and are disabled in production builds unless explicitly enabled.

Project attribution uses `X-TokenFold-Project`. For clients that cannot set headers, `/p/<project>/...` is an equivalent path prefix. The normalized project value is recorded in reports/ledger metadata after redaction or hashing.

#### 3.1 Full header set

| Header | Direction | Example | Notes |
|--------|-----------|---------|-------|
| `X-TokenFold-Status` | Response | `compressed` | Matches `CompressionReport.status` snake_case values |
| `X-TokenFold-Original-Tokens` | Response | `18400` | Integer |
| `X-TokenFold-Compressed-Tokens` | Response | `11900` | Integer |
| `X-TokenFold-Savings-Pct` | Response | `35.3` | Positive float, one decimal, no sign |
| `X-TokenFold-Estimator` | Response | `tiktoken:o200k_base` | Colon-delimited `backend:model`; `heuristic` has no model suffix |
| `X-TokenFold-Applied` | Response | `json_minify,schema_compaction` | Comma-separated applied transform IDs |
| `X-TokenFold-Applied-Versions` | Response | `json_minify@1.0.0,schema_compaction@1.0.0` | ID@version pairs (new) |
| `X-TokenFold-Request-Id` | Both | `tc-7f3a2b1c` | Unique per proxied request; echoed if client sent it |
| `X-TokenFold-Project` | Request | `customer-portal` | Project attribution for stats/ledger; sanitized before persistence |
| `X-TokenFold-Stack` | Request | `litellm` | SDK/integration attribution for stats/diagnostics |
| `X-TokenFold-Bypass` | Request | `true` | Skip compression and report `bypass.reason="request_header"` (populates `CompressionReport.bypass`) |
| `X-TokenFold-Proxy-Token` | Request | `<redacted>` | Proxy auth for provider-compatible routes; required instead of `Authorization` when upstream auth also uses `Authorization` |
| `X-TokenFold-Mode` | Both | `balanced` | Request override and response receipt for active compression mode |
| `X-TokenFold-Format` | Both | `openai_json` | Request override and response receipt for detected/resolved format |
| `X-TokenFold-Target-Tokens` | Request | `12000` | Per-request input budget target |
| `X-TokenFold-Disable` | Request | `schema_compaction,log_compaction` | Per-request transform disable list |
| `X-TokenFold-Lossless-Only` | Request | `true` | Disables lossy transforms for this request; semantics-preserving transforms may still run |
| `X-TokenFold-Store-Originals` | Request | `true` | Enables F-045 local retrieval store for this request when allowed |
| `X-TokenFold-Retrieve-Store` | Request | `project` | Selects retrieval namespace/scope |
| `X-TokenFold-Output-Profile` | Request | `terse` | Optional F-050 output-shaping profile |
| `X-TokenFold-Cache-Boundary` | Request | `turn:3` | Declares frozen prompt-cache prefix boundary |

#### 3.2 Design decisions

**Comma-separated `X-TokenFold-Applied`:** Canonical transform IDs do not contain commas (they are `snake_case` identifiers). Document this as a hard constraint on transform ID naming. IDs are validated at registration: `[a-z][a-z0-9_]*`. Any future ID that would contain a comma is rejected at compile time.

**`X-TokenFold-Applied-Versions` (new header):** The plan specifies transform versioning (`TransformReport.version`) but the proxy headers don't expose it. This header allows operators to track exactly which transform versions processed a request for debugging and audit. Format: `id@semver` pairs, comma-separated.

**`X-TokenFold-Request-Id` (new header):** Correlates proxy access logs with compression reports. If the client sends this header, the proxy echoes it unchanged. If absent, the proxy generates one with prefix `tc-` + 8 hex chars.

**`X-TokenFold-Savings-Pct` (renamed from `X-TokenFold-Savings-Ratio`):** Consistent with `CompressionReport.savings_pct` — a positive float like `35.3`, not `-0.353`. Easier to read in log scrapers and dashboards.

**`best_effort` vs `unreachable_target` in status header:** These are distinct:
- `best_effort` = target was given AND some compression happened BUT target was not fully reached
- `unreachable_target` = target was below the protected floor OR no safe transform could make progress toward the target

A `passthrough` (no target, transforms ran but nothing changed the token count) is also distinct. The four-way distinction matters for SLO dashboards.

**No response body modification for headers:** The proxy adds headers to the upstream response, but does NOT add any X-TokenFold headers to error responses from upstream (4xx/5xx passthrough). This avoids misleading callers about whether tokenfold processed the failing request.

**Proxy auth vs upstream auth:** provider-compatible routes reserve `Authorization` for upstream provider credentials. Non-loopback proxy auth on those routes must use `X-TokenFold-Proxy-Token` or `Proxy-Authorization`. Native tokenfold control routes may accept `Authorization: Bearer <proxy-token>` because there is no upstream provider credential on those routes. TypeScript `apiKey` maps to proxy auth, not provider auth.

**No per-request upstream override (SSRF invariant):** The proxy ignores every inbound `X-TokenFold-*` header not listed in the §3.1 request table. In particular, there is **no `X-TokenFold-Upstream` (or equivalent) header** and no other per-request mechanism to change the upstream URL, credential, or routing — the upstream is fixed at process start (`--upstream` / `proxy.upstream`). This intentionally diverges from Headroom-style `x-headroom-base-url` compatibility in favor of a stricter SSRF boundary. Any future per-request routing override is forbidden without an explicit SSRF/security review. Combined with the pass-through auth model (the client supplies its own upstream credential; the proxy stores none), this closes classic SSRF and credential-drain vectors.

### 4. MCP Tool Contract

`tokenfold mcp serve` speaks stdio MCP. stdout is JSON-RPC only; logs go to stderr. Tool names are stable after v0.2.

| Tool | Input schema | Output schema | Notes |
|------|--------------|---------------|-------|
| `tokenfold_compress` | `{ "content"?: string, "messages"?: array, "format"?: string, "mode"?: string, "target_tokens"?: number, "store_originals"?: boolean }` | `{ "content"?: string, "messages"?: array, "report": CompressionReport, "markers"?: RetrievalMarker[] }` | Exactly one of `content` or `messages` is required. |
| `tokenfold_inspect` | Same as `tokenfold_compress` except `store_originals` defaults false | `{ "report": CompressionReport, "preview"?: string }` | Never returns modified payload as authoritative output. |
| `tokenfold_retrieve` | `{ "hash"?: string, "marker"?: string, "report_ref"?: string }` | `{ "status": "found"|"missing"|"expired", "source": "local_mcp"|"proxy_store"|"report_store"|"sqlite"|"memory", "content"?: string, "ttl_seconds_remaining"?: number }` | Missing/expired retrieval is explicit, never partial silent output. |
| `tokenfold_stats` | `{ "scope"?: "session"|"project"|"user", "window"?: string }` | `StatsSummary` | No raw payloads or originals. |
| `tokenfold_read` | `{ "file_path": string, "fresh"?: boolean }` | `{ "content": string, "report"?: CompressionReport }` | Optional, disabled by default. Requires explicit `TOKENFOLD_MCP_READ=on`; path access is host-policy constrained. |

`tokenfold_retrieve` accepts hash/marker/report references only; no semantic query parameter exists in v0.2. Query-based retrieval belongs to the optional RAG/vector extension.

#### Retrieval Marker Grammar

When an output format can carry comments/metadata safely, tokenfold may emit bounded retrieval markers:

```text
[tokenfold:retrieve hash=<hex> alg=sha256 namespace=<ns> bytes=<n> ttl=<seconds>]
```

Rules:
- `hash` is lowercase hex for the configured hash algorithm (`sha256` by default; `blake3` allowed when configured).
- `namespace` is a redacted project/session namespace.
- `bytes` is the original byte count for that span.
- `ttl` is seconds from storage time; omitted only for non-expiring test fixtures.
- Markers are never inserted into JSON string values, tool schemas, system prompts, or provider cache prefixes. If a format cannot carry markers safely, markers live only in `CompressionReport.retrieval`.
- Retrieval checks local MCP store first, then proxy/report store if configured. In-memory stores are per process; multi-worker proxy deployments require a persistent store or sticky routing.

### 5. Python Binding API

#### 5.1 Canonical API (updated from PLAN.md)

```python
from tokenfold import (
    Status,
    compress,
    inspect,                 # new: dry-run equivalent of compress
    CompressionResult,       # new: named return type
    CompressionReport,
    CompressionPolicy,
    CompressionMode,
    InputFormat,
    TokenFoldError,
)
```

Canonical function signature:

```python
def compress(
    payload: bytes | str,
    *,
    policy: CompressionPolicy | None = None,
    format: InputFormat | str = InputFormat.AUTO,
    mode: CompressionMode | str = CompressionMode.BALANCED,
    target_tokens: int | None = None,
    disable: list[str] | None = None,
    allow_heuristic_budget: bool = False,
) -> CompressionResult: ...

def inspect(
    payload: bytes | str,
    *,
    policy: CompressionPolicy | None = None,
    format: InputFormat | str = InputFormat.AUTO,
    mode: CompressionMode | str = CompressionMode.BALANCED,
    target_tokens: int | None = None,
) -> CompressionResult: ...
```

Usage example:

```python
result = compress(
    payload,
    format=InputFormat.AUTO,
    mode=CompressionMode.BALANCED,
    target_tokens=12_000,
    disable=["log_compaction"],
    allow_heuristic_budget=False,
)
```

`str` input is UTF-8 encoded; `CompressionResult.payload` is always `bytes`. Convenience wrappers may decode UTF-8 for message-oriented APIs but the core API is bytes-first.

Format-specific convenience wrappers are thin and delegate to `compress()`:

```python
compress_openai_payload(payload, *, mode=..., target_tokens=...) -> CompressionResult
compress_anthropic_payload(payload, *, mode=..., target_tokens=...) -> CompressionResult
```

#### 5.2 Message-Oriented API

Headroom's common user path is message-oriented. Tokenfold keeps the bytes API as core but adds a high-level wrapper for OpenAI/Anthropic-style messages:

```python
result = compress_messages(
    messages,
    *,
    model="gpt-4.1",
    token_budget=12_000,
    mode=CompressionMode.BALANCED,
)
```

Return fields mirror `CompressionResult` plus `messages`, `tokens_before`, `tokens_after`, `tokens_saved`, `savings_pct`, `transforms_applied`, and `retrieval_hashes`.

#### 5.3 `CompressionResult` named return type (new)

The canonical return is a named dataclass:

```python
import dataclasses

@dataclasses.dataclass(frozen=True)
class CompressionResult:
    payload: bytes
    report: CompressionReport

    # Convenience
    def saved_pct(self) -> float:
        return self.report.savings_pct

    def is_over_budget(self) -> bool:
        return self.report.status in (Status.BEST_EFFORT, Status.UNREACHABLE_TARGET)
```

Rationale: tuple unpacking `payload, report = compress(...)` is fragile as the API evolves — adding a third return value breaks every caller. A named dataclass is forward-compatible and is self-documenting.

The Rust FFI layer constructs `CompressionResult` from `CompressionOutput` via pyo3 `IntoPy`.

`CompressionPolicy` is an optional convenience dataclass mirroring the Rust policy. If `policy` is supplied together with individual keyword arguments, explicit keyword arguments win, matching CLI precedence rules.

#### 5.4 Enum naming convention

pyo3 by default maps Rust `PascalCase` enum variants to Python identically (`CompressionMode.Balanced`). Python convention is `ALL_CAPS` (`CompressionMode.BALANCED`).

**Resolution:** Use explicit `#[pyo3(name = "BALANCED")]` on each variant, or use `#[pyclass]` with a Python-specific repr. The Python user-facing names are `ALL_CAPS`:

```python
CompressionMode.CONSERVATIVE
CompressionMode.BALANCED
CompressionMode.AGGRESSIVE

InputFormat.OPENAI_JSON
InputFormat.ANTHROPIC_JSON
InputFormat.PLAIN_TEXT
InputFormat.COMMAND_OUTPUT
InputFormat.GIT_DIFF
InputFormat.AUTO

Status.COMPRESSED
Status.PASSTHROUGH
Status.BEST_EFFORT
Status.UNREACHABLE_TARGET
```

Document this explicitly in `crates/tokenfold-py/src/lib.rs` with a comment: "Python names use ALL_CAPS by convention; Rust names use PascalCase."

#### 5.5 Error hierarchy

```python
TokenFoldError                      # base
  ├── InvalidInputError             # bad format, malformed JSON, empty input
  ├── SafetyError                   # redaction failed, invariant violated
  ├── EstimatorError                # backend unavailable, token count failed
  ├── ConfigError                   # invalid mode, unknown transform ID
  └── InternalError                 # unexpected panic from Rust core
```

`TokenFoldError` is the catch-all for `except TokenFoldError:`. Subclasses allow specific handling. Map from Rust `TokenFoldError` variants at the pyo3 boundary.

#### 5.6 `inspect()` function (new)

The CLI has `inspect` (dry-run); the Python API had no equivalent. `inspect()` is identical to `compress()` but:
- Returns a `CompressionResult` with `payload = original_payload` (no payload modification)
- Report shows what WOULD happen if compress were called with the same arguments
- All `TransformReport` entries show predicted `tokens_after` and `savings_ratio`

This is useful for callers who want to audit compression before applying it, or log expected savings without actually compressing.

#### 5.7 Async support (v0.2 consideration)

The synchronous `compress()` ships with the v0.2 Python binding. If async is requested, add `compress_async()` via `asyncio` + `pyo3-asyncio`; do not add it to the first binding release unless a first consumer needs it.

### 6. TypeScript API

The TypeScript package is an optional v0.3+ adapter surface. It mirrors Headroom's SDK shape while preserving tokenfold report semantics.

```ts
const client = new TokenFoldClient({
  baseUrl: "http://127.0.0.1:7878",
  apiKey: process.env.TOKENFOLD_PROXY_TOKEN,
  timeoutMs: 10_000,
  retries: 1,
  fallback: "passthrough",
});

const result = await client.compress(messages, {
  model: "gpt-4.1",
  tokenBudget: 12_000,
  mode: "balanced",
});
```

Result fields: `messages`, `compressed`, `report`, `tokensBefore`, `tokensAfter`, `tokensSaved`, `savingsPct`, `transformsApplied`, `retrievalHashes`.

Adapter entry points are optional packages/functions: `withTokenFold(openaiClient)`, `withTokenFold(anthropicClient)`, Vercel AI middleware, and standalone message compression.

### 7. Stats, Dashboard, Metrics, Filters & Ledger Contracts

#### 7.1 Stats and Analytics JSON

`tokenfold stats`, `tokenfold gain`, `tokenfold session`, and `/stats` emit a stable `StatsSummary` shape under `--json` or HTTP JSON. `/stats-history` returns a series of `StatsSummary`-shaped buckets (one per interval selected by `series=`), not a single object:

```json
{
  "schema_version": "1.0",
  "scope": "project",
  "window": "30d",
  "project": "<redacted-or-hash>",
  "requests": 128,
  "commands": 94,
  "wrapped_commands": 72,
  "raw_commands": 22,
  "bypass_count": 3,
  "raw_tokens": 1200000,
  "compressed_tokens": 760000,
  "saved_tokens": 440000,
  "savings_pct": 36.7,
  "estimated_lost_tokens": 90000,
  "coverage_pct": 76.6,
  "untrusted_filter_count": 1,
  "retrieval": { "markers": 12, "hits": 9, "misses": 1, "expired": 2 },
  "cache": { "hits": 44, "misses": 18 },
  "latency": { "p50_ms": 7.4, "p95_ms": 19.8 },
  "recent_requests": [
    {
      "request_id": "tc-7f3a2b1c",
      "timestamp": "2026-07-08T12:00:00Z",
      "surface": "proxy",
      "format": "openai_json",
      "mode": "balanced",
      "status": "compressed",
      "original_tokens": 18400,
      "compressed_tokens": 11900,
      "saved_tokens": 6500,
      "savings_pct": 35.3,
      "bypass_reason": null,
      "project_hash": "sha256:..."
    }
  ]
}
```

`recent_requests[]` items use this redacted shape:

```json
{
  "request_id": "tc-7f3a2b1c",
  "timestamp": "2026-07-08T12:00:00Z",
  "surface": "proxy",
  "format": "openai_json",
  "mode": "balanced",
  "status": "compressed",
  "original_tokens": 18400,
  "compressed_tokens": 11900,
  "saved_tokens": 6500,
  "savings_pct": 35.3,
  "bypass_reason": null,
  "project_hash": "sha256:..."
}
```

No raw prompt, response, command args, file paths, or secret-bearing headers are allowed in `recent_requests`.

`/metrics` uses Prometheus naming with `tokenfold_` prefix, for example `tokenfold_saved_tokens_total`, `tokenfold_requests_total`, `tokenfold_retrieval_hits_total`, and `tokenfold_transform_duration_seconds_bucket`.

`/dashboard` is loopback-only by default. `/stats/reset` is loopback/admin-only and records a reset event in the ledger.

#### 7.2 Filter Pack Contract

Project filters live at `.tokenfold/filters.toml`; user filters live at `$XDG_CONFIG_HOME/tokenfold/filters.toml`; built-ins are compiled into the binary. Precedence is project → user → built-in.

Filter packs are TOML with:
- `schema_version`
- pack/filter `id` and `version`
- `match_command` and optional `match_output`
- stages such as `strip_ansi`, `replace`, `keep_lines`, `strip_lines`, `head`, `tail`, `max_lines`, `truncate`, and `on_empty`
- inline fixtures with input/output expectations and expected token deltas

Unknown fields are rejected. Filters cannot execute shell commands or read arbitrary files.

#### 7.3 Filter Trust Contract

Built-in filters are trusted. Project/user filters are skipped until trusted unless `TOKENFOLD_TRUST_PROJECT_FILTERS=1` is set in CI. Trust records canonical path + SHA-256 + schema version in `$XDG_DATA_HOME/tokenfold/trusted_filters.json`; content changes invalidate trust. `filters verify --require-all` is the CI contract.

#### 7.4 Ledger and State Paths

Default paths follow XDG conventions:
- Config: `$XDG_CONFIG_HOME/tokenfold/config.toml`
- Project filters: `.tokenfold/filters.toml`
- User filters: `$XDG_CONFIG_HOME/tokenfold/filters.toml`
- Trust store: `$XDG_DATA_HOME/tokenfold/trusted_filters.json`
- Ledger DB: `$XDG_DATA_HOME/tokenfold/ledger.db`
- Retrieval store: `$XDG_DATA_HOME/tokenfold/retrieve/`
- Cache: `$XDG_CACHE_HOME/tokenfold/`

Ledger data stores redacted metadata only. Raw payload capture is opt-in, retention-limited, and never enabled in proxy mode by default.

### 8. Surface Consistency Matrix

Every user-facing surface must render the same semantic information. This matrix is the source of truth for "same concept → same name":

| Concept | CLI human | `--json` field | Proxy header | Python attribute |
|---------|-----------|----------------|--------------|------------------|
| Original token count | `~18,400 est.` | `original_tokens` | `X-TokenFold-Original-Tokens` | `report.original_tokens` |
| Compressed token count | `~11,900 est.` | `compressed_tokens` | `X-TokenFold-Compressed-Tokens` | `report.compressed_tokens` |
| Savings percentage | `35.3% reduction` | `savings_pct` | `X-TokenFold-Savings-Pct` | `report.savings_pct` |
| Outcome status | `COMPRESSED ✅` | `status` | `X-TokenFold-Status` | `report.status` |
| Estimator used | `tiktoken o200k_base` | `estimator.backend + estimator.model` | `X-TokenFold-Estimator` | `report.estimator` |
| Exact vs heuristic | `~ prefix` | `estimator.is_exact` | (derived on the header: heuristic backend → not exact) | `report.estimator.is_exact` |
| Applied transforms | Transform table | `transforms[].id` where `status=applied` | `X-TokenFold-Applied` | `report.transforms` |
| Transform versions | Transform table | `transforms[].version` | `X-TokenFold-Applied-Versions` | `report.transforms[].version` |
| Active mode | Verdict/table header | `mode` | `X-TokenFold-Mode` | `report.mode` |
| Resolved format | Verdict/table header | `format` | `X-TokenFold-Format` | `report.format` |
| Request ID | Diagnostics line | `request_id` | `X-TokenFold-Request-Id` | `report.request_id` |
| Retrieval markers | Retrieval block | `retrieval` | n/a | `report.retrieval` |
| Command wrapper metadata | Command block | `command` | n/a | `report.command` |
| Warnings | WARNINGS block | `warnings[]` | (not exposed in headers) | `report.warnings` |

**Rule:** Any time a field is added to `CompressionReport`, it must be wired through to ALL surfaces in the same PR. Partial updates are not allowed. The contributing guide already covers this; this matrix makes the cross-surface wiring concrete.

### 9. Output Format Stability Policy

This section defines what "stable" means for each surface:

| Surface | Stability level | Breaking change definition |
|---------|-----------------|----------------------------|
| `CompressionReport` JSON | Stable after v0.1 | Any field removed, renamed, or type-changed |
| Proxy headers | Stable after v0.1 | Any header removed or value format changed |
| Python API | Stable after v0.2 (when binding ships) | Any function signature change, enum value rename |
| TypeScript API | Stable after v0.3 (when package ships) | Any function signature change, result-field removal/rename |
| MCP tool schemas | Stable after v0.2 | Tool removal, required input change, output field removal/rename |
| Stats/filter JSON schemas | Stable after v0.2 | Field removal/rename or type change |
| Filter pack schema | Stable after v0.2 | Stage/field removal or semantic change |
| Hook JSON protocol | Stable after v0.1 first host | Exit-code meaning or payload shape change |
| CLI human output | NOT stable (user-facing, not machine-parseable) | N/A — use `--json` for stability |
| CLI exit codes | Stable after v0.1 | Any code meaning changed |

**Non-breaking additions:** New nullable fields in `CompressionReport` (with `#[serde(skip_serializing_if = "Option::is_none")]`), new proxy headers (prefixed `X-TokenFold-`), new Python convenience functions.

**CHANGELOG.md** must document every breaking change with a migration note.

### 10. Output Format Canonical Section

This section is the canonical output-format contract. It must cover:

1. Full `CompressionReport` JSON schema with examples (human + compressed + unreachable-target cases)
2. CLI human output rendering: verdict line, transform table, warnings block format
3. Proxy header complete list with value format
4. Python `CompressionResult` / `CompressionReport` attribute reference
5. `schema_version` migration guide (what to do when version bumps)
6. Surface consistency matrix (from §8 above)
7. What is and is not stable (§9 above)

Keeping this material current inside `INTERFACES.md` is a prerequisite for v0.1 release.

## Part 2 — Transform Execution Order

This document specifies the canonical execution order of transforms within each mode. The order is load-bearing: each transform's token estimator runs against the output of the previous transform, so ordering affects both final token counts and intermediate safety validation.

This is the authoritative reference for `crates/tokenfold-core/src/modes.rs` and `tests/fixtures/mode_matrix.toml`.

### Ordering Principles

1. **`secret_redaction` always runs first** — it is a mandatory safety preprocessor, not a normal pipeline transform. It runs before any content reaches any other transform, any estimator, or any observability boundary.
2. **Lossless transforms before lossy** — lossless transforms (JSON minify, schema compaction) run before evidence-marked lossy transforms (log compaction, diff compaction). This ordering reduces the total token budget required for safe lossy operation.
3. **Higher-savings transforms before lower-savings** — within the same category, transforms that produce larger savings run earlier so that the pipeline can exit early (once the target is met, remaining transforms are skipped).
4. **Transforms that depend on structure run before structure-altering transforms** — schema compaction reads the full JSON schema structure; it must run before any transform that might alter or remove JSON structure.
5. **`table_compaction` runs after `json_minify`** — tables may be embedded in JSON payloads; minification first reduces overhead on the table parser.

### Canonical Order by Mode

#### Conservative Mode

Only lossless and semantics-preserving transforms are applied.

```
1.  secret_redaction          [MANDATORY — always first]
2.  json_minify               [lossless — highest savings, no risk]
3.  schema_compaction         [semantics-preserving — examples only]
```

`table_compaction` is not part of Conservative mode: its `conservative_enabled` flag is `false` and its `max_ratio_conservative` is `0.0` in the mode matrix, so it can never run there even if force-enabled. It first becomes eligible in Balanced/Aggressive (gated on decision D-002).

Conservative mode never applies lossy transforms. If semantics-preserving transforms improve the payload but miss the target, `Status::BestEffort` is returned. If the target is below the protected floor or no safe transform can make progress, `Status::UnreachableTarget` is returned.

#### Balanced Mode (default)

Conservative transforms run first, then fidelity-gated evidence-marked lossy transforms.

```
1.  secret_redaction          [MANDATORY — always first]
2.  json_minify               [lossless]
3.  schema_compaction         [semantics-preserving]
4.  table_compaction          [semantics-preserving, if enabled]
5.  log_compaction            [lossy w/ evidence — task-scoped: general/change_summary; requires fidelity gate green]
6.  diff_compaction           [lossy w/ evidence — task-scoped: code_review/change_summary; requires fidelity gate green]
```

Steps 5–6 are behind `--experimental` until the fidelity gate is green. They are also `task_scope`-filtered per the mode matrix (`log_compaction` → general/change_summary; `diff_compaction` → code_review/change_summary), so they are skipped for other task scopes even when experimental transforms are enabled.

#### Aggressive Mode

Balanced transforms run first, then promoted task-scoped lossy transforms.

```
1.  secret_redaction          [MANDATORY — always first]
2.  json_minify               [lossless]
3.  schema_compaction         [semantics-preserving; more aggressive ratio cap]
4.  table_compaction          [semantics-preserving, if enabled]
5.  log_compaction            [lossy w/ evidence — task-scoped: general/change_summary]
6.  diff_compaction           [lossy w/ evidence — task-scoped: code_review/change_summary]
7.  prose_extraction          [lossy w/ evidence — task-scoped; v0.2+]
8.  code_digest               [lossy w/ evidence — task-scoped; v0.2+, requires feature flag]
9.  conversation              [policy-driven — v0.2+]
```

Steps 7–9 are v0.2+ and require fidelity approval for the relevant `TaskScope`.

### Early Exit

The pipeline exits as soon as `compressed_tokens ≤ target_tokens`. Remaining transforms are skipped and marked with `skipped_reason: "target_already_met"` in `TransformReport`.

This means the order above is also the **priority order** — higher-priority (earlier) transforms run before lower-priority ones when budget is tight.

### Format-Specific Considerations

Not all transforms apply to all `InputFormat` values. The pipeline selects the applicable ordered subset:

| Transform | `OpenAiJson` | `AnthropicJson` | `PlainText` | `CommandOutput` | `GitDiff` |
|-----------|-------------|----------------|------------|----------------|-----------|
| `secret_redaction` | ✅ | ✅ | ✅ | ✅ | ✅ |
| `json_minify` | ✅ | ✅ | — | — | — |
| `schema_compaction` | ✅ (tool schemas) | ✅ (tool schemas) | — | — | — |
| `table_compaction` | ✅ | ✅ | ✅ | — | — |
| `log_compaction` | — | — | ✅ | ✅ | — |
| `diff_compaction` | — | — | ✅ | ✅ | ✅ |
| `prose_extraction` | — | — | ✅ | — | — |
| `code_digest` | — | — | ✅ | — | — |
| `conversation` | ✅ | ✅ | — | — | — |

A transform not applicable to the input format is skipped with `skipped_reason: "not_applicable_to_format"`.

#### `InputFormat::Auto` Detection

`InputFormat::Auto` is resolved before mode-matrix lookup. Detection is deliberately conservative:

1. Parse as JSON. If the top-level object has Anthropic-style content-blocks, `system` plus `messages`, or other Anthropic request shape, resolve to `AnthropicJson`.
2. If the top-level object has OpenAI-style `messages` with `role`/`content` strings, `tools` with OpenAI function/tool schema shape, or OpenAI request fields such as `model` plus `messages`, resolve to `OpenAiJson`.
3. If text starts with unified diff markers (`diff --git`, `--- ` + `+++ `, or `@@` hunks), resolve to `GitDiff`.
4. If input comes from `tokenfold wrap` (or its `shell` alias), resolve to `CommandOutput` unless the user overrides `--format`.
5. Otherwise resolve to `PlainText`.

Ambiguous valid JSON remains JSON-like and uses only generic JSON-safe transforms; malformed JSON falls through to text detection. Bare `messages` or `tools` keys alone are not enough to classify provider format. Reports include the detected format in `X-TokenFold-Format` and report metadata.

### `modes.rs` Structure

The mode matrix is encoded as a table in `crates/tokenfold-core/src/modes.rs`. This is the source of truth; `tests/fixtures/mode_matrix.toml` mirrors it for cross-surface testing.

```rust
// crates/tokenfold-core/src/modes.rs

use crate::{transforms::TransformId, CompressionMode, CompressionPolicy, InputFormat};

#[derive(Debug, Clone)]
pub struct ModeEntry {
    pub transform_id: TransformId,
    pub conservative_enabled: bool,
    pub balanced_enabled: bool,
    pub aggressive_enabled: bool,
    pub experimental: bool,
    pub max_ratio_conservative: f64,
    pub max_ratio_balanced: f64,
    pub max_ratio_aggressive: f64,
    pub task_scopes: &'static [TaskScope],
    pub version: &'static str,    // pinned transform version
}

impl ModeEntry {
    pub fn enabled_for(&self, mode: CompressionMode) -> bool {
        match mode {
            CompressionMode::Conservative => self.conservative_enabled,
            CompressionMode::Balanced => self.balanced_enabled,
            CompressionMode::Aggressive => self.aggressive_enabled,
        }
    }

    pub fn max_ratio_for(&self, mode: CompressionMode) -> f64 {
        match mode {
            CompressionMode::Conservative => self.max_ratio_conservative,
            CompressionMode::Balanced => self.max_ratio_balanced,
            CompressionMode::Aggressive => self.max_ratio_aggressive,
        }
    }
}

/// Returns the ordered transform pipeline for a given (mode, format) pair.
/// Order is canonical per this document; format filters the applicable set.
pub fn pipeline_for(policy: &CompressionPolicy, format: InputFormat, experimental: bool, enabled_ids: &[String]) -> Vec<ModeEntry> {
    ALL_ENTRIES
        .iter()
        .filter(|e| e.enabled_for(policy.mode) || (experimental && e.experimental) || ((!e.experimental || experimental) && enabled_ids.iter().any(|id| id == e.transform_id.as_str())))
        .filter(|e| !policy.disabled.iter().any(|id| id == e.transform_id.as_str()))
        .filter(|e| e.task_scopes.contains(&TaskScope::All) || e.task_scopes.contains(&policy.task_scope))
        .filter(|e| applies_to_format(e.transform_id, format))
        .cloned()
        .collect()
}

// Canonical ordered table (order here IS the execution order)
static ALL_ENTRIES: &[ModeEntry] = &[
    // secret_redaction is not in this table — it runs unconditionally before the pipeline.
    ModeEntry { transform_id: TransformId::JsonMinify,        conservative_enabled: true,  balanced_enabled: true,  aggressive_enabled: true,  experimental: false, max_ratio_conservative: 1.0, max_ratio_balanced: 1.0,  max_ratio_aggressive: 1.0,  task_scopes: &[TaskScope::All], version: "1.0.0" },
    ModeEntry { transform_id: TransformId::SchemaCompaction, conservative_enabled: true,  balanced_enabled: true,  aggressive_enabled: true,  experimental: false, max_ratio_conservative: 0.15, max_ratio_balanced: 0.30, max_ratio_aggressive: 0.50, task_scopes: &[TaskScope::All], version: "1.0.0" },
    ModeEntry { transform_id: TransformId::TableCompaction,  conservative_enabled: false, balanced_enabled: false, aggressive_enabled: false, experimental: false, max_ratio_conservative: 0.0, max_ratio_balanced: 0.40, max_ratio_aggressive: 0.50, task_scopes: &[TaskScope::All], version: "1.0.0" },
    ModeEntry { transform_id: TransformId::LogCompaction,    conservative_enabled: false, balanced_enabled: false, aggressive_enabled: false, experimental: true,  max_ratio_conservative: 0.0, max_ratio_balanced: 0.65, max_ratio_aggressive: 0.75, task_scopes: &[TaskScope::General, TaskScope::ChangeSummary], version: "1.0.0" },
    ModeEntry { transform_id: TransformId::DiffCompaction,   conservative_enabled: false, balanced_enabled: false, aggressive_enabled: false, experimental: true,  max_ratio_conservative: 0.0, max_ratio_balanced: 0.60, max_ratio_aggressive: 0.70, task_scopes: &[TaskScope::CodeReview, TaskScope::ChangeSummary], version: "1.0.0" },
    // v0.2+ entries added here after fidelity approval
];
```

The `max_ratio` values are draft placeholders. They are updated to the validated safe-default per-transform ratio bands produced by the Phase 2 accuracy@ratio curves; each band must satisfy the fidelity-gate quality thresholds defined in `ROADMAP.md § D-005` (D-005 sets the quality thresholds, not the per-transform ratio values themselves — the per-transform default ratio is tracked as PLAN.md Open Decision #8).

`TransformId::as_str()` returns the canonical snake_case transform ID used by CLI flags, config, reports, and headers. `policy.disabled` and `enabled_ids` contain those canonical IDs.

### Mode Matrix Fixture Format

`tests/fixtures/mode_matrix.toml` mirrors the Rust table for cross-surface testing:

```toml
# tests/fixtures/mode_matrix.toml
# Mirror of crates/tokenfold-core/src/modes.rs ALL_ENTRIES.
# CLI, proxy, and Python binding tests assert against this file.

[[transforms]]
id = "json_minify"
version = "1.0.0"
conservative_enabled = true
balanced_enabled = true
aggressive_enabled = true
experimental = false
max_ratio_conservative = 1.0
max_ratio_balanced = 1.0
max_ratio_aggressive = 1.0
applicable_formats = ["openai_json", "anthropic_json"]

[[transforms]]
id = "schema_compaction"
version = "1.0.0"
conservative_enabled = true
balanced_enabled = true
aggressive_enabled = true
experimental = false
max_ratio_conservative = 0.15
max_ratio_balanced = 0.30
max_ratio_aggressive = 0.50
applicable_formats = ["openai_json", "anthropic_json"]

[[transforms]]
id = "table_compaction"
version = "1.0.0"
conservative_enabled = false
balanced_enabled = false     # enabled only if D-002 names tables as top-three payload type
aggressive_enabled = false
experimental = false
max_ratio_conservative = 0.0
max_ratio_balanced = 0.40
max_ratio_aggressive = 0.50
applicable_formats = ["openai_json", "anthropic_json", "plain_text"]

[[transforms]]
id = "log_compaction"
version = "1.0.0"
conservative_enabled = false
balanced_enabled = false     # true after fidelity gate green
aggressive_enabled = false   # true after fidelity gate green
experimental = true          # removed once fidelity gate green
max_ratio_conservative = 0.0
max_ratio_balanced = 0.65    # draft; updated after Phase 2
max_ratio_aggressive = 0.75  # draft; updated after Phase 2
applicable_formats = ["plain_text", "command_output"]

[[transforms]]
id = "diff_compaction"
version = "1.0.0"
conservative_enabled = false
balanced_enabled = false
aggressive_enabled = false
experimental = true
max_ratio_conservative = 0.0
max_ratio_balanced = 0.60
max_ratio_aggressive = 0.70
applicable_formats = ["plain_text", "command_output", "git_diff"]
```

### Safety Invariants on Order

These invariants are asserted by `safety.rs` after each transform:

1. **JSON validity**: After any JSON transform, `serde_json::from_slice` on the output must succeed.
2. **Key order preservation**: After any JSON transform, the key order of all objects in the output must be identical to the key order in the input.
3. **Protected content**: After any transform, all protected content (system turns, latest user message, tool descriptions in conservative mode) must be present in the output.
4. **No redaction bypass**: `secret_redaction` output must not contain any pattern that matched in the original (best-effort; `UnredactedContentPossible` is emitted if the engine cannot confirm).

If any invariant is violated after a transform, that transform is **rolled back** (pre-transform bytes restored), a `SafetyDowngrade` warning is emitted, and the pipeline continues with the next transform.

## Part 3 — Configuration Reference (tokenfold.toml)

`tokenfold.toml` (or `.tokenfoldrc`) is the file-based configuration layer. It sits below CLI flags and environment variables in the precedence chain:

```
flags > environment variables > tokenfold.toml / .tokenfoldrc > built-in defaults
```

The config file is **optional**. Built-in defaults are safe for all use cases. The config file is useful for setting project-specific or machine-specific preferences that you don't want to retype on every invocation.

### File Discovery

`tokenfold` searches for a config file in this order:

1. Path in `TOKENFOLD_CONFIG` environment variable (if set)
2. `./tokenfold.toml` (current working directory)
3. `./.tokenfoldrc` (current working directory)
4. `$XDG_CONFIG_HOME/tokenfold/config.toml` (falls back to `$HOME/.config/tokenfold/config.toml` when `XDG_CONFIG_HOME` is unset)
5. `$HOME/.tokenfoldrc`

The first file found is used. Files are not merged.

**Note:** `tokenfold.toml` in the project root is in `.gitignore` by default (machine-specific settings should not be committed). To commit a shared project config, use `.tokenfold.project.toml` (not yet implemented — tracked as a future feature).

### Full Schema

```toml
# tokenfold.toml — full schema with all fields and their defaults

# --------------------------------------------------
# [compression] — default compression behavior
# --------------------------------------------------
[compression]

# CLI flag: --mode
mode = "balanced"

# Default target token count (no default = compress to safe floor)
# CLI flag: --target-tokens
# target_tokens = 12000

# Token budget reserved for model output (not deducted from input budget)
# Useful when you know the model needs N output tokens.
reserve_output_tokens = 0

# auto = infer from content (heuristic). CLI flag: --format
format = "auto"

# Task scope used to decide whether task-scoped lossy transforms are legal:
# "all" | "general" | "code_review" | "change_summary" | "debugging" |
# "generation" | "api_overview" | "retrieval_qa" | "agent_history"
# CLI flag: --task-scope
task_scope = "general"

# Whether to preserve the latest user message byte-for-byte (always recommended)
preserve_latest_user_message = true

# Transforms to disable by default (comma-separated canonical IDs).
# Cannot include "secret_redaction" (use unsafe_disable_redaction instead).
# CLI flag: --disable
# disabled = ["table_compaction"]
disabled = []

# Enable all currently experimental transforms that are present in the mode matrix.
# CLI flag: --experimental
experimental = false

# Named transform opt-ins. Experimental transforms also require experimental=true.
# CLI flag: --enable
enable = []

# Prompt-cache boundary protection. Examples: "byte:4096", "turn:3".
# CLI flag: --cache-boundary
# cache_boundary = "turn:3"


# --------------------------------------------------
# [estimator] — token counting behavior
# --------------------------------------------------
[estimator]

# Whether to allow heuristic (bytes/4) counting for budget decisions.
# false = fail closed if exact backend unavailable.
# true = proceed with heuristic; report labels it as "heuristic:bytes/4" and prefixes ~.
# CLI flag: --allow-heuristic-budget
allow_heuristic_budget = false

# Timeout for exact estimator API calls (seconds).
exact_backend_timeout_secs = 10

# Path to a cached exact-count file for CI/offline use.
# The file is a JSON map of { "<sha256 of input>": token_count }.
# exact_count_cache = ".tokenfold-exact-count-cache.json"


# --------------------------------------------------
# [output] — CLI display behavior
# --------------------------------------------------
[output]

# Also honored via NO_COLOR env var.
no_color = false

quiet = false

# Report format. See INTERFACES.md §1.3 for stream routing.
json = false

# Number of decimal places for savings percentages in human output.
savings_precision = 1

# Do not truncate long transform names or table cells in human output.
# CLI flag: --no-truncate
no_truncate = false


# --------------------------------------------------
# [safety] — safety and redaction behavior
# --------------------------------------------------
[safety]

# DANGER: Disable secret redaction entirely.
# This is a CLI-only escape hatch. It is NEVER allowed in proxy mode.
# It emits a Critical warning and writes an audit event.
# Use only for testing with known-safe synthetic fixtures.
# Equivalent to CLI flag: --unsafe-disable-redaction
unsafe_disable_redaction = false

# Maximum input size in bytes before the compression is rejected.
# Protects against OOM on very large inputs.
# 0 = no limit (not recommended in proxy mode).
max_input_bytes = 10_485_760   # 10 MB default

# Maximum JSON nesting depth accepted by JSON transforms.
# Protects against stack/allocator pressure from malicious JSON.
max_json_depth = 256


# --------------------------------------------------
# [retrieval] — reversible evidence store
# --------------------------------------------------
[retrieval]

# Store originals for retrieval markers by default.
store_originals = false

# Retrieval store namespace/scope: "session" | "project" | "user".
namespace = "project"

# TTL for stored originals in seconds.
ttl_seconds = 3600

# Maximum store size in bytes before GC is required.
max_store_bytes = 268_435_456

# Store backend: "memory" | "sqlite" | "filesystem".
backend = "filesystem"

# Optional store path. Defaults to XDG data path.
# store_path = "$XDG_DATA_HOME/tokenfold/retrieve"


# --------------------------------------------------
# [output_savings] — optional output-token policy layer
# --------------------------------------------------
[output_savings]

# "none" | "terse" | "standard"
profile = "none"

# Fraction of proxy requests held out for measured output-savings comparison.
holdout_rate = 0.0


# --------------------------------------------------
# [transforms] — per-transform overrides
# --------------------------------------------------

[transforms.schema_compaction]
# Number of examples to keep per schema field (in balanced mode).
max_examples_balanced = 1
# Number of examples to keep per schema field (in aggressive mode).
max_examples_aggressive = 1

[transforms.log_compaction]
# Remove timestamps from log lines before deduplication.
# Default off: timestamp removal changes the log's evidentiary record.
remove_timestamps = false

[transforms.diff_compaction]
# Keep the changed line bodies (+/-) in the output.
# Set to false only for TaskScope::ChangeSummary (change-summary consumers).
keep_line_bodies = true


# --------------------------------------------------
# [proxy] — proxy-mode settings (tokenfold-proxy only)
# --------------------------------------------------
[proxy]

# Address to bind the proxy listener.
listen = "127.0.0.1:7878"

# Upstream LLM API base URL.
# upstream = "https://api.openai.com"

# Maximum request body size to buffer (bytes).
# Requests larger than this are passed through uncompressed.
max_body_bytes = 10_485_760  # 10 MB default

# Require TLS on upstream connections. Never set to false in production.
require_upstream_tls = true

# CLI flag: --insecure-upstream. Allows http:// upstreams only when explicitly set.
# Startup fails if this is true and any credential-forwarding mode is enabled in production config.
allow_insecure_upstream = false

# Require an explicit opt-in before binding anything other than loopback.
allow_non_loopback_bind = false

# Optional bearer token for non-loopback proxy callers.
# Also accepted via X-TokenFold-Proxy-Token.
# proxy_token = ""

# Default project attribution when X-TokenFold-Project or /p/<project> is absent.
# project = "default"


# --------------------------------------------------
# [analytics] — ledger, stats, discover/session
# --------------------------------------------------
[analytics]

enabled = true
ledger_db = "$XDG_DATA_HOME/tokenfold/ledger.db"
retention_days = 90
hash_project_paths = true


# --------------------------------------------------
# [filters] — declarative command-output filters
# --------------------------------------------------
[filters]

enabled = true
project_filters = ".tokenfold/filters.toml"
user_filters = "$XDG_CONFIG_HOME/tokenfold/filters.toml"
trust_store = "$XDG_DATA_HOME/tokenfold/trusted_filters.json"
trust_project_filters = false


# --------------------------------------------------
# [update] — signed internal updates
# --------------------------------------------------
[update]

channel = "stable"
check_on_run = false
allow_prerelease = false


# --------------------------------------------------
# [benchmark] — benchmark-mode settings
# --------------------------------------------------
[benchmark]

# Default format for benchmark fixtures.
# Can be overridden per-fixture with a --format flag.
format = "openai_json"

# Whether to emit benchmark output as JSON.
json = false
```

### Minimal Example

```toml
# tokenfold.toml — minimal project config
[compression]
mode = "balanced"
target_tokens = 12000
format = "openai_json"
```

### Environment Variable Overrides

Every config file field has a corresponding environment variable. Variable names follow the pattern `TOKENFOLD_<SECTION>_<FIELD>` in `SCREAMING_SNAKE_CASE`, **except** for three intentionally shortened names (marked below); the table is authoritative wherever it diverges from the pattern (this also disambiguates the shared `TOKENFOLD_OUTPUT_SAVINGS_*` prefix between the `[output]` and `[output_savings]` sections):

| Config field | Environment variable |
|--------------|----------------------|
| `compression.mode` | `TOKENFOLD_COMPRESSION_MODE` |
| `compression.target_tokens` | `TOKENFOLD_COMPRESSION_TARGET_TOKENS` |
| `compression.reserve_output_tokens` | `TOKENFOLD_COMPRESSION_RESERVE_OUTPUT_TOKENS` |
| `compression.format` | `TOKENFOLD_COMPRESSION_FORMAT` |
| `compression.task_scope` | `TOKENFOLD_COMPRESSION_TASK_SCOPE` |
| `compression.preserve_latest_user_message` | `TOKENFOLD_COMPRESSION_PRESERVE_LATEST_USER_MESSAGE` |
| `compression.disabled` | `TOKENFOLD_COMPRESSION_DISABLED` (comma-separated) |
| `compression.experimental` | `TOKENFOLD_COMPRESSION_EXPERIMENTAL` |
| `compression.enable` | `TOKENFOLD_COMPRESSION_ENABLE` (comma-separated) |
| `compression.cache_boundary` | `TOKENFOLD_COMPRESSION_CACHE_BOUNDARY` |
| `estimator.allow_heuristic_budget` | `TOKENFOLD_ESTIMATOR_ALLOW_HEURISTIC_BUDGET` |
| `estimator.exact_backend_timeout_secs` | `TOKENFOLD_ESTIMATOR_EXACT_BACKEND_TIMEOUT_SECS` |
| `estimator.exact_count_cache` | `TOKENFOLD_ESTIMATOR_EXACT_COUNT_CACHE` |
| `output.no_color` | `TOKENFOLD_OUTPUT_NO_COLOR` or `NO_COLOR` (standard) |
| `output.quiet` | `TOKENFOLD_OUTPUT_QUIET` |
| `output.json` | `TOKENFOLD_OUTPUT_JSON` |
| `output.savings_precision` | `TOKENFOLD_OUTPUT_SAVINGS_PRECISION` |
| `output.no_truncate` | `TOKENFOLD_OUTPUT_NO_TRUNCATE` |
| `safety.unsafe_disable_redaction` | `TOKENFOLD_SAFETY_UNSAFE_DISABLE_REDACTION` |
| `safety.max_input_bytes` | `TOKENFOLD_SAFETY_MAX_INPUT_BYTES` |
| `safety.max_json_depth` | `TOKENFOLD_SAFETY_MAX_JSON_DEPTH` |
| `retrieval.store_originals` | `TOKENFOLD_RETRIEVAL_STORE_ORIGINALS` |
| `retrieval.namespace` | `TOKENFOLD_RETRIEVAL_NAMESPACE` |
| `retrieval.ttl_seconds` | `TOKENFOLD_RETRIEVAL_TTL_SECONDS` |
| `retrieval.max_store_bytes` | `TOKENFOLD_RETRIEVAL_MAX_STORE_BYTES` |
| `retrieval.backend` | `TOKENFOLD_RETRIEVAL_BACKEND` |
| `retrieval.store_path` | `TOKENFOLD_RETRIEVAL_STORE_PATH` |
| `output_savings.profile` | `TOKENFOLD_OUTPUT_SAVINGS_PROFILE` |
| `output_savings.holdout_rate` | `TOKENFOLD_OUTPUT_SAVINGS_HOLDOUT_RATE` |
| `transforms.schema_compaction.max_examples_balanced` | `TOKENFOLD_TRANSFORMS_SCHEMA_COMPACTION_MAX_EXAMPLES_BALANCED` |
| `transforms.schema_compaction.max_examples_aggressive` | `TOKENFOLD_TRANSFORMS_SCHEMA_COMPACTION_MAX_EXAMPLES_AGGRESSIVE` |
| `transforms.log_compaction.remove_timestamps` | `TOKENFOLD_TRANSFORMS_LOG_COMPACTION_REMOVE_TIMESTAMPS` |
| `transforms.diff_compaction.keep_line_bodies` | `TOKENFOLD_TRANSFORMS_DIFF_COMPACTION_KEEP_LINE_BODIES` |
| `proxy.listen` | `TOKENFOLD_PROXY_LISTEN` |
| `proxy.upstream` | `TOKENFOLD_PROXY_UPSTREAM` |
| `proxy.max_body_bytes` | `TOKENFOLD_PROXY_MAX_BODY_BYTES` |
| `proxy.require_upstream_tls` | `TOKENFOLD_PROXY_REQUIRE_UPSTREAM_TLS` |
| `proxy.allow_insecure_upstream` | `TOKENFOLD_PROXY_ALLOW_INSECURE_UPSTREAM` |
| `proxy.allow_non_loopback_bind` | `TOKENFOLD_PROXY_ALLOW_NON_LOOPBACK_BIND` |
| `proxy.proxy_token` | `TOKENFOLD_PROXY_TOKEN` (shortened — not `TOKENFOLD_PROXY_PROXY_TOKEN`) |
| `proxy.project` | `TOKENFOLD_PROXY_PROJECT` |
| `analytics.enabled` | `TOKENFOLD_ANALYTICS_ENABLED` |
| `analytics.ledger_db` | `TOKENFOLD_LEDGER_DB` (shortened — not `TOKENFOLD_ANALYTICS_LEDGER_DB`) |
| `analytics.retention_days` | `TOKENFOLD_ANALYTICS_RETENTION_DAYS` |
| `analytics.hash_project_paths` | `TOKENFOLD_ANALYTICS_HASH_PROJECT_PATHS` |
| `filters.enabled` | `TOKENFOLD_FILTERS_ENABLED` |
| `filters.project_filters` | `TOKENFOLD_FILTERS_PROJECT_FILTERS` |
| `filters.user_filters` | `TOKENFOLD_FILTERS_USER_FILTERS` |
| `filters.trust_store` | `TOKENFOLD_FILTERS_TRUST_STORE` |
| `filters.trust_project_filters` | `TOKENFOLD_TRUST_PROJECT_FILTERS` (shortened — not `TOKENFOLD_FILTERS_TRUST_PROJECT_FILTERS`) |
| `update.channel` | `TOKENFOLD_UPDATE_CHANNEL` |
| `update.check_on_run` | `TOKENFOLD_UPDATE_CHECK_ON_RUN` |
| `update.allow_prerelease` | `TOKENFOLD_UPDATE_ALLOW_PRERELEASE` |
| `benchmark.format` | `TOKENFOLD_BENCHMARK_FORMAT` |
| `benchmark.json` | `TOKENFOLD_BENCHMARK_JSON` |

`NO_COLOR` (without prefix) is also honored per the [no-color.org](https://no-color.org) standard.

Boolean environment variables accept `1`, `true`, `yes`, `on` and `0`, `false`, `no`, `off` case-insensitively. Empty boolean values are invalid. `TOKENFOLD_DISABLED=1` and `TOKENFOLD_PASSTHROUGH=1` are global emergency bypasses: they skip compression and report `bypass.reason="env"` (the `reason` field of `CompressionReport.bypass`).

### Validation

`tokenfold` validates the config file at startup and emits a clear error for:
- Unknown field names (typos)
- Invalid enum values (e.g., `mode = "turbo"`)
- Out-of-range numeric values (e.g., `max_input_bytes = -1`)
- Conflicting settings (e.g., `disabled = ["secret_redaction"]`)
- Empty or duplicate transform IDs in `compression.disabled` / `TOKENFOLD_COMPRESSION_DISABLED`; surrounding whitespace is trimmed, empty entries are rejected, and duplicates are de-duplicated with a warning

Invalid config exits with code 5 (`ConfigError`). Invalid CLI flags remain exit 2 (`InvalidInput`).

### Config File in CI

For CI environments, prefer environment variables over a committed config file:

```bash
# .github/workflows/compress.yml (example)
env:
  TOKENFOLD_COMPRESSION_MODE: balanced
  TOKENFOLD_COMPRESSION_TARGET_TOKENS: 12000
  TOKENFOLD_COMPRESSION_FORMAT: openai_json
  TOKENFOLD_ESTIMATOR_ALLOW_HEURISTIC_BUDGET: "false"
```

If a config file is needed in CI (e.g., to share benchmark settings across runs), commit it as `tokenfold.ci.toml` and pass `TOKENFOLD_CONFIG=tokenfold.ci.toml` in the CI environment — don't commit a plain `tokenfold.toml` that overrides developer preferences.

