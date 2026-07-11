# tokenfold — Engineering (Testing · CI/CD · Risks · Contributing)

This is one of five canonical docs — the single reference for how the project is built, tested, hardened, and contributed to. It merges the testing strategy, the CI/CD pipeline, the risk register, and the contributing guide into one place so that engine and ML/research contributors share the same source of truth for quality gates, release mechanics, known risks, and the prototype-then-port workflow.

## Part 1 — Testing Strategy

Testing strategy for all crates and surfaces. Every layer has a type, scope, required fixtures, and a pass criterion. The required test list — not a coverage percentage — is the contract.

---

### Test Layer Overview

| Layer | Tool | Scope | CI trigger | Blocking |
|-------|------|-------|-----------|----------|
| Unit | `cargo test` (inline `#[cfg(test)]`) | Per-module, per-transform | All PRs + merge + release | Yes |
| Property-based | `proptest` | Transform invariants | All PRs + merge + release | Yes |
| Golden (byte-exact) | `cargo test` + fixture files | Cross-platform output | All PRs + merge + release | Yes |
| Integration | `cargo test` (workspace) | Pipeline end-to-end | All PRs + merge + release | Yes |
| CLI end-to-end | `cargo test -p tokenfold-cli` | Documented CLI examples | All PRs + merge + release | Yes |
| Benchmark smoke | `cargo bench` | Gross regression detection | Merge to main + release | Non-blocking on merge; blocking on release |
| Benchmark regression | `cargo bench` + threshold file | Full regression against baselines | Release only | Blocking |
| Fidelity smoke | Python harness | Downstream quality, first-consumer fixtures | Merge to main + release | Blocking on release; advisory on merge |
| Fidelity full | Python harness | All transforms, all content types | Release only (lossy-enabled releases) | Blocking |
| Hook/MCP/retrieval | `cargo test` + fixture host configs | Agent integration, MCP tools, local evidence store | All PRs touching parity surfaces | Yes |
| Stats/filter contracts | `cargo test` + fixture reports/filters | Ledger safety, JSON/CSV stability, command-filter verification | All PRs touching parity surfaces | Yes |
| Optional extension gates | Package-specific tests | Adapters, RAG/vector, output shaping, image/multimodal, learn/update | Releases including those extensions | Blocking for that extension |

---

### Unit Tests

**Location:** `crates/tokenfold-core/src/**/*.rs` (inline `#[cfg(test)]` modules)

Required tests per module:

#### `report.rs`
- `savings_ratio` math: `f64::EPSILON`-tolerant compare (or compare rounded to 4 decimal places)
- Zero `original_tokens` edge case: `savings_ratio` = 0.0, no divide-by-zero
- `CompressionReport::new` constructor: `saved_tokens = original - compressed` (saturating)
- `QualityReport` serialization round-trip (all fields preserved)
- `TransformReport` serialization round-trip

#### `status.rs`
- All `Status` variants serialize/deserialize round-trip (`serde_json`)
- `Status::UnreachableTarget` serializes as the bare string `"unreachable_target"`; achieved/floor details are preserved in `BudgetReport`
- `Status::Compressed`, `Passthrough`, `BestEffort` each serialize to a distinct JSON representation

#### `errors.rs`
- Each `TokenFoldError` variant → expected CLI exit code (property: mapping is exhaustive)
- Each `TokenFoldError` variant → expected proxy HTTP status code
- `TokenFoldError::from(std::io::Error)` works (via `thiserror` `#[from]`)

#### `token_estimator.rs`
- `ByteHeuristicEstimator::count_bytes`: rounds up (`div_ceil(4)`)
- `ByteHeuristicEstimator::count_bytes(b"")` returns 0
- `ByteHeuristicEstimator::count_bytes` for a 1-byte input returns 1 (not 0)
- Exact backends: golden fixture produces known count (using cached exact count or deterministic mock)
- `TiktokenEstimator::name()` returns a string starting with `tiktoken:`
- `AnthropicApiEstimator` is not used in unit tests (mock estimator injected instead)
- `compress_with_estimator` accepts a mock estimator that always returns a fixed count

#### `budget.rs`
- `protected_floor` ≤ `original_tokens` always
- `protected_floor` for an OpenAI input with a system message = at least the tokens of the system message
- `protected_floor` for a plain text input = at least the tokens of the last user message
- `Passthrough` status when `original_tokens ≤ target_tokens`
- `UnreachableTarget` status when `target < protected_floor`
- `default mode` is `Balanced` (verify `CompressionPolicy::default()`)
- `disabled` list cannot include `secret_redaction` (builder returns `Err` if attempted)

#### `transforms/json.rs`
- Valid JSON → minified → re-parseable (semantically equal after parse)
- Key order preserved byte-for-byte: `{"b":1,"a":2}` → `{"b":1,"a":2}` (not `{"a":2,"b":1}`)
- String content preserved byte-for-byte including escapes
- Duplicate-key fixture: behavior documented; no silent key drop
- Non-UTF-8 input → `InvalidInput` error
- Empty byte slice → empty output (no error, no panic)
- Unterminated string → `JsonMinifyError::Invalid(...)`; the lexical whitespace pass runs only after `serde_json` validation
- `minify(minify(x)) == minify(x)` (byte-identical idempotency)

#### `transforms/logs.rs`
- Adjacent duplicates collapsed: `[A, A, A, A]` → `[A, [repeated 4x], A]`
- Non-adjacent duplicates preserved: `[A, B, A]` → `[A, B, A]` (undocumented limitation test)
- Single line → unchanged
- Empty input → empty output
- Two identical lines → `[A, A]` (no collapse; marker only for runs of 3+)
- Three identical lines → `[A, [repeated 3x], A]`

#### `transforms/schema.rs`
- `description` field of a tool preserved byte-for-byte in Conservative mode
- `required` array preserved
- `enum` array preserved
- `type` field preserved
- `default` field preserved
- `examples` array shortened to the configured count
- Tool `name` field preserved

#### `transforms/redaction.rs`
- JWT token pattern redacted
- OpenAI API key pattern (`sk-...`) redacted
- AWS access key pattern (`AKIA...`) redacted
- Basic-auth-in-URL redacted (`https://user:pass@host`)
- Bearer token in `Authorization` header value redacted
- `--disable secret_redaction` → `Err(TokenFoldError::InvalidInput(...))` or equivalent rejected policy
- `unsafe_disable_redaction = true` → audit event emitted (check for `Warning { code: UnredactedContentPossible, severity: Critical }`)
- `UnredactedContentPossible` warning is present in output whenever redaction runs
- ReDoS canary fixture: a crafted catastrophic-backtracking input completes within a bounded wall-clock budget (proves the linear-time `regex` engine is on the redaction path)
- Proxy integration: a request carrying both `Content-Length` and `Transfer-Encoding` (CL.TE / TE.CL) is rejected with `400` before any upstream buffering
- Proxy startup: `unsafe_disable_redaction = true` in any config source makes the proxy refuse to start and exit non-zero

#### `pipeline.rs`
- `UnreachableTarget` fixture: target set below floor; protected content survives; `Status::UnreachableTarget` in report
- `Passthrough` when `input_tokens ≤ target_tokens`: output bytes identical to input bytes
- Safety rollback: a transform configured to produce invalid JSON is rejected; `SafetyDowngrade` warning emitted; pre-transform bytes returned
- Multi-transform pipeline: each transform's `TransformReport` is present in `CompressionReport.transforms`

#### `safety.rs`
- JSON still valid after each JSON transform (re-parseable post-transform)
- System turn preserved after any pipeline run on a conversation
- Required schema fields preserved (use a fixture with `required: ["id"]`)
- Prompt-cache prefix is byte-identical before `cache_boundary`
- Provider cache-control blocks are protected and reported in `CacheReport`
- Retrieval markers never expose secret values and are absent when persistence is disabled

#### `retrieval_store.rs` (v0.2 parity surface)
- Stores original spans by content hash and namespace
- `retrieve` restores exact original bytes from marker/report fixtures
- Expired or missing entries return a retrieval error with no partial silent output
- TTL and max-size garbage collection remove only eligible entries
- Unsafe raw-secret persistence flag is rejected in proxy mode

#### `stats.rs` (v0.2 parity surface)
- Aggregates fixture `CompressionReport` files by transform, format, estimator, status, and project
- JSON and CSV outputs are schema-stable and contain no raw payload bytes
- Cost savings are labeled `measured`, `estimated`, or `heuristic`
- Ledger retention cleanup deletes old metadata without touching active records

#### `filters.rs` (v0.2 parity surface)
- Declarative filter schema validation rejects unknown fields and unsafe regex settings
- Inline fixture tests pass for every built-in filter
- Precedence order is project → user → built-in, with deterministic conflict resolution
- `never_worse` returns raw output when filtered output saves no tokens
- Custom filters cannot execute shell commands or read arbitrary files

#### `hooks.rs` (v0.1 first host → v0.2 full)
- `init` writes a managed block and byte-restorable backup for each supported host
- `init` is idempotent and never duplicates hook entries
- `uninit` restores the original config byte-for-byte when no unrelated host edits occurred
- `doctor` reports missing hook, stale binary path, unavailable estimator, and disabled stores accurately
- Malformed host config fails closed with a clear repair suggestion

---

### Property-Based Tests

**Location:** `crates/tokenfold-core/tests/property_tests.rs`
**Crate:** `proptest`

| Property | Transform | Invariant |
|----------|-----------|-----------|
| JSON round-trip | `json_minify` | `parse(minify(valid_json))` is semantically equal to `parse(valid_json)` |
| JSON key order | `json_minify` | Key order in minified output equals key order in input (for any valid JSON object) |
| JSON idempotency | `json_minify` | `minify(minify(x)) == minify(x)` byte-identical |
| Log idempotency | `log_compaction` | `compact(compact(x))` produces no further token reduction |
| Redaction idempotency | `secret_redaction` | `redact(redact(x)) == redact(x)` |
| Savings non-negative | pipeline | `saved_tokens ≥ 0` for all valid inputs |
| Floor ≤ original | budget | `protected_floor ≤ original_tokens` for all valid inputs |
| Best-effort ≥ floor | pipeline | When `UnreachableTarget`, `achieved ≥ floor` |
| Safety: JSON valid post-transform | pipeline | After any JSON transform, output is valid JSON |

---

### Golden Tests (Byte-Exact)

**Location:** `tests/golden/`
**Purpose:** Catch silent output changes across platforms and transform version bumps.

#### Requirements
- Every transform must have at least one golden fixture pair: named input → expected output.
- Golden files are **versioned and immutable**. Updating requires `REGENERATE_GOLDENS=1 cargo test` and an explanation in the commit message.
- **Cross-platform assertion:** the same golden test runs on macOS arm64, macOS x86_64, and Linux x86_64-musl in CI. All three must produce byte-identical output.
- Stored in `tests/golden/{transform_id}/{fixture_name}.{in,out}`.
- A manifest (`tests/golden/MANIFEST.toml`) records each pair's `transform_id`, `version`, `description`, and SHA-256 of the expected output.

#### Manifest format
```toml
[[golden]]
transform_id = "json_minify"
version = "1.0.0"
input    = "tests/golden/json_minify/simple_object.in.json"
expected = "tests/golden/json_minify/simple_object.out.json"
description = "Simple object: strips whitespace, preserves key order"
sha256   = "<hex>"
```

#### Required golden fixtures (minimum set)

| Transform | Fixture | What it tests |
|-----------|---------|---------------|
| `json_minify` | `simple_object` | Whitespace stripped; key order preserved |
| `json_minify` | `nested_array` | Nested structure; no key reordering |
| `json_minify` | `string_escapes` | `\n`, `\"`, `\\` preserved verbatim |
| `schema_compaction` | `openai_tool_schema` | Description preserved; examples shortened |
| `log_compaction` | `adjacent_duplicates` | Collapse with evidence marker |
| `log_compaction` | `no_duplicates` | Output identical to input |
| `diff_compaction` | `small_diff` | Headers + changed lines kept |
| `secret_redaction` | `jwt_in_body` | JWT redacted; rest of body unchanged |

#### Security regression fixtures
- ReDoS canary inputs for every redaction regex complete within a fixed timeout budget
- `deny.toml` bans `fancy-regex` and `pcre2` in the redaction path
- `max_json_depth` rejects deeply nested JSON before transform execution
- Proxy `CL.TE` and `TE.CL` fixtures are rejected with no upstream request sent
- Proxy startup rejects `unsafe_disable_redaction = true`
- Header sanitizer fixtures prove credential-bearing header values never appear in logs/reports while still forwarding required upstream auth headers
- SSRF invariant fixture: the proxy ignores every inbound `X-TokenFold-*` header not in the `INTERFACES.md §3.1` request table; in particular there is no `X-TokenFold-Upstream` (or equivalent) override, and the upstream stays fixed at process start regardless of request headers

---

### Integration Tests

**Location:** `tests/integration/`

| Scenario | Description |
|----------|-------------|
| Multi-transform pipeline | Input requiring `json_minify` + `schema_compaction`: correct combined output; both `TransformReport` entries present |
| Conservative: tool description preserved | Tool schema with a long description: Conservative mode preserves description byte-for-byte |
| Protected-content survival | Multi-turn input with a fact in an early turn and a question in the last; fact survives |
| Safety rollback | Transform that would produce invalid JSON is rolled back; `SafetyDowngrade` warning emitted |
| `UnreachableTarget` | Target set below floor: `UnreachableTarget` status; floor content intact; best-effort output returned |
| Mode matrix compliance | Every `(mode, transform_id)` pair in `mode_matrix.toml` behaves as specified |
| Redaction before report | A fixture containing a known secret: no secret value appears in any `Warning.message` or report field |

---

### CLI End-to-End Tests

**Location:** `crates/tokenfold-cli/tests/`

Every example in PLAN.md § Public Interfaces / CLI must have a corresponding test. Tests invoke the built binary (not `cargo run`).

**Required test cases:**

```bash
# Each must be a test; expected exit code and stdout/stderr content asserted

tokenfold inspect examples/openai_payload.json --format openai --target-tokens 12000
# assert: exit 0; stderr contains "OVER budget" or "UNDER budget"; stdout empty

tokenfold compress examples/openai_payload.json --format openai --target-tokens 12000 --mode balanced
# assert: exit 0; stdout is valid JSON; stderr contains report

cat examples/openai_payload.json | tokenfold compress - --format openai --target-tokens 12000
# assert: exit 0; stdin works; stdout is valid JSON

tokenfold wrap -- git status
# assert: exit 0; stdout contains compressed git output; stderr contains report

tokenfold shell -- git diff
# assert: exit 0; `shell` alias behaves identically to `wrap`; stdout contains compressed git diff output; stderr contains report

tokenfold inspect examples/openai_payload.json --json
# assert: exit 0; stdout is valid JSON with top-level fields: schema_version, original_tokens, compressed_tokens, saved_tokens, savings_ratio, savings_pct, status, mode, format, task_scope, estimator, transforms, warnings (caller gates on schema_version first); stderr empty

tokenfold compress examples/openai_payload.json --disable secret_redaction
# assert: exit 2; stderr contains actionable error message

tokenfold benchmark examples/openai_payload.json --format openai
# assert: exit 0; output contains per-transform metrics; no heuristic label on counts

tokenfold compress examples/openai_payload.json --format openai
# assert: exit 0; stdout is compressed payload; stderr contains savings report
```

**Additional CLI contract tests:**
- `NO_COLOR=1 tokenfold inspect ...` → no ANSI codes in output
- `tokenfold compress --json ...` → stdout is the compressed payload; stderr is ONLY the JSON report
- Under-budget: `tokenfold inspect examples/small.json --target-tokens 100000` → exit 0; "UNDER budget" in stderr
- `tokenfold --version` → exit 0; version string on stdout

**Parity surface CLI tests:**
- `tokenfold init --agent <fixture-host> --dry-run --json` → no file changes; JSON lists planned edits and backup path
- `tokenfold init --agent <fixture-host>` then `tokenfold uninit --agent <fixture-host>` → final config byte-identical to original fixture
- `tokenfold doctor --json` → reports hook, estimator, retrieval store, stats ledger, proxy, and MCP status fields
- `tokenfold mcp serve` contract test → MCP initialize/list_tools/call_tool works for `tokenfold_compress`, `tokenfold_inspect`, `tokenfold_retrieve`, `tokenfold_stats`
- `tokenfold retrieve fixture-report.json` → restores exact original span bytes from a temporary local store
- `tokenfold stats tests/fixtures/reports/*.json --json` → stable aggregate fields, no raw payload bytes
- `tokenfold filters verify tests/fixtures/filters/*.toml` → validates schema, inline fixtures, and regex safety
- `tokenfold discover --json` → uses metadata fixtures only unless raw capture is explicitly enabled
- `tokenfold update --check --json` → verifies signed metadata fixture and does not mutate the installed binary

---

### Benchmark Tests

**Location:** `benches/compression_bench.rs`
**Tools:** `criterion` (latency/throughput) + `divan` (bytes allocated per call)

#### Regression threshold file
`benches/THRESHOLDS.toml` — records the baseline values that CI compares against. Updated deliberately at release; changes require reviewer sign-off.

```toml
[thresholds]
# p95 latency
command_output_under_1mb_p95_ms = 10.0
structured_json_under_2mb_p95_ms = 50.0
# Allocation
max_bytes_allocated_per_call = 0  # fill in after Phase 1 baseline
# Token savings (first-consumer fixture)
min_exact_token_savings_ratio = 0.0  # fill in after Phase 2
```

> **Placeholder baselines:** The `0` and `0.0` values above are sentinels, not measured baselines. Measured baselines from the dedicated benchmark runner MUST replace them (Phase 4) before `bench-regression` is authoritative; until they are filled in, `bench-regression` cannot block a release on those two thresholds.

#### Reproducibility requirements
Every published benchmark number must include:
- Tokenizer name + version (never heuristic in benchmark output)
- Fixture SHA-256 and source
- Mode and config
- Hardware: CPU model, core count, RAM
- Exact `cargo bench` command

These are recorded in the benchmark release section of `ENGINEERING.md` or in `CHANGELOG.md` per release.

---

### Fidelity Tests

**Location:** `eval/`
**Invocation:** `python eval/run_fidelity.py --gate --profile <profile>`

#### Gate profiles

| Profile | When run | Fixture set | Purpose |
|---------|----------|-------------|---------|
| `smoke-first-consumer` | Merge + release | 20-50 first-consumer fixtures | Block lossy transform promotion |
| `full-lossy-promotion` | Release (lossy-enabled) | All content types | Verify all default-enabled lossy transforms (v0.2 / Phase 5 lossy-promotion gate; not run for lossless-only v0.1) |

#### Gate artifact
Every run emits a JSON artifact with:

```json
{
  "profile": "smoke-first-consumer",
  "gate": "pass",
  "model_version": "...",
  "fixture_hashes": ["sha256:...", "..."],
  "total_cost_usd": 0.0,
  "quality_retention": 0.975,
  "contrastive_failure_rate": 0.002,
  "critical_token_survival_rate": 0.998,
  "transforms_evaluated": ["log_compaction", "diff_compaction"]
}
```

#### Cost controls
- Hard cost cap per CI run (reject before starting if estimated cost exceeds cap)
- Fixtures pinned (no dynamic selection in CI)
- Results cached by (fixture hash × model version × transform config hash)
- Re-run required only on cache miss or explicit `--no-cache`
- Cost per run logged to CI output for auditability

#### Fixture policy
Before any fixture enters `tests/fixtures/**` or `eval/tasks/**`:
- [ ] Data classification assigned
- [ ] License/source documented
- [ ] PII/secret scan completed
- [ ] Retention owner named
- [ ] Approval recorded in `eval/tasks/FIXTURES.md`

No production secrets or PII in any fixture — ever.

---

### Test Naming Conventions

```
# Pattern: layer::module::scenario__expected_outcome

unit::report::savings_ratio__zero_original__returns_zero
unit::budget::protected_floor__below_target__unreachable_target_status
unit::transforms::json_minify__duplicate_keys__order_preserved
unit::transforms::redaction__jwt__redacted
property::json_minify__any_valid_json__idempotent
property::budget__any_input__floor_lte_original
golden::json_minify__simple_object__byte_identical_across_platforms
integration::pipeline__unreachable_target__floor_content_intact
integration::safety__system_turn__preserved_after_compression
cli::inspect__json_flag__only_report_on_stdout
cli::compress__disable_secret_redaction__exit_2
```

---

### Coverage Policy

No coverage percentage target. Coverage is enforced by the required test list above, not by a `%` number. A green `tarpaulin` report that misses a critical edge case is insufficient — the required test list is the contract.

**New code rules:**
- New transform → new golden fixture(s) before merge
- New CLI example (in docs or README) → new CLI end-to-end test before merge
- New transform invariant → new property test before merge
- New fixture in `tests/fixtures/**` or `eval/tasks/**` → fixture policy checklist complete before merge

## Part 2 — CI/CD Pipeline

Defines all CI jobs, triggers, gates, and the release pipeline. Authoritative alongside PLAN.md § Internal Artifact Policy and PLAN.md § Task 11.

> **D-006 required:** Fill in `[CI_SYSTEM]`, runner specs, and credential injection method once D-006 is resolved. All `[TBD]` fields are placeholders.

---

### CI System

| Field | Value |
|-------|-------|
| CI system | `[CI_SYSTEM]` |
| PR runner | Standard Linux x86_64 |
| Cross-platform runners | macOS arm64, macOS x86_64, Linux x86_64, Windows x86_64 (release only) |
| Toolchain caching | `sccache` or native Cargo target cache |
| Rust toolchain | Pinned in `rust-toolchain.toml` |
| Artifact storage | Internal Artifactory generic repo |

---

### Trigger Matrix

| Trigger | Jobs run | Notes |
|---------|----------|-------|
| Pull request (any → main) | `lint`, `test`, `security` (advisory), `golden-cross-platform` | Fast feedback loop |
| Merge to main | All PR jobs + `security` (blocking), `bench-smoke`, `fidelity-smoke` | `bench-smoke` advisory on merge; `fidelity-smoke` blocking on merge |
| Release tag (`v*.*.*`) | ALL jobs, including `bench-regression`, `fidelity-full`, `cross-compile`, `publish` | All jobs blocking |
| Scheduled nightly | `security`, `bench-smoke`, `fidelity-smoke` | Failures alert but do not block; catches drift |

---

### Job Definitions

#### Job: `lint`
**Triggers:** PR, merge, release
**Blocking:** Yes (all)
**Runner:** Linux x86_64
**Steps:**
```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```
**Pass criterion:** Both commands exit 0.

---

#### Job: `test`
**Triggers:** PR, merge, release
**Blocking:** Yes (all)
**Runner:** Linux x86_64
**Steps:**
```bash
cargo test --workspace --locked
```
**Pass criterion:** All tests pass. No skipped tests allowed on release.
**Note:** Unit tests inject a deterministic mock estimator via `compress_with_estimator`. No real LLM API calls in this job.

---

#### Job: `security`
**Triggers:** PR (advisory), merge (blocking), release (blocking), nightly (advisory)
**Blocking:** Yes on merge and release; advisory on PR
**Runner:** Linux x86_64
**Steps:**
```bash
cargo audit
cargo deny check advisories bans licenses sources
```
**Pass criterion:** 0 unfixed RUSTSEC advisories; all `cargo deny` rules pass.

---

#### Job: `golden-cross-platform`
**Triggers:** PR, merge, release
**Blocking:** Yes (all)
**Runners:** macOS arm64, macOS x86_64, Linux x86_64-musl (parallel)
**Description:** Runs the golden byte-equality test suite on all three platforms. Same input → same bytes on all.
```bash
cargo test --workspace --locked -- golden
```
**Pass criterion:** SHA-256 of every golden fixture output matches the value in `tests/golden/MANIFEST.toml` on all three platforms.

---

#### Job: `bench-smoke`
**Triggers:** Merge to main, release, nightly
**Blocking:** Advisory on merge; blocking on release
**Runner:** Linux x86_64 (consistent hardware required — same runner type every run)
**Description:** Fast benchmark run to catch gross regressions. Not the authoritative benchmark.
```bash
cargo bench --workspace -- --measurement-time 5
```
**Pass criterion (advisory on merge; blocking on release):**
- No transform exceeds 2× its baseline p95 latency
- Bytes allocated do not regress > 20%
- Baseline stored in `benches/THRESHOLDS.toml`

---

#### Job: `bench-regression`
**Triggers:** Release tag only
**Blocking:** Yes
**Runner:** Dedicated benchmark runner (consistent hardware; no shared resources)
**Description:** Full benchmark with exact tokenizer and full measurement time. Produces the benchmark release section for `ENGINEERING.md` or `CHANGELOG.md`.
```bash
cargo bench --workspace
# Compare all threshold values in benches/THRESHOLDS.toml
```
**Pass criterion:** Every threshold in `benches/THRESHOLDS.toml` passes. Any failure requires an explicit override note in the release PR with reviewer sign-off.
**Artifact:** Updated benchmark release section committed to `ENGINEERING.md` or `CHANGELOG.md`.

---

#### Job: `fidelity-smoke`
**Triggers:** Merge to main (advisory), release (blocking)
**Blocking:** Blocking on release; advisory on merge
**Runner:** Linux x86_64 (with `ANTHROPIC_API_KEY` or equivalent injected)
**Steps:**
```bash
python eval/run_fidelity.py --gate --profile smoke-first-consumer
```
**Pass criterion:** Gate artifact contains `"gate": "pass"`.
**Required artifact fields:**
```json
{
  "profile": "smoke-first-consumer",
  "gate": "pass",
  "model_version": "...",
  "fixture_hashes": ["..."],
  "total_cost_usd": 0.0,
  "quality_retention": ...,
  "contrastive_failure_rate": ...,
  "critical_token_survival_rate": ...
}
```
**Cost controls:**
- Hard cap: `$[TBD]` per run (reject if estimated cost exceeds cap before starting)
- Results cached by (fixture hash × model version × transform config hash)
- Re-run only on cache miss or explicit `--no-cache`

---

#### Job: `fidelity-full`
**Triggers:** Release tag (only for releases that include a lossy transform enabled by default)
**Blocking:** Yes
**Runner:** Linux x86_64 (with credentials)
**Steps:**
```bash
python eval/run_fidelity.py --gate --profile full-lossy-promotion
```
**Pass criterion:** All lossy transforms that ship enabled by default meet their `quality_retention` and `contrastive_failure_rate` thresholds from `ROADMAP.md § D-005`.
**Note:** `full-lossy-promotion` is a v0.2 / Phase 5 lossy-promotion gate; it is not run for lossless-only v0.1 releases (matches `ROADMAP.md § F-016` "full = v0.2").

---

#### Job: `cross-compile`
**Triggers:** Release tag only
**Blocking:** Yes
**Description:** Produces static binaries for all supported platforms.

**Build matrix:**

| Target | Runner | Method |
|--------|--------|--------|
| `x86_64-apple-darwin` | macOS x86_64 | `cargo build --release --locked` (native) |
| `aarch64-apple-darwin` | macOS arm64 | `cargo build --release --locked` (native) |
| `x86_64-unknown-linux-musl` | Linux x86_64 | `cross build --release --locked` (Docker musl) |
| `aarch64-unknown-linux-musl` | Linux x86_64 | `cross build --release --locked` (Docker musl aarch64) |
| `x86_64-pc-windows-msvc` | Windows x86_64 | `cargo build --release --locked` (native) |

**Post-build steps (per target):**
```bash
# 1. Compute checksum
sha256sum target/<target>/release/tokenfold > tokenfold-<target>.sha256

# 2. Sign binary (signing key management: [TBD per D-007])
<signing-command> tokenfold-<target>

# 3. Verify signature
<verify-command> tokenfold-<target>.sig tokenfold-<target>

# 4. Upload to Artifactory
<artifactory-upload> \
  --path "tokenfold-cli/<version>/tokenfold-<target>" \
  --file target/<target>/release/tokenfold

<artifactory-upload> \
  --path "tokenfold-cli/<version>/tokenfold-<target>.sha256" \
  --file tokenfold-<target>.sha256

<artifactory-upload> \
  --path "tokenfold-cli/<version>/tokenfold-<target>.sig" \
  --file tokenfold-<target>.sig
```
**Pass criterion:** All 5 binaries uploaded with checksums and signatures; SHA-256 values recorded in the release manifest.

---

#### Job: `sbom`
**Triggers:** Release tag only
**Blocking:** Yes
**Steps:**
```bash
cargo-cyclonedx --format json --output tokenfold-sbom.json
<artifactory-upload> --path "tokenfold-cli/<version>/tokenfold-sbom.json" --file tokenfold-sbom.json
```
**Pass criterion:** SBOM generated and uploaded.

---

#### Job: `publish`
**Triggers:** Release tag (runs ONLY after all other release jobs are green)
**Blocking:** Yes
**Runner:** Linux x86_64
**Steps:**
```bash
cargo publish -p tokenfold-core --registry internal --locked
cargo publish -p tokenfold-cli  --registry internal --locked
```
**Pass criterion:** Both commands exit 0; internal registry shows the new version.
**Note:** `cargo publish` without `--registry internal` is forbidden by `PLAN.md § Internal Artifact Policy`.

---

### Release Checklist

Human gate after all CI jobs pass on the release tag:

- [ ] All blocking CI jobs green
- [ ] CHANGELOG.md has an entry for this version
- [ ] SBOM generated and attached to release
- [ ] Install one-liner verified on a clean macOS arm64 environment (manual)
- [ ] Install one-liner verified on a clean Linux x86_64 environment (manual)
- [ ] `tokenfold --version` outputs the correct version string
- [ ] Fidelity gate artifact archived (retained for audit)
- [ ] Benchmark artifact committed and reviewed in `ENGINEERING.md` or `CHANGELOG.md`
- [ ] Measured `benches/THRESHOLDS.toml` baselines have replaced all `0` / `0.0` placeholder sentinels (required before `bench-regression` is authoritative)
- [ ] No known secret values appear in any log, report, or artifact (verify from CI logs)

---

### Artifact Naming Convention

```
# Binaries (Artifactory generic repo path)
tokenfold-cli/<version>/tokenfold-x86_64-apple-darwin
tokenfold-cli/<version>/tokenfold-x86_64-apple-darwin.sha256
tokenfold-cli/<version>/tokenfold-x86_64-apple-darwin.sig

tokenfold-cli/<version>/tokenfold-aarch64-apple-darwin
tokenfold-cli/<version>/tokenfold-aarch64-apple-darwin.sha256
tokenfold-cli/<version>/tokenfold-aarch64-apple-darwin.sig

tokenfold-cli/<version>/tokenfold-x86_64-unknown-linux-musl
tokenfold-cli/<version>/tokenfold-x86_64-unknown-linux-musl.sha256
tokenfold-cli/<version>/tokenfold-x86_64-unknown-linux-musl.sig

tokenfold-cli/<version>/tokenfold-aarch64-unknown-linux-musl
tokenfold-cli/<version>/tokenfold-aarch64-unknown-linux-musl.sha256
tokenfold-cli/<version>/tokenfold-aarch64-unknown-linux-musl.sig

tokenfold-cli/<version>/tokenfold-x86_64-pc-windows-msvc.exe
tokenfold-cli/<version>/tokenfold-x86_64-pc-windows-msvc.exe.sha256
tokenfold-cli/<version>/tokenfold-x86_64-pc-windows-msvc.exe.sig

tokenfold-cli/<version>/tokenfold-sbom.json

# Cargo crates (internal registry)
tokenfold-core-<version>.crate
tokenfold-cli-<version>.crate
```

---

### Environment Variables

| Variable | Used by job | Source |
|----------|-------------|--------|
| `CARGO_REGISTRY_TOKEN` | `publish` | CI secrets manager |
| `ARTIFACTORY_TOKEN` | `cross-compile`, `sbom`, `publish` | CI secrets manager |
| `ANTHROPIC_API_KEY` (or equivalent) | `fidelity-smoke`, `fidelity-full` | CI secrets manager |
| `TOKENCUT_FIDELITY_PROVIDER` | `fidelity-*` | CI config (not a secret) |
| `SCCACHE_*` or equivalent | All `cargo` jobs | CI config |
| `BINARY_SIGNING_KEY` | `cross-compile` | CI secrets manager |

No secret values are printed to CI logs. All credential handling follows PLAN.md § Security Model.

---

### Branch / Tag Strategy

| Branch / Tag | Purpose |
|--------------|---------|
| `main` | Stable development branch; fidelity-smoke blocks merge |
| `feature/*` | Feature branches; PR target is `main` |
| `v0.1.0`, `v0.1.1`, ... | Release tags; trigger the full release pipeline |
| `v0.2.0`, ... | Major milestone releases |

Semver policy:
- **Patch** (`0.1.x`): bug fixes, no API changes, no transform behavior changes
- **Minor** (`0.x.0`): new transforms, new CLI flags, backwards-compatible API additions; any transform version bump
- **Major** (`x.0.0`): breaking API changes (post-v1.0 only; before v1.0 minor versions may break)

---

### v0.2 Optional Surface Jobs

When a release includes the proxy or Python binding, add these blocking gates:

```bash
# Proxy
cargo test -p tokenfold-proxy --locked

# Python binding
maturin build --release -m crates/tokenfold-py/pyproject.toml
pytest python-tests
python -m twine check target/wheels/*
python -m twine upload --repository-url "$ARTIFACTORY_PYPI_REPOSITORY_URL" target/wheels/*
```

These jobs block only releases that include those crates; they are not required for core/CLI-only releases.

## Part 3 — Risk Register

Risks assessed by **Likelihood** (H/M/L) and **Impact** (H/M/L). Priority = L × I. Residual risk after stated mitigation is noted. Owners are TBD until Phase 0 assignments are made.

Review and update this register at the start of each phase.

---

### Risk Summary

| ID | Title | L | I | Priority | Phase | Status |
|----|-------|---|---|----------|-------|--------|
| R-001 | ML contributors blocked by Rust engine | H | H | Critical | Ongoing | Open |
| R-002 | Package name collision or legal delay | H | H | Critical | Phase 0 | Open |
| R-003 | Internal Artifactory not provisioned in time | M | H | High | Phase 0/4 | Open |
| R-004 | Exact tokenizer credentials unavailable in CI | M | H | High | Phase 1+ | Open |
| R-005 | Fidelity gate cost exceeds budget | M | M | Medium | Phase 2+ | Open |
| R-006 | Transform quality below threshold: lossy scope shrinks | M | H | High | Phase 2 | Open |
| R-007 | Circular dependency: mode matrix ↔ fidelity gate | H | M | High | Phase 2 | Open |
| R-008 | `serde_json preserve_order` not enforced in practice | M | H | High | Phase 2+ | Open |
| R-009 | Cross-platform static binary build complexity | M | M | Medium | Phase 4 | Open |
| R-010 | First consumer changes requirements mid-build | M | H | High | Phase 1+ | Open |
| R-011 | ReDoS in redaction path via wrong regex crate | L | H | High | Phase 2+ | Open |
| R-012 | Fidelity harness secrets handling in CI | M | H | High | Phase 2+ | Open |
| R-013 | maturin/abi3 wheel platform complexity (v0.2) | L | M | Low | Phase 5 | Deferred |
| R-014 | Benchmark hardware variance masks real regressions | M | M | Medium | Phase 4+ | Open |
| R-015 | MCP protocol drift breaks agent integrations | M | M | Medium | Phase 5+ | Open |
| R-016 | Evidence-store growth or TTL failures persist sensitive originals | M | H | High | Phase 5+ | Open |
| R-017 | Declarative command filter injection or unsafe regex | M | H | High | Phase 5+ | Open |
| R-018 | Stats ledger metadata leaks PII or business-sensitive context | M | H | High | Phase 5+ | Open |
| R-019 | Proxy SSRF via per-request upstream override / credential drain | L | H | High | Phase 5 | Open |

---

### Risk Details

#### R-001 · ML Contributors Blocked by Rust Engine
**Likelihood:** High | **Impact:** High | **Phase:** Ongoing

The highest-value, most iteration-heavy transforms — lossy semantic transforms (conversation history, prose extraction, code digest) — are owned by ML/research contributors who work in Python. The Rust-first architecture creates a contribution bottleneck: ML contributors can prototype in the Python harness but cannot iterate on the engine directly.

**Mitigation:**
- Prototype and quality-gate every new lossy transform in the Python/research harness first (PLAN.md § Language Decision)
- Port only proven, stabilized algorithms into Rust
- Python binding (v0.2) gives Python contributors a test surface against the real engine
- Explicit prototype-then-port workflow documented in ENGINEERING.md
- Keep the transform API surface small and well-documented

**Residual risk:** Medium. The workflow requires discipline; without enforcement, ML contributors are silently excluded from the engine.

**Owner:** TBD

---

#### R-002 · Package Name Collision or Legal Delay
**Likelihood:** High | **Impact:** High | **Phase:** Phase 0

`tokenfold` may collide with an existing crate on crates.io, an internal registry entry, or a trademarked term. If the name is blocked after Task 1 has started, every file, module, type name, binary name, and doc URL must be renamed — significant churn.

**Mitigation:**
- D-001 must be fully resolved (including legal sign-off) before Task 1 creates any file
- Registry checks: crates.io, internal Cargo, internal PyPI namespace
- Identify a backup name before confirming the primary
- Phase 0 exit criterion explicitly requires name approval

**Residual risk:** Low if Phase 0 is completed correctly. Critical if code is written before the check.

**Owner:** TBD (legal/brand + engineering)

---

#### R-003 · Internal Artifactory Not Provisioned in Time
**Likelihood:** Medium | **Impact:** High | **Phase:** Phase 0 (dependency resolution) / Phase 4 (publishing)

The plan depends on internal Artifactory mirrors for both dependency resolution (Cargo sparse index) and artifact publishing. If the mirror is not available before Task 1, the workspace cannot build in CI. If it is not available before Phase 4, the v0.1 release cannot be published.

**Mitigation:**
- D-003 must be resolved and infrastructure provisioned before Phase 0 exits
- Track as a Phase 0 exit criterion with an infra-team owner
- Fallback for local development only: temporarily allow public crates.io in `.cargo/config.toml` with a `# TEMP` comment; this is explicitly removed before any CI run or internal publication
- No code is published to public crates.io under any circumstances

**Residual risk:** Medium. Infra provisioning timelines are often unpredictable.

**Owner:** TBD (platform/infra team)

---

#### R-004 · Exact Tokenizer Credentials Unavailable in CI
**Likelihood:** Medium | **Impact:** High | **Phase:** Phase 1+

`AnthropicApiEstimator` calls Anthropic's `/messages/count_tokens` API, which requires credentials. If credentials are not injected into the CI environment, benchmark runs fall back to heuristic counts, and release benchmarks lose validity. `TiktokenEstimator` also requires the tiktoken model files to be accessible.

**Mitigation:**
- CI unit tests inject a deterministic mock estimator via `compress_with_estimator` (no real API call)
- Exact counts for fixtures are cached; CI uses cached values for most test runs
- Only the `fidelity-*` and `bench-regression` CI jobs require live credentials
- Credential provisioning is a required item in D-006 (CI system setup)
- `ENGINEERING.md § Environment Variables` documents which jobs need which credentials

**Residual risk:** Low for development CI (mock covers unit tests). Medium for release benchmarks (exact counts required; credentials must be provisioned and rotated).

**Owner:** TBD

---

#### R-005 · Fidelity Gate Cost Exceeds Budget
**Likelihood:** Medium | **Impact:** Medium | **Phase:** Phase 2+

The fidelity harness calls an LLM API for every (transform × mode × ratio × fixture) combination. As the fixture set grows, per-release evaluation cost grows. If costs are not controlled, teams may bypass the gate under budget pressure, removing the quality guarantee.

**Mitigation:**
- Hard cost cap per CI run (reject before starting if estimated cost exceeds cap)
- Smoke gate uses a small fixture set (20-50 cases); full gate only runs on release
- Results cached by (fixture hash × model version × transform config hash); re-run only on cache miss
- Cost per run logged in CI output (transparency discourages scope creep)
- Gate profile scope is explicitly controlled: new fixtures require approval

**Residual risk:** Low for smoke gate. Medium for full gate as fixture set grows — the cap and caching are essential long-term.

**Owner:** TBD

---

#### R-006 · Transform Quality Below Threshold — Lossy Scope Shrinks
**Likelihood:** Medium | **Impact:** High | **Phase:** Phase 2

If the fidelity gate reveals that `log_compaction` or `diff_compaction` cannot meet the quality threshold at any useful compression ratio, these transforms cannot ship as defaults in v0.1. The v0.1 release would contain only lossless transforms — still useful, but narrower than planned.

**Mitigation:**
- Lossy transforms start behind `--experimental`; they are only promoted after the gate is green (PLAN.md sequencing)
- v0.1 is fully defined by the lossless set; lossy promotion is an upside, not a requirement
- Fidelity gate failure triggers algorithm revision, not a scope exception
- Safe default ratio per transform is derived from accuracy@ratio curves, not assumed

**Residual risk:** Low for plan integrity (the `--experimental` path is the correct fallback). Medium for first-consumer impact if their primary workload requires lossy transforms.

**Owner:** TBD

---

#### R-007 · Circular Dependency: Mode Matrix ↔ Fidelity Gate
**Likelihood:** High | **Impact:** Medium | **Phase:** Phase 2

Task 3.5 must create the mode matrix (`tests/fixtures/mode_matrix.toml`) with ratio caps per transform. But the final ratio values are supposed to be derived from the accuracy@ratio curves produced by the fidelity gate (Task 9). This creates a bootstrapping problem: the matrix must exist before the gate can run, but the gate produces the data needed to validate the matrix.

**Mitigation:**
- Phase 2 (Task 3.5) sets **draft** thresholds in the mode matrix using baselines from published research (ACON, kompact, LLMLingua)
- The fidelity smoke gate runs against those draft thresholds
- Phase 2 refines the thresholds with real accuracy@ratio data once the full fidelity gate runs, and updates the mode matrix (per `INTERFACES.md` Part 2 and `ROADMAP.md § D-005`, the validated ratio bands land after Phase 2)
- The mode matrix is versioned: threshold updates are explicit, reviewable changes (not silent drift)
- Draft threshold source is documented in the matrix file

**Residual risk:** Low. Draft thresholds are acceptable for the smoke gate; versioning ensures the update path is explicit.

**Owner:** TBD

---

#### R-008 · `serde_json preserve_order` Not Enforced in Practice
**Likelihood:** Medium | **Impact:** High | **Phase:** Phase 2+

The plan requires `serde_json` with the `preserve_order` feature to prevent key reordering (which would bust provider prompt caches). If this feature is not in the workspace `Cargo.toml`, a transitive dependency could enable a different `serde_json` feature set, or a future refactor might use `HashMap` iteration. Silent key reordering would be a latent bug affecting every JSON transform.

**Mitigation:**
- Workspace `Cargo.toml` explicitly declares `serde_json = { version = "1", features = ["preserve_order"] }`
- Golden tests detect any key reordering (byte-exact comparison)
- Property-based tests assert key order for arbitrarily-structured JSON inputs
- `cargo deny` can ban `serde_json` without the `preserve_order` feature (custom check)
- CI runs golden tests on every PR, not just release

**Residual risk:** Low with property tests + golden tests. Medium without them.

**Owner:** TBD

---

#### R-009 · Cross-Platform Static Binary Build Complexity
**Likelihood:** Medium | **Impact:** Medium | **Phase:** Phase 4

Building static binaries for all 5 targets requires cross-compilation infrastructure, macOS code signing, Windows MSVC toolchain, and musl libc toolchains for Linux. Each platform introduces unique failure modes: macOS Gatekeeper quarantine, Windows SmartScreen, musl C library edge cases for tree-sitter (if `code-digest` feature is enabled).

**Mitigation:**
- D-007 (cross-compile strategy) resolved before Phase 4 starts
- Hybrid approach: native runners for macOS/Windows; `cross` + Docker for Linux musl/aarch64
- macOS quarantine handling documented in README install notes or `ENGINEERING.md`
- Windows Defender/SmartScreen behavior documented
- `code-digest` feature (tree-sitter + C toolchain) excluded from static builds unless explicitly tested
- Install one-liner verified on clean environments before release

**Residual risk:** Medium. Platform-specific toolchain issues require periodic maintenance; macOS code signing in particular requires active certificates.

**Owner:** TBD

---

#### R-010 · First Consumer Changes Requirements Mid-Build
**Likelihood:** Medium | **Impact:** High | **Phase:** Phase 1+

If the first consumer's workload, payload types, or quality targets change after Phase 0 sign-off, the transform set, fidelity fixture set, and benchmark targets may need revision.

**Mitigation:**
- Phase 0 exit criterion requires first-consumer sign-off on the worksheet (D-002), not just verbal agreement
- v0.1 payload format scope (D-004) is locked at Phase 0; changes require an explicit re-review of impacted tasks
- The engine is provider-neutral; adding a new payload format adds a new `InputFormat` variant and estimator, not a core redesign
- Change request process: any first-consumer change after Phase 0 requires a written impact assessment before the plan is updated

**Residual risk:** Medium. Requirements changes are always possible; explicit sign-off and narrow scope reduce but don't eliminate the risk.

**Owner:** TBD (first consumer team + engineering lead)

---

#### R-011 · ReDoS in Redaction Path via Wrong Regex Crate
**Likelihood:** Low | **Impact:** High | **Phase:** Phase 2+

If a backtracking or PCRE-style regex crate (`fancy-regex`, `pcre2`) is introduced into the `secret_redaction` transform — either directly or via a transitive dependency — a crafted input could trigger catastrophic backtracking, causing a denial-of-service in the proxy or a hung CLI.

**Mitigation:**
- Pin the Rust `regex` crate (linear-time, guaranteed no backtracking) in the redaction path
- Explicitly ban `fancy-regex` and `pcre2` in `deny.toml` for the `tokenfold-core` crate
- ReDoS canary fixtures included in redaction unit tests (inputs designed to catastrophically backtrack PCRE engines)
- Code review checklist: any new dependency in `tokenfold-core` must not introduce backtracking regex

**Residual risk:** Low if `cargo deny` bans are enforced and reviewed. High if the ban is not in `deny.toml`.

**Owner:** TBD

---

#### R-012 · Fidelity Harness Secrets Handling in CI
**Likelihood:** Medium | **Impact:** High | **Phase:** Phase 2+

The fidelity harness calls an LLM API with credentials and runs against fixtures that may contain sensitive internal content (even after PII scan). If the harness logs response content, dumps fixture text to CI logs, or writes intermediate results to unprotected paths, secrets or sensitive content could leak.

**Mitigation:**
- Harness never logs raw fixture text or LLM response bodies to CI output
- Intermediate result files written to a credential-protected path or cleaned up immediately
- Fixture approval workflow (documented in `eval/tasks/FIXTURES.md`) includes a data classification step
- CI log retention policy reviewed to ensure eval artifacts are not retained beyond a defined window
- Harness emits only the gate artifact (JSON summary) to CI; raw scores retained only in a secured artifact store

**Residual risk:** Medium. Secrets handling in Python scripts requires careful review; a careless `print()` or exception traceback can leak content.

**Owner:** TBD

---

#### R-013 · maturin/abi3 Wheel Platform Complexity (v0.2)
**Likelihood:** Low | **Impact:** Medium | **Phase:** Phase 5 (deferred)

Building Python abi3 wheels via maturin requires careful coordination of pyo3 version, Python version floor (abi3-py39), and platform-specific toolchains. macOS wheels require `MACOSX_DEPLOYMENT_TARGET`; Windows wheels require MSVC.

**Mitigation:** Deferred to Phase 5. By then, cross-compile infrastructure from Phase 4 provides the foundation. Well-known patterns exist for maturin abi3 wheels.

**Residual risk:** Low (deferred; well-understood problem space).

**Owner:** TBD

---

#### R-014 · Benchmark Hardware Variance Masks Real Regressions
**Likelihood:** Medium | **Impact:** Medium | **Phase:** Phase 4+

If `bench-regression` runs on shared or variable hardware (e.g., a CI runner with variable CPU scheduling), latency measurements will have high variance, making it impossible to detect real regressions within the threshold tolerance.

**Mitigation:**
- `bench-regression` runs on a dedicated benchmark runner with consistent hardware specs
- Benchmark runner spec is recorded in `ENGINEERING.md` or `CHANGELOG.md` per release
- `bench-smoke` (which runs on shared hardware) uses a 2× threshold (coarse regression detection); `bench-regression` uses tighter thresholds on dedicated hardware
- divan's byte-allocation measurements are deterministic and not affected by hardware variance — use these as the primary regression signal for allocator behavior

**Residual risk:** Medium for latency thresholds on shared hardware. Low for allocation thresholds (deterministic).

**Owner:** TBD
---

<!-- [transcription gap] Lines 1005-1018 (risk R-015) and 1020-1025 (start of R-016) were not captured in the source PDF: its screenshots jump from line 1002 to 1026. -->













#### R-016 · Evidence-Store Growth or TTL Failures Persist Sensitive Originals






**Residual risk:** Medium until first production retention review.

**Owner:** TBD

---

#### R-017 · Declarative Command Filter Injection or Unsafe Regex
**Likelihood:** Medium | **Impact:** High | **Phase:** Phase 5+

Custom F-047 filter packs could introduce catastrophic regex behavior, unexpected file reads, or shell-like behavior if the schema is too permissive.

**Mitigation:** Filters are declarative only; no shell execution; regex safety checks and ReDoS canaries are mandatory; `filters trust` records provenance; malformed filters fail closed.

**Residual risk:** Medium for user-authored filters, low for built-ins.

**Owner:** TBD

---

#### R-018 · Stats Ledger Metadata Leaks PII or Business-Sensitive Context
**Likelihood:** Medium | **Impact:** High | **Phase:** Phase 5+

Even without raw payload bytes, report metadata can reveal filenames, tool names, project names, token volumes, or failure patterns.

**Mitigation:** Ledger stores redacted report metadata only; raw payload capture is opt-in and retention-limited; JSON/CSV export stability tests assert no payload fields; project identifiers can be hashed for shared exports.

**Residual risk:** Medium. Metadata sensitivity depends on deployment context.

**Owner:** TBD

---

#### R-019 · Proxy SSRF via Per-Request Upstream Override / Credential Drain
**Likelihood:** Low | **Impact:** High | **Phase:** Phase 5

A per-request mechanism to change the upstream URL, credential, or routing (e.g. a Headroom-style `x-headroom-base-url` compat header) would expose classic SSRF and credential-drain vectors, letting a caller redirect traffic or exfiltrate forwarded upstream credentials.

**Mitigation:** Per `INTERFACES.md § 3.2`, the upstream is fixed at process start (`--upstream` / `proxy.upstream`); the proxy ignores every inbound `X-TokenFold-*` header not in the §3.1 request table, and there is **no** `X-TokenFold-Upstream` (or equivalent) override. Combined with pass-through auth (the client supplies its own upstream credential; the proxy stores none), this closes the vector. A security-regression fixture asserts the invariant, and any future per-request routing override requires an explicit SSRF/security review.

**Residual risk:** Low. The invariant is designed-in and test-guarded.

**Owner:** TBD

---

### Risk Review Schedule

| Milestone | Risk review action |
|-----------|--------------------|
| Phase 0 exit | Review all Open risks; confirm owner assignments; update likelihood estimates |
| Phase 1 exit | Review R-001, R-003, R-004; close R-002 if resolved |
| Phase 2 exit | Review R-005, R-006, R-007, R-008; fidelity data updates estimates |
| Phase 4 (release) | Full risk review; archive closed risks |
| Phase 5 (v0.2 release) | Final review; update deferred risks |

## Part 4 — Contributing (prototype-then-port)

### Who This Is For

Two contributor personas:

| Persona | Primary language | Primary contribution area |
|---------|------------------|---------------------------|
| **Engine contributor** | Rust | Core types, estimators, pipeline, CLI, proxy, performance |
| **ML/Research contributor** | Python | Lossy transform prototyping, fidelity harness, accuracy evaluation |

Both personas are essential. ML contributors own the quality of every lossy transform. Engine contributors own the stability and portability of the runtime. The prototype-then-port workflow below is the bridge.

---

### Prototype-Then-Port Workflow (ML Contributors)

This is the required path for every new lossy or policy-driven transform. It exists because:
- ML contributors can iterate quickly in Python against the fidelity harness
- The Rust port happens only once the algorithm is stable and proven
- This reserves the irreversible Rust commitment for settled algorithms (see PLAN.md § Language Decision)

#### Step 1: Prototype in Python

Write the transform algorithm in `eval/transforms/` as a pure Python function:

```python
# eval/transforms/log_compaction.py

def compact_logs(text: str, *, remove_timestamps: bool = False) -> str:
    """
    Collapse adjacent duplicate lines to first + [repeated Nx] + last.
    Adjacent-only: interleaved logs compress little (documented limitation).
    """
    lines = text.splitlines()
    if not lines:
        return ""
    # ... implementation
    return "\n".join(out)
```

Requirements:
- Pure Python, no external dependencies beyond the `eval/` virtualenv
- Must be deterministic (same input → same output on every call)
- Must be testable with `pytest eval/`

#### Step 2: Run Against the Fidelity Harness

Add a task set for the transform in `eval/tasks/`:

```
eval/tasks/
  log_compaction/
    task_001.json          # { "input": "...", "question": "...", "answer": "..." }
    task_002.json
    ...
    FIXTURES.md            # data classification + approval record
```

Run the harness against your prototype:

```bash
cd eval
pip install -e .
python run_fidelity.py \
  --transform log_compaction \
  --profile prototype \
  --model claude-sonnet-5 \
  --ratio 0.60
```

The harness will output:
- `quality_retention` (downstream task score: original vs compressed)
- `contrastive_failure_rate` (tasks that pass raw but fail compressed)
- `accuracy_at_ratio` curve
- `critical_token_survival_rate`

#### Step 3: Iterate on Quality

Tune the algorithm until it meets the agreed thresholds from ROADMAP.md § D-005. Common levers:
- Adjust the compression ratio (less aggressive → higher quality)
- Refine line selection criteria
- Add evidence markers for context recovery
- Adjust what counts as "adjacent" (window size)

**The gate condition** is: `quality_retention ≥ threshold` at the planned shipped default ratio.

Document the `validated_ratio_band` and `quality_retention` numbers in a comment in your prototype file.

#### Step 4: Write the Port Spec

Before opening a port PR, write a brief port spec in `eval/transforms/{transform_id}_port_spec.md`:

```markdown
# log_compaction Port Spec

## Algorithm (finalized)
Collapse adjacent identical lines → first + [repeated Nx] + last.
Adjacent window: 1 (exact adjacent-only; interleaved not handled).
Timestamp removal: opt-in only (default off).
Evidence marker: `[repeated Nx]`

## Validated ratio band
0.55–0.70 for command-output fixtures (first-consumer set)

## Quality gate results
quality_retention: 0.978 at ratio=0.65
contrastive_failure_rate: 0.003 at ratio=0.65
critical_token_survival_rate: 0.997

## Edge cases to port
- Single line: unchanged
- Empty input: empty output
- All lines identical: first + [repeated Nx] + last
- Two lines: both kept (no collapse)

## Not ported (Python-only limitation)
- Interleaved log deduplication (documented limitation)
```

#### Step 5: Port to Rust

An engine contributor ports the finalized algorithm to `crates/tokenfold-core/src/transforms/{transform_id}.rs`. The Rust implementation must:
- Match the Python prototype's behavior on all test fixtures (golden tests)
- Be deterministic (property test)
- Pass the byte-level golden suite

The ML contributor reviews the port against the fidelity harness:

```bash
# Build the CLI with the new transform
cargo build -p tokenfold-cli --release

# Run fidelity harness against the Rust binary
python eval/run_fidelity.py \
  --transform log_compaction \
  --profile smoke-first-consumer \
  --binary target/release/tokenfold \
  --gate
```

**Port is accepted only if the Rust binary passes the fidelity gate.**

---

### Engine Contributor Guidelines

#### Adding a new transform

1. Add the transform module in `crates/tokenfold-core/src/transforms/`
2. Register it in `pipeline.rs` with a canonical ID and version
3. Add it to the mode matrix in `crates/tokenfold-core/src/modes.rs` (start as `--experimental`)
4. Add golden fixtures in `tests/golden/{transform_id}/`
5. Update `tests/fixtures/mode_matrix.toml`
6. Run `cargo test --workspace --locked`

For lossy transforms, the transform must have passed the ML contributor's fidelity review (Step 4 above) before the port PR is opened.

#### Adding a new CLI flag

1. Add the flag to `Cli` or the relevant `Command` variant in `crates/tokenfold-cli/src/main.rs`
2. Wire it through to `CompressionPolicy` or the relevant config struct
3. Add a CLI end-to-end test in `crates/tokenfold-cli/tests/`
4. Update `INTERFACES.md` if the flag affects report output
5. Update `tokenfold.toml` schema in `INTERFACES.md`

#### Modifying the `CompressionReport` struct

Any field addition, removal, or rename is a breaking API change:
- Bump the report schema version
- Update the `INTERFACES.md` canonical contract
- Update CLI rendering
- Update proxy `X-TokenFold-*` header mapping
- Update Python binding (if v0.2 is live)
- Update fidelity harness JSON parsing

#### Security-bearing code (redaction, safety gates)

Changes to `transforms/redaction.rs` or `safety.rs` require:
- A security-focused code review (tag `security-review`)
- New regression fixture for the attack pattern being addressed
- `cargo deny check` passing (no new backtracking regex crates)

---

### Fixture Policy

Before any fixture enters `tests/fixtures/**` or `eval/tasks/**`:

- [ ] Data classification assigned (`public`, `internal`, `confidential`)
- [ ] License/source documented
- [ ] PII/secret scan completed (no credentials, no personal data)
- [ ] Retention owner named
- [ ] Approval recorded in `eval/tasks/FIXTURES.md`

**No production secrets or PII in any fixture, ever.** This is a hard rule, not a guideline. If you're unsure whether a fixture is clean, run it through `secret_redaction` first and verify the output.

---

### Dev Environment Setup

#### Rust

```bash
# Install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install the pinned toolchain (from rust-toolchain.toml)
rustup show

# Build everything
cargo build --workspace --locked

# Run tests
cargo test --workspace --locked

# Run clippy
cargo clippy --workspace --all-targets -- -D warnings
```

#### Python (fidelity harness)

```bash
cd eval
python -m venv .venv
source .venv/bin/activate        # or .venv\Scripts\activate on Windows
pip install -e ".[dev]"

# Run smoke tests (no LLM API call; uses fixture cache)
pytest eval/ -m "not live"

# Run full harness (requires LLM API credentials)
export ANTHROPIC_API_KEY=...
python run_fidelity.py --gate --profile smoke-first-consumer
```

#### Config file setup

```bash
# Copy the example config (do not commit local customizations)
cp tokenfold.example.toml tokenfold.toml
# Edit tokenfold.toml for your local dev preferences
# tokenfold.toml is in .gitignore
```

---

### Commit Message Style

```
<type>(<scope>): <short description>

# Types: feat, fix, chore, docs, test, perf, security, refactor
# Scope: core, cli, proxy, py, eval, ci, docs

feat(core): add log_compaction transform (lossy, --experimental)
fix(core): preserve key order in json_minify for nested arrays
test(core): add property tests for json_minify idempotency
docs: add prototype-then-port workflow to contributing guide
security(core): add JWT redaction fixture to regression suite
```

For transform additions: always note the transform ID, mode, and whether it's behind `--experimental`.

For any change to public API types: note the version bump.

---

### PR Requirements

| PR type | Required |
|---------|----------|
| New transform (lossless) | Golden fixtures, unit tests, mode matrix entry |
| New transform (lossy) | All of the above + fidelity gate green + port spec |
| API type change | Changelog entry, version bump, all surfaces updated |
| Security change | Security-focused reviewer tagged, regression fixture added |
| Any | `cargo fmt --all --check`, `cargo clippy ... -D warnings`, `cargo test --workspace` all green |

