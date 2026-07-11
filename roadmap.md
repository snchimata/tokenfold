# tokenfold — Roadmap (Phases · Features · Decisions)

> **Status: pre-code proposal.** Planning docs and empty scaffolding only — no source, tests, CI, or artifacts exist. All features and criteria are proposed; unchecked boxes are not evidence of implementation.

Covers project phases (Part 1), feature definitions (Part 2), and open decisions (Part 3). See PLAN.md for the specification, INTERFACES.md for contracts, ENGINEERING.md for build/test/risk, and TOUCHPOINTS.md for competitive coverage.

## Part 1 — Project Phases

Entry gate, PLAN.md tasks, exit criteria, and key deliverables per phase.

### Phase 0: Pre-Code Gates

**Goal:** Resolve the decisions and provisioning that truly block Phase 1 code from being written.

**Entry criteria:** PLAN.md approved by engineering owner.

**Tasks:** None (decisions and provisioning only)

**Work items:**
- Complete First Consumer worksheet (PLAN.md § First Consumer & Success Definition)
- Resolve Phase 0 blockers D-001, D-002, D-003, and D-006
- Confirm internal Cargo registry URL and credentials (D-003)
- Legal/brand approval on package name (D-001)
- CI system selected and repo created (D-006)
- Start D-004/D-005/D-007 discovery, but those decisions are resolved at their listed blocking phases

**Exit criteria (all must be checked before Phase 1 starts):**
- [x] First Consumer worksheet signed off by named owner *(see plan.md § First Consumer & Success Definition; resolved 2026-07-11)*
- [x] Package name approved (registry check clean, no legal hold) *(`tokenfold` — crates.io and PyPI both clean; solo project, no legal team — see D-001)*
- [x] ~~Internal Cargo registry URL confirmed and credentials provisioned~~ Resolved as N/A: solo/open-source project publishes directly to public crates.io/PyPI — see D-003
- [x] CI system selected and repository created *(GitHub Actions; local git repo initialized 2026-07-11 — see D-006)*

**Deliverables:** Phase 0 decision cards resolved, provisioned CI environment, internal repo created.

**Resolved 2026-07-11:** This is a solo/open-source project, not an internal-org effort — see D-001, D-002, D-003, D-006 in the Decision Log below. Downstream phases (4+) still reference an "internal Artifactory" for release publishing; that will need reconciling to GitHub Releases/crates.io when Phase 4 is scoped, but is out of Phase 0's scope.

### Phase 1: Core Engine  *(v0.1-alpha)*

**Goal:** A buildable, testable Rust workspace with correct types, token estimation, and budget planning. No transforms yet.

**Entry criteria:** All Phase 0 exit criteria checked.

**Tasks (PLAN.md):**
- Task 1: Scaffold Rust Workspace
- Task 2: Core Types, Reports, Status, Errors
- Task 3: Token Estimation and Budget Planning

**Exit criteria:**
- [ ] `cargo fmt --all --check` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] `cargo test -p tokenfold-core` green (types, report math, estimator, budget)
- [ ] `ByteHeuristicEstimator` and `TiktokenEstimator` have smoke tests; HF and Anthropic estimators remain v0.1.x unless D-004 pulls them forward
- [ ] `CompressionReport.estimator` field correctly propagates backend name
- [ ] `Status::UnreachableTarget` returns best-effort output (never panics)
- [ ] `CompressionPolicy.cache_boundary` field exists but enforcement is gated by F-044/D-013 in Phase 2
- [ ] `compress_with_estimator` seam exists and accepts `&dyn TokenEstimator`
- [ ] `cargo build --workspace --locked` succeeds cleanly

**Deliverables:** `tokenfold-core` crate with all public types, trait, heuristic estimator, mode enum, policy builder. No transforms.

### Phase 2: Transforms + Fidelity Bootstrap  *(v0.1-beta)*

**Goal:** Lossless transforms default-enabled; lossy transforms behind `--experimental`; fidelity smoke gate operational.

**Entry criteria:** Phase 1 complete.

**Tasks (PLAN.md):**
- Task 3.5: Fidelity Gate Contract and Mode Matrix
- Task 4: Deterministic Transforms
- Task 5: Pipeline, Floor, Safety Gates

**Exit criteria:**
- [ ] `json_minify`, `schema_compaction`, `secret_redaction` pass all golden tests
- [ ] `log_compaction`, `diff_compaction` exist and remain gated behind `--experimental` until fidelity approval promotes them
- [ ] Fidelity smoke gate (`python eval/run_fidelity.py --gate --profile smoke-first-consumer`) exits 0 and emits a green JSON artifact
- [ ] Mode matrix fixture (`tests/fixtures/mode_matrix.toml`) exists; tests assert against it
- [ ] `pipeline::compress` on `UnreachableTarget` fixture returns correct status + best-effort output
- [ ] Per-transform safety rollback test passes (transform that would drop a required field is rejected)
- [ ] Cross-platform golden byte-equality test passes (macOS/Linux produce identical bytes) in CI
- [ ] `cargo test -p tokenfold-core transforms` and `cargo test -p tokenfold-core pipeline` and `cargo test -p tokenfold-core safety` are all green
- [ ] `secret_redaction` cannot be disabled via `--disable`; unsafe bypass emits audit event

**Deliverables:** Working compression pipeline with lossless transforms, safety gates, and a minimal fidelity harness.

### Phase 3: CLI  *(v0.1-rc)*

**Goal:** All documented CLI examples run verbatim; human-readable output matches the Output & UX spec.

**Entry criteria:** Phase 2 complete; fidelity smoke gate green.

**Tasks (PLAN.md):**
- Task 6: CLI

**Exit criteria:**
- [ ] Every example in PLAN.md § CLI runs verbatim
- [ ] `tokenfold inspect` renders: verdict line, transform table, totals row, warnings block
- [ ] `tokenfold compress - --format text` reads from stdin correctly
- [ ] `tokenfold wrap -- git diff` runs the command and compresses its output
- [ ] `tokenfold inspect --json` emits only the report JSON to stdout
- [ ] `tokenfold compress --disable secret_redaction` is rejected with a clear error; exits non-zero
- [ ] `tokenfold inspect` with no `--target-tokens` shows max achievable savings per transform
- [ ] Exit codes match the Error Taxonomy table in PLAN.md
- [ ] `NO_COLOR` env var and non-tty detection suppress ANSI codes
- [ ] Config precedence enforced: flags > env > `tokenfold.toml` > built-in defaults
- [ ] `cargo test -p tokenfold-cli` green

**Deliverables:** `tokenfold` binary producing correct output for all v0.1 use cases.

### Phase 4: Benchmarks + Release Hardening  *(v0.1)*

**Goal:** All v0.1 release gates pass; native executables are published to Artifactory (fully static for Linux musl targets; self-contained native builds for macOS and Windows).

**Entry criteria:** Phase 3 complete; fixture compliance review complete.

**Tasks (PLAN.md):**
- Task 7: Benchmarks
- Task 11: Documentation and Release Gates

**Exit criteria (all blocking):**
- [ ] `cargo fmt --all --check` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] `cargo test --workspace --locked` passes
- [ ] `cargo audit` — 0 unfixed RUSTSEC advisories
- [ ] `cargo deny check advisories bans licenses sources` passes
- [ ] `cargo bench --workspace` — no regression beyond thresholds defined in `benches/THRESHOLDS.toml`
- [ ] `python eval/run_fidelity.py --gate --profile smoke-first-consumer` green
- [ ] Cross-platform golden byte-equality passes: macOS arm64, macOS x86_64, Linux x86_64-musl
- [ ] SBOM generated (`cargo-cyclonedx`) and attached to release
- [ ] Prebuilt native executables for all 5 targets published to internal Artifactory with checksums and signatures; Linux musl builds are fully static
- [ ] Install one-liner verified on a clean macOS and a clean Linux environment
- [ ] `ENGINEERING.md` or `CHANGELOG.md` release notes include a reproducible benchmark run with exact tokenizer, fixture hashes, and all required metrics
- [ ] Every fixture in `tests/fixtures/**` and `eval/tasks/**` has: data classification, license/source note, PII scan result, retention owner, approval record
- [ ] README quickstart verified: install one-liner + `git diff | tokenfold` produces expected output

**Deliverables (proposed):** v0.1 release, internal executables on Artifactory, and a verified first-consumer installation path.

### Phase 5: Extended Surfaces + Portable Parity  *(v0.2)*

**Goal:** HTTP proxy binary, Python wheel, MCP server, reversible local retrieval, stats/ledger, declarative command filters, and full fidelity harness operational. Lossy transforms promoted from `--experimental`.

**Entry criteria:** Phase 4 complete (v0.1 stable); Rust API stable; D-009 (Python wheel repo) resolved; D-008 (proxy timing) confirmed.

**Tasks (PLAN.md):**
- Task 8: Optional HTTP Proxy
- Task 9: Full Fidelity Harness
- Task 10: Optional Python Binding
- New tasks: MCP stdio server, reversible evidence store, stats ledger, and declarative command filter registry

**Exit criteria:**
- [ ] `tokenfold-proxy`: compressed JSON when enabled; SSE passes through unbuffered; conflicting-framing requests rejected; no credential value appears in logs
- [ ] Full fidelity harness: accuracy@ratio curves, contrastive KPI, needle tests, all content types in scope
- [ ] `log_compaction` and `diff_compaction` promoted out of `--experimental` after fidelity approval
- [ ] Python binding: `compress_openai_payload` works from Python ≥3.9; wheel published to internal PyPI
- [ ] MCP server exposes `tokenfold_compress`, `tokenfold_inspect`, `tokenfold_retrieve`, and `tokenfold_stats` when backing stores are enabled
- [ ] Reversible evidence store restores exact bytes for eligible stored spans and enforces TTL/size limits; secret-matched spans are never persisted or retrievable
- [ ] `tokenfold stats` aggregates report files and optional redacted local ledger data with JSON/CSV output
- [ ] Declarative command filters cover first-consumer top commands and pass `filters verify`
- [ ] All optional surface gates from PLAN.md § Task 11 pass
- [ ] Proxy contract in `INTERFACES.md` complete and accurate

**Deliverables (proposed):** v0.2 release with `tokenfold-proxy`, MCP server mode, Python wheel, local retrieval/stat surfaces, and first-consumer command filters on internal registries. Lossy transforms become default-available only after their blocking fidelity gates pass.

### Phase 6: Full Headroom Parity Extensions  *(v0.3+)*

**Goal:** Support every remaining Headroom capability through optional packages or sidecars while preserving the v0.1/v0.2 static-binary default path.

**Entry criteria:** Phase 5 complete; D-014 (parity extension boundary) resolved; at least one first consumer requests each optional extension being implemented.

**Tasks:** New extension plans for framework adapters, RAG/vector retrieval, output-shaping holdouts, image/multimodal compression, learn/session mining, and auth/update/admin commands.

**Exit criteria:**
- [ ] Framework adapters pass provider-shape parity tests for scoped SDKs/frameworks
- [ ] RAG/vector extension passes retrieval QA and citation-grounding fidelity gates
- [ ] Output-shaping reports separate measured/estimated output-token savings from exact input-token savings
- [ ] Image/multimodal extension has lossless metadata and lossy OCR/summarization gates separated
- [ ] `discover`/`learn` proposes policy changes without silently changing defaults
- [ ] `update` verifies signatures/checksums and supports rollback through internal release metadata

**Deliverables (proposed):** v0.3+ optional extension releases. The default CLI/proxy/hook path is intended to run without Python, ONNX, HF models, vector DBs, or provider SDK dependencies.

### Phase Dependency Graph

```
Phase 0  Pre-Code Gates
    │
    ▼
Phase 1  Core Engine           (v0.1-alpha)
    │
    ▼
Phase 2  Transforms + Fidelity (v0.1-beta)
    │
    ▼
Phase 3  CLI                   (v0.1-rc)
    │
    ▼
Phase 4  Benchmarks + Release  (v0.1)  ◀── ship here
    │
    ├──▶ Task 8: Proxy          ┐
    ├──▶ Task 9: Fidelity Full  ├─ parallel, any order
    ├──▶ Task 10: Python Binding│
    ├──▶ MCP / Retrieve / Stats │
    └──▶ Command Filter Registry┘
                                │
                                ▼
                    Phase 5  Extended Surfaces + Portable Parity (v0.2)
                                │
                                ▼
                    Phase 6  Full Headroom Parity Extensions (v0.3+)
```

### Phase Summary

| Phase | Name | PLAN Tasks | Ship milestone | Key gate |
|-------|------|------------|----------------|----------|
| 0 | Pre-Code Gates | - | - | All decisions resolved; repo provisioned |
| 1 | Core Engine | 1, 2, 3 | v0.1-alpha | `cargo test -p tokenfold-core` green |
| 2 | Transforms + Fidelity Bootstrap | 3.5, 4, 5 | v0.1-beta | Fidelity smoke gate green |
| 3 | CLI | 6 | v0.1-rc | All CLI examples run verbatim |
| 4 | Benchmarks + Release | 7, 11 | **v0.1** | All release gates pass; binary published |
| 5 | Extended Surfaces + Portable Parity | 8, 9, 10 + parity surfaces | **v0.2** | Optional surface gates pass |
| 6 | Full Headroom Parity Extensions | Extension-specific plans | **v0.3+** | Optional runtime gates pass |

## Part 2 — Feature Definitions

Feature IDs (`F-NNN`) are stable. Each entry includes version, acceptance criteria, and dependencies.

### Core Engine Features

#### F-001 · Token Budget Planner
**Version:** v0.1 | **Category:** Infrastructure

Computes the protected-content floor, selects the correct tokenizer backend for the given input format and policy, applies the ordered transform pipeline, and returns a typed `CompressionOutput` with a `CompressionReport`.

**Acceptance criteria:**
- [ ] `compress(input, policy)` returns `Ok(CompressionOutput)` for all valid inputs
- [ ] `Status::Passthrough` returned when `input_tokens ≤ target_tokens` (no-op; input bytes unchanged)
- [ ] `Status::UnreachableTarget` returned with `CompressionReport.budget.protected_floor` and `.achieved_tokens` populated (never panics) when `target < protected_floor`
- [ ] No target set → pipeline runs to the safe floor; reports real savings
- [ ] `CompressionReport.estimator` is an `EstimatorInfo` struct with `backend` / `model` / `is_exact` (e.g. `backend="tiktoken"`, `model="o200k_base"`, `is_exact=true`; heuristic has `model=null`, `is_exact=false`)
- [ ] `compress_with_estimator(input, policy, &dyn TokenEstimator)` exists; mock estimators work in tests

**Dependencies:** F-002 (estimators)

#### F-002 · Pluggable Token Estimators
**Version:** v0.1 (heuristic + tiktoken); v0.1.x (HF + Anthropic API) | **Category:** Infrastructure

Proposed `TokenEstimator` trait with four implementations over time. Phase 1/v0.1 scope is heuristic + tiktoken; HF and Anthropic are v0.1.x unless D-004 explicitly pulls one forward.

| Backend | Crate | Target models |
|---------|-------|---------------|
| `ByteHeuristicEstimator` | none | Pre-filter only (fast; under-counts dense JSON/code) |
| `TiktokenEstimator` | `tiktoken-rs` | OpenAI (`o200k_base`, `cl100k_base`) |
| `HfTokenizerEstimator` | `huggingface/tokenizers` | Llama, HF-format models |
| `AnthropicApiEstimator` | HTTP (`/messages/count_tokens`) | Claude (no public tokenizer) |

**Acceptance criteria:**
- [ ] `ByteHeuristicEstimator::count_bytes` rounds up for dense formats: `bytes.len().div_ceil(4)`
- [ ] `ByteHeuristicEstimator::count_bytes(b"")` returns 0
- [ ] `TiktokenEstimator` produces byte-identical counts to the reference implementation on golden fixtures
- [ ] `AnthropicApiEstimator` calls the Anthropic API; never approximates Claude with tiktoken
- [ ] Heuristic numbers are prefixed `~` and labeled `EST TOKENS` in all output surfaces
- [ ] Exact-backend unavailability: fails closed for budget decisions OR proceeds with `--allow-heuristic-budget` (user opt-in); report always labels which was used
- [ ] CI injects a deterministic mock estimator via `compress_with_estimator` (no real API call in unit tests)
- [ ] `CompressionReport.estimator.backend` is always one of `heuristic` / `tiktoken` / `huggingface` / `anthropic`, with `model` set accordingly (heuristic `bytes/4` has no model); the `X-TokenFold-Estimator` proxy header renders the colon-joined `backend:model` form

**Dependencies:** none

#### F-003 · Typed Error Taxonomy
**Version:** v0.1 | **Category:** Infrastructure

`TokenFoldError` enum. `Status::UnreachableTarget` is a first-class typed outcome on `CompressionOutput`, not an error.

| Variant | CLI exit | Proxy HTTP | Python exception |
|---------|----------|-----------|-----------------|
| `InvalidInput` | 2 | 400 | `InvalidInputError` |
| `Status::Compressed` / `Passthrough` / `BestEffort` / `UnreachableTarget` | 0 | 200 + status header | returned status |
| `SafetyViolation` / `RedactionFailed` | 3 | 422 | `SafetyError` |
| `EstimatorError` | 4 | 503 | `EstimatorError` |
| `ConfigError` | 5 | 500 | `ConfigError` |
| `InternalError` / `Io` | 6 | 500 | `InternalError` / `OSError` |

**Acceptance criteria:**
- [ ] Each error/outcome exits with the code in the table above and in `INTERFACES.md §1.4` (tested via CLI integration test)
- [ ] `UnreachableTarget` returns `Ok(CompressionOutput)` with `Status::UnreachableTarget`, not `Err(...)`
- [ ] Proxy maps each variant to the correct HTTP status
- [ ] Python binding maps each variant to the correct Python exception class

**Dependencies:** none

#### F-004 · Transform Versioning
**Version:** v0.1 | **Category:** Infrastructure

Every transform carries a semantic version. `TransformReport` records `{ id, version, ... }`. Policies and proxies can pin exact versions through the canonical `transform_versions` policy/config map in `INTERFACES.md`; an unavailable or mismatched pin fails with `ConfigError` before transformation.

**Acceptance criteria:**
- [ ] Every transform exposes a stable canonical ID (e.g., `json_minify`) and a semver string (e.g., `1.0.0`)
- [ ] `TransformReport.id` and `TransformReport.version` are present in all output surfaces
- [ ] Any behavior change in a transform is a version bump documented in CHANGELOG.md
- [ ] Proxy `X-TokenFold-*` headers include the applied transform IDs and versions
- [ ] Pinning a transform version in policy prevents a binary upgrade from silently changing behavior

**Dependencies:** none

#### F-005 · Determinism Contract
**Version:** v0.1 | **Category:** Infrastructure

Same input always produces same bytes on all supported platforms and architectures.

**Acceptance criteria:**
- [ ] `serde_json` `preserve_order` feature is active; no `HashMap`-iteration-order in any transform output
- [ ] Cross-platform golden byte-equality test passes: macOS arm64, macOS x86_64, Linux x86_64-musl
- [ ] No nondeterministic iteration (sorted output or indexmap-based where order matters)
- [ ] Golden manifest (`tests/golden/MANIFEST.toml`) records SHA-256 of every expected output; CI fails on mismatch

**Dependencies:** none

### Transform Features

#### F-010 · JSON Minify (`json_minify`)
**Version:** v0.1 | **Mode:** Lossless | **Task-scope:** All

Strips insignificant whitespace from JSON. Never reorders keys.

**Acceptance criteria:**
- [ ] Output is valid JSON (parseable by `serde_json` after transform)
- [ ] Key order preserved byte-for-byte vs input (requires `preserve_order`)
- [ ] String content and escape sequences preserved byte-for-byte
- [ ] Number spelling preserved (no normalization of `1.0` → `1`)
- [ ] Duplicate-key fixture: behavior documented; keys not silently dropped
- [ ] Non-UTF-8 input → `InvalidInput` error
- [ ] Empty input → empty output (no error, no panic)
- [ ] Unterminated string in input → `JsonMinifyError::Invalid(...)`; no unreachable lexer-only error path
- [ ] Idempotent: `minify(minify(x)) == minify(x)` (byte-identical)

**Dependencies:** F-001, F-005

#### F-011 · Schema Compaction (`schema_compaction`)
**Version:** v0.1 | **Mode:** Semantics-preserving | **Task-scope:** All

Shortens `examples` arrays in JSON Schema and OpenAI tool definitions. Preserves all semantic fields.

**Acceptance criteria:**
- [ ] Tool/function `description` fields preserved byte-for-byte in Conservative mode
- [ ] `required`, `enum`, `type`, `default` fields preserved in all modes
- [ ] `examples` arrays shortened (count controlled by mode config)
- [ ] Tool/function `name` fields preserved
- [ ] No security-bearing field altered without a `SecurityFieldAltered` warning
- [ ] Golden fixture: 5-example schema → 1 example; all required fields intact; description byte-identical
- [ ] Invalid JSON input → `InvalidInput` error

**Dependencies:** F-010

#### F-012 · Log Compaction (`log_compaction`)
**Version:** v0.1 (behind `--experimental` until fidelity gate green) | **Mode:** Lossy w/ evidence | **Task-scope:** `General`, `ChangeSummary`

Collapses runs of three or more adjacent identical lines: first occurrence + `[repeated Nx]` evidence marker + last occurrence. Runs of one or two remain unchanged.

**Acceptance criteria:**
- [ ] Adjacent-duplicate lines collapsed: first + `[repeated Nx]` + last kept
- [ ] Non-adjacent duplicates NOT collapsed (documented limitation; tested)
- [ ] Relative line ordering preserved
- [ ] Timestamps preserved by default; opt-in removal via `--remove-timestamps`
- [ ] Evidence marker format: `[repeated 42x]`
- [ ] Single line input → unchanged (no evidence marker added)
- [ ] Empty input → empty output
- [ ] Fidelity gate green before this transform leaves `--experimental`

**Dependencies:** F-001, F-016 (fidelity gate)

#### F-013 · Diff Compaction (`diff_compaction`)
**Version:** v0.1 (behind `--experimental` until fidelity gate green) | **Mode:** Lossy w/ evidence | **Task-scope:** `CodeReview`, `ChangeSummary`

Keeps file names, hunk headers, and changed line bodies (`+`/`-`). Header-only form requires explicit `TaskScope::ChangeSummary`.

**Acceptance criteria:**
- [ ] Hunk headers (`@@`) preserved
- [ ] Changed `+`/`-` line bodies preserved in default form
- [ ] File names and `diff --git` headers preserved
- [ ] `TaskScope::ChangeSummary` required to enable header-only form (bodies dropped)
- [ ] Evidence markers indicate dropped unchanged-context lines
- [ ] Fidelity gate green before this transform leaves `--experimental`

**Dependencies:** F-001, F-016 (fidelity gate)

#### F-014 · Secret Redaction (`secret_redaction`)
**Version:** v0.1 | **Mode:** Safety transform (mandatory) | **Task-scope:** All

Linear-time regex + high-entropy/base64 heuristics. Runs before any observability or persistence boundary. Cannot be disabled by normal `--disable`.

**Acceptance criteria:**
- [ ] Runs before any report snippet, log line, trace, or on-disk output
- [ ] `--disable secret_redaction` rejected with a clear, actionable error message; exits non-zero
- [ ] `--unsafe-disable-redaction` (CLI-only escape hatch): emits `Critical` warning + writes audit event; forbidden in proxy mode
- [ ] `UnredactedContentPossible` warning always emitted (best-effort, not a guarantee)
- [ ] Linear-time `regex` crate only; no PCRE/backtracking crate in this path
- [ ] Regex engine pinned in `deny.toml` (ban `fancy-regex`, `pcre2` in redaction path)
- [ ] Regression fixtures pass: JWT, OpenAI API key pattern, AWS access key, basic-auth-in-URL, Bearer token

**Dependencies:** F-001

#### F-015 · Code Digest (`code_digest`)
**Version:** v0.2+ (behind `code-digest` Cargo feature) | **Mode:** Lossy w/ evidence | **Task-scope:** `ApiOverview`

Tree-sitter AST compression: function signatures, imports, public API surface, doc headers. Requires a C toolchain.

**Acceptance criteria:**
- [ ] Only compiled when `code-digest` Cargo feature is active (build fails gracefully without C toolchain)
- [ ] Validated for `TaskScope::ApiOverview` only; emits `SafetyDowngrade` warning and degrades for `Debugging`/`Generation`
- [ ] Minimum 17 language support
- [ ] Fidelity gate green for target task scope before shipping

**Dependencies:** F-001, F-016 (fidelity gate)

#### F-016 · Fidelity Gate (eval harness)
**Version:** v0.1 smoke; v0.2 full | **Mode:** Release gate | **Category:** Quality

Python-based paired original/compressed evaluation harness. Computes downstream task scores, contrastive failure rate, and accuracy@ratio curves. Blocking CI gate for all lossy transforms.

**Acceptance criteria:**
- [ ] Emits a pass/fail JSON artifact containing: profile ID, model version, fixture hashes, total cost, gate decision
- [ ] Gate condition: `quality_retention ≥ threshold` at shipped default ratio
- [ ] Contrastive failure rate (`raw_passes_compressed_fails`) reported; target near-zero at default mode
- [ ] Critical-token needle tests: both output-survival AND model answerability asserted
- [ ] Gate is a blocking CI step for any release that includes a lossy transform
- [ ] Eval results cached by (fixture hash × model version × transform config); re-run only on cache miss

**Dependencies:** none (pure Python; calls external LLM API)

#### F-017 · Conversation Compaction (`conversation`)
**Version:** v0.2+ | **Mode:** Policy-driven | **Task-scope:** `AgentHistory`

Structured extractive memory: decisions, constraints, entities, open tasks, IDs. Preserves earliest durable goal/constraint turns and all system/safety turns.

**Acceptance criteria:**
- [ ] Earliest durable goal/constraint turns preserved byte-for-byte
- [ ] System and safety turns preserved byte-for-byte
- [ ] Compaction depth capped (no unbounded collapse)
- [ ] Already-summarized blocks are never re-summarized
- [ ] Fidelity gate passes for `AgentHistory` task scope before enabling

**Dependencies:** F-001, F-016 (fidelity gate)

#### F-018 · Prose Extraction (`prose_extraction`)
**Version:** v0.2+ | **Mode:** Lossy w/ evidence | **Task-scope:** `RetrievalQa`

Query/task-aware BM25/TF-IDF selection. Coreference-safe unit selection; never splits a negation from its clause.

**Acceptance criteria:**
- [ ] Units selected are coreference-safe (no dangling pronoun after selection)
- [ ] Negations never split from their clause
- [ ] Fidelity gate passes for `RetrievalQa` task scope before enabling

**Dependencies:** F-001, F-016 (fidelity gate)

#### F-019 · Table Compaction (`table_compaction`)
**Version:** v0.1 (if first-consumer worksheet names tables as top-3 payload type; otherwise v0.2) | **Mode:** Semantics-preserving where possible | **Task-scope:** `All`

Keeps headers, row counts, sampled rows, min/max/null summaries.

**Acceptance criteria:**
- [ ] Column headers preserved byte-for-byte
- [ ] Row count preserved (not hidden)
- [ ] Sampled rows are representative (first + last + every Nth row)
- [ ] Min/max/null summaries added for numeric/date columns
- [ ] Schema: type/column metadata preserved

**Dependencies:** F-001

### CLI Features

#### F-020 · CLI Inspect
**Version:** v0.1

Dry-run preview. With no `--target-tokens`: shows max achievable savings per transform. With `--target-tokens`: shows reachability verdict.

**Acceptance criteria:**
- [ ] Renders (top-to-bottom): verdict line, transform table, totals row, warnings block
- [ ] No-target mode header: `No target set — showing max achievable savings per transform`
- [ ] Under-budget line: `UNDER budget: ~7,200 est. tokens ≤ target 12,000 — nothing to compress`, exit 0
- [ ] Reachable verdict: green; unreachable: yellow/warning
- [ ] Heuristic numbers prefixed `~`; column labeled `EST TOKENS`; exact numbers have no prefix, column labeled `TOKENS`
- [ ] `--json` emits only the versioned `CompressionReport` JSON to stdout
- [ ] Never dumps raw payload to stdout

**Dependencies:** F-001

#### F-021 · CLI Compress
**Version:** v0.1

Compress a payload. Payload → stdout. Human report → stderr.

**Acceptance criteria:**
- [ ] Compressed payload to stdout; human report to stderr (pipe-safe: `tokenfold compress | apply` is never corrupted)
- [ ] `--json`: report JSON to **stderr**; payload stays on stdout (pipe stays intact even with `--json`). NOTE: this differs from `inspect --json`, which puts the report on stdout because `inspect` emits no payload. See `INTERFACES.md` §1.3 (authoritative). To capture only the JSON report: `tokenfold compress f --json 2>report.json 1>/dev/null`
- [ ] `-` reads from stdin
- [ ] `-o path` writes payload to file
- [ ] `--disable <id>` works for all transforms except `secret_redaction`
- [ ] `--disable secret_redaction` is rejected; exits non-zero
- [ ] Exit codes match F-003 Error Taxonomy
- [ ] Config precedence: flags > env > `tokenfold.toml` > built-in defaults

**Dependencies:** F-001, F-020

#### F-022 · CLI Wrap
**Version:** v0.1

Run a command and compress its combined output. The `rtk`/`squeez` analog. `wrap` is the canonical name; `shell` is a visible alias and `exec` a hidden alias.

**Acceptance criteria:**
- [ ] `tokenfold wrap -- git diff` runs `git diff`, compresses stdout+stderr output
- [ ] Compressed output → stdout; report → stderr
- [ ] Subprocess exit code propagated when compression succeeds
- [ ] `tokenfold wrap` with no arguments prints usage; exits non-zero
- [ ] `shell` (visible) and `exec` (hidden) aliases resolve to `wrap` with identical behavior

**Dependencies:** F-021

#### F-023 · CLI Diff
**Version:** v0.1

Compression-aware diff of raw vs compressed files.

**Acceptance criteria:**
- [ ] Header: `raw ~18,400 → compressed ~11,900 est. tokens (35.3% reduction)`
- [ ] Removed regions dimmed; evidence markers highlighted
- [ ] Per-transform saved-token subtotals shown
- [ ] `--json` emits structured hunk list
- [ ] `NO_COLOR` / non-tty: no ANSI codes emitted

**Dependencies:** F-021

#### F-024 · CLI Benchmark
**Version:** v0.1

Benchmark mode: runs the bench suite against a set of fixture files; prints per-transform metrics.

**Acceptance criteria:**
- [ ] Accepts glob or explicit list of fixture paths
- [ ] Reports: exact token counts, savings ratio, p95 latency, bytes allocated per transform
- [ ] Never uses heuristic estimator in benchmark output (exact backend required)
- [ ] `--json` emits machine-readable results

**Dependencies:** F-001, F-002

### Distribution Features

#### F-030 · Static Binary Distribution
**Version:** v0.1

Prebuilt native executables for all supported platforms, published to internal Artifactory. Linux musl targets are fully static; macOS and Windows artifacts are native self-contained executables but are not described as fully static.

**Platform matrix:**

| Target triple | Platform |
|--------------|---------|
| `x86_64-apple-darwin` | macOS Intel |
| `aarch64-apple-darwin` | macOS Apple Silicon |
| `x86_64-unknown-linux-musl` | Linux x86_64 (static) |
| `aarch64-unknown-linux-musl` | Linux ARM64 (static) |
| `x86_64-pc-windows-msvc` | Windows x86_64 |

**Acceptance criteria:**
- [ ] Linux musl binaries are fully static; macOS and Windows executables have no project-managed language runtime dependency and satisfy their documented OS-library requirements
- [ ] Each binary ships with a SHA-256 checksum and detached signature
- [ ] Install one-liner verified on a clean macOS arm64 and a clean Linux x86_64 environment
- [ ] macOS quarantine handling documented (`xattr -d com.apple.quarantine`)
- [ ] `tokenfold --version` outputs the version string in all binaries
- [ ] Shell completion scripts generated and included

**Dependencies:** F-020, F-021, F-022, F-023, F-024

### v0.2 Surface Features

#### F-040 · HTTP Proxy (`tokenfold-proxy`)
**Version:** v0.2 | **Separate binary**

**Acceptance criteria:**
- [ ] Separate `tokenfold-proxy` binary (not a CLI subcommand)
- [ ] SSE/streaming responses pass through unbuffered
- [ ] Non-streaming JSON requests buffered up to `max_body_bytes`; rejected if larger
- [ ] `Content-Length` recomputed after body rewrite; conflicting `CL`+`TE` rejected
- [ ] `X-TokenFold-*` response headers attached
- [ ] No credential header value appears in logs
- [ ] Upstream TLS validated; `http://` rejected without `--insecure-upstream`
- [ ] Binds loopback by default
- [ ] Startup rejects `unsafe_disable_redaction = true` in proxy mode
- [ ] Non-loopback bind requires explicit `allow_non_loopback_bind` / flag opt-in
- [ ] `CL.TE` and `TE.CL` conflicting-framing fixtures are rejected

**Dependencies:** F-001, F-014 (redaction)

#### F-041 · Python Binding (`tokenfold-py`)
**Version:** v0.2 | **abi3 wheel (Python ≥3.9)**

**Acceptance criteria:**
- [ ] `compress_openai_payload(payload, policy=...)` works from Python ≥3.9
- [ ] `CompressionPolicy(target_tokens=..., mode=CompressionMode.BALANCED)` constructor
- [ ] `result.report.saved_tokens`, `.estimator`, `.status` accessible
- [ ] `TokenFoldError` variants map to correct Python exception classes
- [ ] Single abi3 wheel per platform; published to internal PyPI

**Dependencies:** F-001 (via pyo3/maturin)

### Competitive-Parity Features (added July 2026 review)

Parity features map Headroom/RTK gaps to portable Rust paths or optional extensions; implementation depends on resolved decisions and acceptance gates.

#### F-042 · MCP Server Surface (`tokenfold mcp serve`)
**Version:** v0.2 (gated on D-011 / first-consumer adoption path) | **Category:** Touchpoint

Exposes compression as MCP tools over stdio so any MCP-capable agent can call it without a shell hook or proxy. Analog of headroom's `mcp serve`.

**Acceptance criteria:**
- [ ] `tokenfold mcp serve` runs a stdio MCP server wrapping `tokenfold_core::compress`
- [ ] Exposes `tokenfold_compress` (compress content), `tokenfold_inspect` (dry-run preview), `tokenfold_retrieve` (F-045), and `tokenfold_stats` (F-046) tools when their backing stores are enabled
- [ ] Tool schemas validate against the MCP spec; `tokenfold mcp install` registers with a client config
- [ ] Reuses the core `CompressionReport`; no new report schema
- [ ] Retrieval is local/content-addressed by default and never requires a vector DB or ML runtime

**Dependencies:** F-001, F-045, F-046

#### F-043 · Durable Agent Integration (`tokenfold init` / `uninit`)
**Version:** v0.1 (first-consumer host only) → v0.2 (full host set) | **Category:** Touchpoint

One-command install/removal of per-host hooks (PreToolUse / rewrite-rule files). Closes the biggest adoption-friction gap vs `rtk init` and `headroom wrap`/`unwrap`.

**Acceptance criteria:**
- [ ] `tokenfold init --agent <name>` writes the correct hook/rewrite file for the host (Claude Code settings, `hooks.json`, `.clinerules`, `.windsurfrules`, …)
- [ ] `tokenfold uninit --agent <name>` removes it, leaving the host config byte-identical to its pre-init state
- [ ] `tokenfold doctor` verifies the hook is installed and estimator backends are reachable (headroom `doctor` analog)
- [ ] v0.1 covers only the first consumer's host (scope from D-002); unknown hosts print a clear "not yet supported" message
- [ ] Idempotent: running `init` twice does not duplicate the hook

**Dependencies:** F-021 for the v0.1 first-host hook path; F-030 only for packaged static-binary distribution in Phase 4+

#### F-044 · Prompt-Cache Prefix Preservation
**Version:** v0.1 | **Category:** Infrastructure (correctness)

Guarantees tokenfold never rewrites bytes inside a caller-declared cached prefix, so provider prompt caching stays warm. Prevents the failure mode where compression reduces token count but *increases* cost by invalidating the cache. Analog of headroom's `CacheAligner`.

**Acceptance criteria:**
- [ ] `CompressionPolicy.cache_boundary: Option<CacheBoundary>` (byte offset or turn index)
- [ ] All content before the boundary is folded into `protected_floor` (never transformed)
- [ ] Any transform that would touch the frozen prefix is rolled back with a `PrefixModified` warning
- [ ] Golden test: a payload with a declared prefix is byte-identical before the boundary after compression
- [ ] Default (`None`): current behavior (only key order + latest user message preserved)

**Dependencies:** F-001, D-013

#### F-045 · Reversible Evidence Store and Retrieval (`tokenfold retrieve`)
**Version:** v0.2 | **Category:** Fidelity / Memory

Proposes reversible retrieval without making tokenfold a shared-memory platform. When explicitly enabled, compression may write eligible, post-redaction original spans to a local permission-restricted store and emit bounded retrieval markers. Spans matching secret-redaction rules are never persisted and are explicitly reported as non-retrievable; retrieval restores exact bytes only for eligible stored spans.

**Acceptance criteria:**
- [ ] `tokenfold compress --store-originals` writes originals to a local store keyed by SHA-256/BLAKE3 content hash
- [ ] Compressed output may include retrieval markers only in formats that can carry comments/metadata safely; otherwise markers live in the `CompressionReport`
- [ ] `tokenfold retrieve <marker-or-report>` restores exact original bytes for eligible stored spans
- [ ] `tokenfold mcp serve` exposes `tokenfold_retrieve` when the store is enabled
- [ ] Store has TTL, max-size, per-project namespace, and `tokenfold retrieve gc` cleanup
- [ ] Secret-matched spans are never persisted or retrievable on any surface; there is no unsafe persistence override

**Dependencies:** F-014, F-016

#### F-046 · Savings Ledger, Stats, and Dashboard Export (`tokenfold stats` / `gain` / `session`)
**Version:** v0.2 | **Category:** Observability

Covers Headroom `stats`/`dashboard` and provides first-class RTK `gain`/`session` analog commands over a report-first ledger. `stats`, `gain`, and `session` all emit the stable `StatsSummary` shape (see `INTERFACES.md §7.1`). The default is static, scriptable summaries; a live dashboard is an optional export, not required for the core binary.

**Acceptance criteria:**
- [ ] `tokenfold stats <report-glob>` aggregates `CompressionReport` JSON files by project, transform, format, estimator, and status
- [ ] `tokenfold gain [--scope project|user] [--since 30d] [--json|--csv]` summarizes realized token/cost savings from report and ledger data
- [ ] `tokenfold session [--recent N] [--json]` reports host-session coverage: total, wrapped, and raw commands, bypasses, and `coverage_pct`
- [ ] Optional local ledger records redacted report metadata only; raw payload bytes are never stored
- [ ] `tokenfold stats --json` and `--csv` are stable machine contracts; `gain`/`session` share the `StatsSummary` schema
- [ ] `tokenfold stats --serve` serves a loopback-only static dashboard from ledger/report data
- [ ] Cost savings are labeled `measured`, `estimated`, or `heuristic` based on estimator and provider pricing provenance

**Dependencies:** F-001, F-020, F-021

#### F-047 · Declarative Command Filter Registry
**Version:** v0.2 | **Category:** CLI / Shell-wrap

Adds RTK-style command-output breadth without hardcoding 100+ Rust handlers up front. Filters are versioned, testable TOML pipelines that run before or alongside generic `log_compaction`.

**Acceptance criteria:**
- [ ] Built-in filters cover the first consumer's top commands from D-002
- [ ] Project/user/built-in filter precedence is deterministic and documented
- [ ] `tokenfold filters list|verify|trust` validates schemas, regex safety, fixtures, and expected token deltas
- [ ] A `never_worse` guard falls back to raw output when exact or allowed heuristic counts show no savings
- [ ] Custom filters cannot run shell code; they are declarative transforms only

**Dependencies:** F-012, F-022

#### F-048 · Framework Adapter Pack
**Version:** v0.3 | **Category:** Library / Integration

Supports Headroom's adapter breadth as optional packages layered on the stable core API: OpenAI, Anthropic, LiteLLM, LangChain, Vercel AI SDK, Agno/Strands, and ASGI middleware. The core Rust CLI does not depend on these packages.

**Acceptance criteria:**
- [ ] Adapter APIs preserve provider request/response shapes except for documented compressed fields
- [ ] Each adapter has fixture parity tests against raw SDK calls or mocked provider requests
- [ ] Adapter packages are independently publishable and versioned against the `CompressionReport.schema_version`
- [ ] First-consumer adapters ship first; broad adapter coverage is not allowed to block v0.1/v0.2 core releases

**Dependencies:** F-041, F-040

#### F-049 · RAG and Vector Retrieval Extension
**Version:** v0.3 | **Category:** Optional ML / Retrieval

Supports Headroom's RAG chunk routing and vector retrieval as an optional sidecar or Python extension. It is not part of the static binary default path.

**Acceptance criteria:**
- [ ] `tokenfold-rag` accepts retrieval chunks with IDs, scores, source metadata, and query context
- [ ] Default path uses deterministic BM25/TF-IDF selection; optional vector mode requires explicit install/runtime flags
- [ ] Chunk routing preserves citations, source IDs, scores, and required spans
- [ ] Fidelity harness includes RAG QA and citation-grounding tasks before any RAG transform becomes default
- [ ] Multi-worker proxy deployments require a shared retrieval backend or sticky sessions when retrieval cache is enabled

**Dependencies:** F-018, F-045, F-016

#### F-050 · Output-Shaping and Holdout Measurement
**Version:** v0.3 | **Category:** Optional Policy / Evaluation

Supports Headroom-style output-token savings through deterministic instruction profiles and live holdout measurement. This is a policy layer, not a compression transform, and it never claims savings without measurement provenance.

**Acceptance criteria:**
- [ ] Optional `--output-profile terse|standard|none` appends provider-safe response-shaping instructions outside frozen prefixes
- [ ] Reports separate input-token savings from output-token savings
- [ ] Proxy can run holdout sampling comparing raw vs compressed/output-shaped requests when explicitly enabled
- [ ] Output-savings claims are labeled estimated/measured and never mixed with exact input-token savings

**Dependencies:** F-040, F-016, F-044

#### F-051 · Image and Multimodal Compression Extension
**Version:** v0.3+ | **Category:** Optional ML / Multimodal

Supports Headroom's image/multimodal lane through optional preprocessing, OCR, metadata stripping, and image summarization packages. The static binary default remains text/token oriented.

**Acceptance criteria:**
- [ ] Image handling is disabled by default and requires an optional package/feature
- [ ] Lossless metadata stripping is separated from lossy OCR/summarization
- [ ] Reports identify modality, transform IDs, and quality/eval provenance
- [ ] No image transform ships as default until a downstream multimodal fidelity gate exists

**Dependencies:** F-016

#### F-052 · Learn, Session Mining, and Auto-Tuning
**Version:** v0.3 | **Category:** Observability / Policy

Supports Headroom `learn` and RTK `discover/session` through privacy-preserving mining of local reports, shell-wrap sessions, and agent hooks. It proposes policy changes but never silently changes defaults.

**Acceptance criteria:**
- [ ] `tokenfold discover` identifies high-token uncompressed tool outputs from local report/session metadata
- [ ] `tokenfold learn` proposes filter, mode, or target-token changes with before/after evidence
- [ ] Auto-tuning recommendations require explicit approval and are written to config with provenance comments
- [ ] Raw payload capture is opt-in, redacted, retention-limited, and disabled in CI/proxy by default

**Dependencies:** F-046, F-047

#### F-053 · Auth, Update, and Admin Commands
**Version:** v0.3 | **Category:** Operations

Supports Headroom's auth/update/admin ergonomics without storing provider credentials in tokenfold. Auth is limited to proxy/admin access and provider pass-through validation.

**Acceptance criteria:**
- [ ] Proxy supports loopback-only default admin endpoints and optional inbound bearer auth for non-loopback deployments
- [ ] `tokenfold update` checks internal release metadata, verifies signatures/checksums, and supports rollback
- [ ] `tokenfold auth doctor` verifies pass-through provider credentials without persisting secret values
- [ ] Logs and reports never include bearer/API-token values

**Dependencies:** F-030, F-040

### Feature → Phase Mapping

| Feature | Phase | Version |
|---------|-------|---------|
| F-001 Budget Planner | 1 | v0.1 |
| F-002 Estimators | 1 | v0.1 |
| F-003 Error Taxonomy | 1 | v0.1 |
| F-004 Transform Versioning | 1 | v0.1 |
| F-005 Determinism Contract | 2 | v0.1 |
| F-010 JSON Minify | 2 | v0.1 |
| F-011 Schema Compaction | 2 | v0.1 |
| F-012 Log Compaction | 2 (experimental) → 5 (default) | v0.1 / v0.2 |
| F-013 Diff Compaction | 2 (experimental) → 5 (default) | v0.1 / v0.2 |
| F-014 Secret Redaction | 2 | v0.1 |
| F-016 Fidelity Gate (smoke) | 2 | v0.1 |
| F-019 Table Compaction | 2 (if first-consumer) | v0.1 or v0.2 |
| F-020 CLI Inspect | 3 | v0.1 |
| F-021 CLI Compress | 3 | v0.1 |
| F-022 CLI Wrap | 3 | v0.1 |
| F-023 CLI Diff | 3 | v0.1 |
| F-024 CLI Benchmark | 4 | v0.1 |
| F-030 Static Binary Distribution | 4 | v0.1 |
| F-015 Code Digest | 5 | v0.2+ |
| F-016 Fidelity Gate (full) | 5 | v0.2 |
| F-017 Conversation Compaction | 5 | v0.2+ |
| F-018 Prose Extraction | 5 | v0.2+ |
| F-040 HTTP Proxy | 5 | v0.2 |
| F-041 Python Binding | 5 | v0.2 |
| F-044 Prompt-Cache Prefix Preservation | 2 | v0.1 |
| F-043 Durable Agent Integration | 3 (first host) → 5 (full) | v0.1 / v0.2 |
| F-042 MCP Server Surface | 5 | v0.2 |
| F-045 Reversible Evidence Store and Retrieval | 5 | v0.2 |
| F-046 Savings Ledger, Stats, and Dashboard Export | 5 | v0.2 |
| F-047 Declarative Command Filter Registry | 5 | v0.2 |
| F-048 Framework Adapter Pack | 6 | v0.3 |
| F-049 RAG and Vector Retrieval Extension | 6 | v0.3 |
| F-050 Output-Shaping and Holdout Measurement | 6 | v0.3 |
| F-051 Image and Multimodal Compression Extension | 6 | v0.3+ |
| F-052 Learn, Session Mining, and Auto-Tuning | 6 | v0.3 |
| F-053 Auth, Update, and Admin Commands | 6 | v0.3 |

## Part 3 — Open Decisions

### Blocking — Phase 0

*(D-001, D-002, D-003, D-006 resolved 2026-07-11 — moved to the Decision Log below.)*

#### D-004 · v0.1 Payload Formats
**Status:** OPEN
**Blocks:** Phase 2 start (transform scope, estimator selection, and fidelity fixture set depend on this)
**Owner:** TBD
**Deadline:** Before Phase 1 ends

**Options:**

| Option | Formats | Notes |
|--------|---------|-------|
| **Narrow (recommended)** | Plain text/command output, Git diff, OpenAI-compatible JSON | Derive from the First Consumer worksheet's "dominant payload types" |
| Broader | + Anthropic JSON | Adds `AnthropicApiEstimator` dependency and a second fixture set in v0.1 |
| Full | All formats | Significantly increases v0.1 scope; not recommended |

**Recommendation:** Narrow. Include only what the First Consumer worksheet's dominant payload types require. Anthropic JSON is a v0.1.x or v0.2 addition unless the first consumer's top payloads require it.

**Decision:** *(fill in when resolved)*
**Rationale:** *(fill in when resolved)*

#### D-005 · Fidelity Gate Thresholds
**Status:** OPEN
**Blocks:** Phase 2 start (Task 3.5 cannot finalize the mode matrix without agreed thresholds)
**Owner:** TBD
**Deadline:** Before Phase 2 starts

**Required threshold values:**

| Threshold | Description | Draft value | Final value |
|-----------|-------------|-------------|-------------|
| Conservative mode: max downstream-score drop | Absolute drop at default ratio | ≤ 2% | *(fill in)* |
| Balanced mode: max downstream-score drop | Absolute drop at default ratio | ≤ 5% | *(fill in)* |
| Aggressive mode: max downstream-score drop | Absolute drop at default ratio | ≤ 10% | *(fill in)* |
| Contrastive failure rate ceiling | % of tasks that pass raw but fail compressed | ≤ 0.5% at Balanced | *(fill in)* |
| Critical-token survival rate floor | % of injected critical tokens that survive in output | ≥ 99% at Conservative | *(fill in)* |

**Note:** Draft thresholds can be agreed pre-Phase 2 using baselines from the research summary (ACON, kompact, LLMLingua). Final values are set after Phase 2 accuracy@ratio data is collected. The gate must exist before any lossy transform leaves `--experimental`.

**Decision:** *(fill in when resolved)*
**Rationale:** *(fill in when resolved)*

*(D-006 resolved 2026-07-11 — moved to the Decision Log below.)*

#### D-007 · Cross-Compile / Release Runner Strategy
**Status:** OPEN
**Blocks:** Phase 4 start (static binary release matrix requires this)
**Owner:** TBD
**Deadline:** Before Phase 4 starts (can be resolved later than D-001–D-006)

**Options:**

| Option | Description | Trade-offs |
|--------|-------------|------------|
| **A: Hybrid (recommended)** | Native runners for macOS (arm64, x86_64) and Windows x86_64; `cross` + Docker for Linux musl/aarch64 | Best balance of reliability and coverage; macOS/Windows signing is difficult to cross-compile |
| B: All-native | One dedicated runner per platform | Simplest, most reliable; expensive in runner infrastructure |
| C: All-cross via `cross` | Docker cross-compilation for all Linux targets; macOS/Windows native | Linux musl works well with `cross`; macOS/Windows code signing requires native runners regardless |

**Recommendation:** Option A (Hybrid). macOS code signing and Windows MSVC toolchains are difficult to cross-compile; Linux musl/aarch64 cross-compiles well with `cross`.

**Decision:** *(fill in when resolved)*
**Rationale:** *(fill in when resolved)*

### Blocking — Phase 2

#### D-013 · Prompt-Cache Prefix-Stability Policy
**Status:** OPEN
**Blocks:** Phase 2 (pipeline/transform design — a correctness property, not a feature add-on)
**Owner:** TBD
**Deadline:** Before transforms are wired into the pipeline (Task 5)

**Context:** Surfaced by July 2026 competitive review. `headroom`'s `CacheAligner` keeps a frozen prefix byte-identical so the provider prompt cache (~90% discount) stays warm. If tokenfold rewrites a cached prefix it invalidates the cache and can increase total cost — the opposite of the product promise.

**Options:**

| Option | Pros | Cons |
|--------|------|------|
| A) Add a prefix-stability contract: never rewrite bytes before a configurable cache boundary (recommended) | Protects prompt-cache economics; matches headroom's key correctness property; cheap to specify | Requires a cache-boundary input (byte offset or turn index) from the caller |
| B) Document cache-invalidation risk; leave prefix handling to the caller | Zero engine work | Cedes headroom's differentiator; risks net cost *increase* for cache-heavy callers |
| C) Full live-zone model (frozen prefix + rolling compression window) | Most capable | Significant complexity; likely v0.2+ |

**Recommendation:** A for v0.1 — add `CompressionPolicy.cache_boundary: Option<CacheBoundary>` (byte offset or turn index); the pipeline treats everything before it as protected content (folds into `protected_floor`). Emit a `PrefixModified` warning if any transform would touch the frozen prefix. Defer the full live-zone model (C) to v0.2.

**Decision:** *(fill in when resolved)*
**Rationale:** *(fill in when resolved)*

### Blocking — Phase 5

#### D-008 · Proxy Timing (v0.1 vs v0.2)
**Status:** OPEN (default: v0.2 unless first consumer requires proxy deployment)
**Blocks:** Phase 5 scope (if v0.1, it moves to Phase 2; if v0.2, it stays in Phase 5)
**Owner:** TBD
**Deadline:** Before Phase 1 ends

**Default decision:** v0.2 (deferred). Override only if the First Consumer worksheet (D-002) names a proxy/request-path deployment as the adoption path.

**Decision:** *(fill in when resolved)*
**Rationale:** *(fill in when resolved)*

#### D-009 · Internal Python Wheel Repository
**Status:** DEFERRED
**Blocks:** Phase 5 (Python binding publishing)
**Deferred until:** Python binding (Phase 5) is formally scoped.

**Required when undeferred:**
- Internal Artifactory PyPI simple index URL
- Publishing credentials and permissions
- `pip` / `uv` install verification process

#### D-011 · MCP Server Touchpoint (compression as MCP tools)
**Status:** OPEN
**Blocks:** Phase 5 scope (net-new surface; not in the original PLAN)
**Owner:** TBD
**Deadline:** Before Phase 5 scoping

**Context:** MCP is becoming the standard agent integration path. `tokenfold mcp serve` should expose `tokenfold_compress`, `tokenfold_inspect`, `tokenfold_retrieve`, and `tokenfold_stats`, with retrieval/stats tools enabled only when local stores are configured.

**Options:**

| Option | Pros | Cons |
|--------|------|------|
| A) Add `tokenfold mcp serve` in v0.2 (recommended) | Matches headroom; standard agent path; reuses core plus local retrieve/stats | Net-new surface; MCP protocol maintenance |
| B) Defer to v0.3+ | Keeps v0.2 lean | Cedes the standard integration path to headroom |
| C) Never (CLI/proxy only) | Simplest | Likely a losing stance as MCP adoption grows |

**Recommendation:** A, but gated on the First Consumer worksheet (D-002) for priority. The command should still be in the v0.2 parity plan because it is now part of the minimum Headroom-competitive agent surface.

**Decision:** *(fill in when resolved)*
**Rationale:** *(fill in when resolved)*

### Blocking — Phase 6

#### D-014 · Full Headroom Parity Extension Boundary
**Status:** OPEN
**Blocks:** Phase 6 scope
**Owner:** TBD
**Deadline:** Before v0.3 planning

**Context:** The July 2026 implementation review found Headroom capabilities beyond the static-binary wedge: framework adapters, vector/RAG routing, output-token shaping, image/multimodal compression, learn/session mining, auth/update/admin commands, and dashboard/live observability. The user direction is to plan support for all Headroom features, but not to make every optional runtime a dependency of the base CLI.

**Options:**

| Option | Pros | Cons |
|--------|------|------|
| A) Optional extension packs (recommended) | Full Headroom parity without burdening the static binary | More packages and release coordination |
| B) Monolithic runtime | Simpler product story | Loses tokenfold's portability differentiator |
| C) Continue ceding ML/vector/adapter features | Keeps scope small | Violates full-parity direction and loses broad-platform deals |

**Recommendation:** A. Keep `tokenfold` as the portable default binary, and support the remaining Headroom feature set through optional packages/sidecars with explicit runtime gates and independent tests.

**Decision:** *(fill in when resolved)*
**Rationale:** *(fill in when resolved)*

### Blocking — Phase 4

#### D-012 · Durable Agent-Wrap / `init` Auto-Integration
**Status:** OPEN
**Blocks:** Phase 4 (distribution) — affects adoption-friction, not the engine
**Owner:** TBD
**Deadline:** Before Phase 4 distribution work

**Context:** Both RTK and headroom install durable per-host hooks automatically. tokenfold's original plan shipped only manual snippets — a real adoption-friction gap.

**Options:**

| Option | Pros | Cons |
|--------|------|------|
| A) `tokenfold init --agent <name>` + `uninit` in v0.1 (recommended) | Removes adoption friction; matches RTK/headroom; reversible | Per-host rewrite-rule maintenance (Claude Code, Copilot, Cursor, Cline, Windsurf each differ) |
| B) Manual snippets only (current plan) | Zero maintenance | Higher adoption friction; loses to one-command rivals |
| C) `init` for Claude Code only in v0.1; expand later | Cheap; covers the likely first consumer | Partial coverage |

**Recommendation:** C for v0.1 (cover the first consumer's host), expand to A in v0.2. Ship `uninit`/`unwrap` from day one — a tool that can't cleanly remove itself is an adoption blocker for cautious teams. Scope the exact host set from D-002.

**Decision:** *(fill in when resolved)*
**Rationale:** *(fill in when resolved)*

### Deferred

#### D-010 · License Model for Open-Source Release
**Status:** DEFERRED
**Blocks:** Nothing in v0.1/v0.2 (internal-only)
**Deferred until:** First internal workload meets target outcome AND the open-source go/no-go decision is made.

**Context:** PLAN.md § Identity Decision: default to internal cost-reduction tool first. Open-source naming, legal review, public registry publishing, and community support are explicitly out of scope until the first internal workload succeeds.

### Decision Log (Resolved)

#### D-001 · Package Name
**Resolved:** 2026-07-11
**Owner:** Project author

**Verification checklist:**
- [x] crates.io name check — clean (`GET /api/v1/crates/tokenfold` → 404, name unclaimed)
- [x] ~~Internal Cargo registry name check~~ N/A — no internal registry (solo/open-source project)
- [x] PyPI namespace check (for future Python binding) — clean (`/pypi/tokenfold/json` → 404, name unclaimed)
- [x] ~~Internal binary artifact naming convention check~~ N/A — releases go to GitHub Releases, not an internal artifact store
- [x] ~~Legal/brand sign-off~~ N/A — no legal/brand team; solo project
- [ ] GitHub repository/org name available — not yet checked (no remote created yet)

**Decision:** `tokenfold`.
**Rationale:** Short, CLI-friendly, matches the crate's purpose, and both public registries it will actually publish to (crates.io, PyPI) are clean. The enterprise-only checklist items (internal registry, legal/brand, internal artifact naming) don't apply once this is scoped as a solo/open-source project.

#### D-002 · First Consumer
**Resolved:** 2026-07-11
**Owner:** Project author

**Decision:** No external/organizational first consumer. Worksheet filled with tokenfold dogfooding its own dev workflow — see plan.md § First Consumer & Success Definition for the full table (workload, payload types, target outcome, fixture source, adoption path).
**Rationale:** This is a solo project with no separate internal team or workload to validate against yet. Using the author's own dev workflow (git diffs, build/test logs, agent tool-call JSON) as the first consumer still gives Phase 1+ a concrete, narrow payload scope (matches D-004's "Narrow" recommendation) without inventing a fictitious external stakeholder.

#### D-003 · Internal Cargo Registry URL
**Resolved:** 2026-07-11
**Owner:** Project author

**Decision:** N/A — no internal Cargo registry. Dependency resolution and publishing both use public crates.io directly.
**Rationale:** Solo/open-source project; there is no internal Artifactory/infra team to provision a mirror for. `.cargo/config.toml` needs no sparse-index override — default crates.io works for both local dev and CI.

#### D-006 · CI/CD System
**Resolved:** 2026-07-11
**Owner:** Project author

**Decision:** GitHub Actions, GitHub-hosted (cloud) runners.
**Rationale:** Free for a public repo, native to GitHub (where the repo lives), and covers the release matrix directly: `ubuntu-latest` (Linux x86_64), `macos-latest`/`macos-13` (macOS arm64/x86_64), `windows-latest` (Windows x86_64). No self-hosted runner infra to maintain. Rust toolchain caching via `actions/cache` (or `Swatinem/rust-cache`) over the Cargo registry/target dirs; no Artifactory credential injection needed since there's no internal registry (D-003).

### Dependency Map

```
D-001 (name) ----------------------------------- RESOLVED (Phase 0 exit)
D-002 (first consumer) ------------------------- RESOLVED (Phase 0 exit)
D-003 (cargo registry) ------------------------- RESOLVED (Phase 0 exit)
D-006 (CI system) ------------------------------ RESOLVED (Phase 0 exit)
D-004 (payload formats) ------------------------ blocks Phase 2 start
D-005 (fidelity thresholds) -------------------- blocks Phase 2 start
D-007 (cross-compile) -------------------------- blocks Phase 4 start
D-008 (proxy timing) --------------------------- blocks Phase 5 scope
D-009 (python wheel repo) ---------------------- blocks Phase 5 python
D-013 (prompt-cache prefix stability) ---------- blocks Phase 2 start
D-011 (MCP server surface) --------------------- blocks Phase 5 scope
D-012 (init/wrap auto-integration) ------------- blocks Phase 4 distribution
D-014 (full Headroom parity boundary) ---------- blocks Phase 6 scope
D-010 (license) -------------------------------- deferred
```

