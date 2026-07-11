# tokenfold — Integration Touchpoints & Competitive Coverage

Answers two questions: (1) where does tokenfold plug into a workflow, and (2) does it cover what `rtk` and `headroom` do — with a complete ability-by-ability coverage matrix.

**Verified competitors (July 2026):**
- **`rtk-ai/rtk`** — Rust single binary; command-output compression, durable hooks, TOML filter packs, SQLite analytics. Verified at `Cargo.toml` v0.42.4. [github.com/rtk-ai/rtk](https://github.com/rtk-ai/rtk)
- **`headroomlabs-ai/headroom`** — Python CLI/proxy/library + TypeScript SDK + Rust/PyO3 core. Multi-surface: proxy, MCP, agent wrap/init, reversible retrieval, dashboard/stats, output shaping, adapters, optional ML/vector/image. [github.com/headroomlabs-ai/headroom](https://github.com/headroomlabs-ai/headroom)

tokenfold plans support for every Headroom capability; heavy adapter, ML, vector, image, and dashboard functionality ships as optional extensions to preserve the portable-binary advantage.

## Part 1 — The Nine Touchpoint Zones

```
                        ┌─────────────────────────────────────────────┐
                        │                   MODEL                     │
                        └─────────────────────────────────────────────┘
                                          ▲
             ┌────────────────────────────┼───────────────────────────┐
             │                            │                           │
    Zone 4: PROXY             Zone 5: LIBRARY/BINDING       Zone 6: MCP SERVER
    (request path)            (in-process call)             (agent tool call)
             ▲                            ▲                           ▲
             │                            │                           │
    ┌────────┴──────────┬─────────────────┴──────────┬────────────────┴──────┐
    │                   │                            │                       │
 Zone 1: PIPE      Zone 2: SHELL-WRAP         Zone 3: EDITOR/AGENT HOOK   Zone 7: CI/BATCH
 (stdin→stdout)    (tokenfold wrap <cmd>)      (PreToolUse auto-rewrite)   (offline job)

    Auxiliary surfaces (local, off the model round-trip path):

      Zone 8: LOCAL RETRIEVAL / STATS        Zone 9: OPTIONAL EXTENSIONS
      (tokenfold retrieve / stats / ledger)   (adapters, RAG/vector, output, image, …)
```

| # | Zone | tokenfold surface | Feature | Version | vs rtk | vs headroom |
|---|------|------------------|---------|---------|--------|-------------|
| 1 | Pipe (stdin→stdout) | CLI `compress -` | F-021 | v0.1 | ✅ meets | ✅ meets |
| 2 | Shell-wrap | CLI `wrap <cmd>` (alias `shell`) | F-022 | v0.1 | ⚠️ partial | ✅ meets |
| 3 | Editor/agent hook | `init`/`uninit` | F-043 | v0.1 (Phase 4) | ⚠️ closing (was ❌) | ⚠️ closing (was ❌) |
| 4 | Proxy (request path) | `tokenfold-proxy` | F-040 | v0.2 | n/a | ⚠️ partial |
| 5 | Library / binding | core crate; `tokenfold-py` | F-001/F-041 | v0.1 (Rust)/v0.2 (Py) | n/a | ⚠️ partial |
| 6 | MCP server | `tokenfold mcp serve` | F-042/F-045/F-046 | v0.2 | n/a | ✅ planned parity |
| 7 | CI / batch | CLI in a CI job | F-021/F-024 | v0.1 | ✅ meets | ✅ meets |
| 8 | Local retrieval / stats | `retrieve`, `stats`, ledger | F-045/F-046 | v0.2 | ⚠️ closes analytics | ✅ planned parity |
| 9 | Optional extensions | adapters, RAG/vector, output, image, learn, update/auth | F-048–F-053 | v0.3+ | n/a | ✅ planned parity |

Legend: ✅ meets · ⚠️ partial/closing · ❌ gap

### Zone 1 — Pipe (stdin → stdout)
`producer | tokenfold compress - --format <fmt> | consumer`. Differentiator: tokenfold emits a typed, versioned `CompressionReport` neither rival offers as a stable machine contract.

### Zone 2 — Shell-wrap (`tokenfold wrap <cmd>`, alias `shell`)
Run a command, compress its captured output. **⚠️ Partial by design:** tokenfold ships `log_compaction` (F-012) applied to the `command_output` input format (adjacent dedup + redaction), NOT 100+ command handlers.

### Zone 3 — Editor / Agent Hook (PreToolUse auto-rewrite)
Highest-adoption, lowest-friction zone. **Now closing via F-043 (D-012):** `tokenfold init`/`uninit`/`doctor`, first-consumer host in v0.1, full host set in v0.2.

### Zone 4 — Proxy (request path)
`tokenfold-proxy` (F-040, v0.2): axum/hyper, streams bodies, buffers only non-streaming JSON, recomputes `Content-Length`, rejects CL+TE, and supports `X-TokenFold-*` controls. **Parity path:** input compression and safety controls ship in v0.2; output-token shaping and holdout measurement are covered by F-050 as an optional v0.3 policy layer.

### Zone 5 — Library / Binding
Rust `tokenfold_core::compress` (v0.1) anchors the API; Python `tokenfold-py` ships in v0.2. **Parity path:** framework adapter breadth (OpenAI/Anthropic/Vercel AI SDK/LiteLLM/LangChain/Agno/ASGI/Strands) is planned as optional extension pack F-048, with Node/TS/WASM bindings added when a first consumer requires them.

### Zone 6 — MCP Server
**Now closing via F-042/F-045/F-046 (D-011):** `tokenfold mcp serve` exposes `tokenfold_compress`, `tokenfold_inspect`, `tokenfold_retrieve`, and `tokenfold_stats` when local stores are enabled. Retrieval is local/content-addressed by default, not a vector-memory platform.

### Zone 7 — CI / Batch
Same CLI in a CI job; env-var config (`TOKENCUT_*`), stable scriptable exit codes. **✅ At parity**, and *more* scriptable than rtk (stable exit codes + `--json`); savings analytics are covered by `tokenfold stats <report-glob>`.

### Zone 8 — Local Retrieval / Stats
Headroom's CCR and stats/dashboard are now covered by the v0.2 portable parity layer: `tokenfold retrieve` (F-045) and `tokenfold stats` (F-046). The design is deliberately local/report-first: no raw payload persistence by default, no required vector DB, and loopback-only dashboard export.

### Zone 9 — Optional Extensions
Headroom's heavy surfaces are supported by v0.3+ optional extensions: framework adapters (F-048), RAG/vector retrieval (F-049), output shaping and holdout measurement (F-050), image/multimodal compression (F-051), learn/session mining (F-052), and auth/update/admin commands (F-053). These are parity commitments, but they must not add runtime dependencies to the default CLI/hook/proxy path.

## Part 2 — Complete Headroom Ability Coverage

Every headroom capability found in the July 2026 review, and how tokenfold addresses it. **Status key:** ✅ Covered/planned in default path · 🟦 Decided/tracked · 🍀 Optional extension · ⚠️ Partial/narrower portable equivalent.

### Integration surfaces

| Headroom ability | tokenfold response | Status | Ref |
|------------------|-------------------|--------|-----|
| `wrap <agent>` / `init`, durable | `tokenfold init --agent` | 🟦 Decided | F-043 / D-012 |
| `unwrap <tool>` | `tokenfold uninit --agent` | 🟦 Decided | F-043 / D-012 |
| `proxy --port` | `tokenfold-proxy` | ✅ Covered (v0.2) | F-040 |
| `mcp serve` / `mcp install` | `tokenfold mcp serve`/`install` | ✅ Covered (v0.2) | F-042 / D-011 |
| Library — Python `compress()` | `tokenfold-py` wheel | ✅ Covered (v0.2) | F-041 |
| Library — TypeScript `compress()` | Node/TS binding package | 🍀 Optional extension | F-048 |
| Framework adapters (Anthropic, OpenAI, Vercel AI SDK, Agno, ASGI, LiteLLM, LangChain, Strands) | Optional adapter pack | 🍀 Optional extension | F-048 |
| Multi-agent `SharedContext` (put/get) | Local report/retrieve store first; shared backend only in optional extension | 🍀 Optional extension | F-045/F-049 |

### Ops / observability commands

| Headroom ability | tokenfold response | Status | Ref |
|------------------|-------------------|--------|-----|
| `doctor` (health/routing check) | `tokenfold doctor` (verify hook + estimator backends) | 🟦 Decided | F-043 |
| `perf` (perf metrics) | `benchmark` (build-time) + proxy stats | ⚠️ Partial | F-024 |
| `dashboard` (live savings viz) | `tokenfold stats --serve` loopback dashboard export | ✅ Covered (v0.2) | F-046 |
| `learn` (mine failed sessions, auto-tune terseness) | `tokenfold learn` recommendations, explicit approval required | 🍀 Optional extension | F-052 |
| `output-savings` (estimate output-token fold) | Output-shaping profile + holdout measurement | 🍀 Optional extension | F-050 |
| `update` (in-place self-upgrade) | Signed internal update/rollback command | 🍀 Optional extension | F-053 |
| `copilot-auth login` / auth/admin | Auth doctor + proxy/admin bearer support; no secret persistence | 🍀 Optional extension | F-053 |

### Engine / content-type capabilities

| Headroom ability | tokenfold response | Status | Ref |
|------------------|-------------------|--------|-----|
| JSON (SmartCrusher) | `json_minify` + `schema_compaction` | ✅ Covered (v0.1) | F-010/F-011 |
| Source code, AST-aware (CodeCompressor) | `code_digest` (tree-sitter, feature-gated) | ✅ Covered (v0.2) | F-015 |
| Prose, ML model (Kompress-v2-base) | `prose_extraction` (BM25/TF-IDF) | ⚠️ Partial — heuristic, NOT a trained model (deliberate: no ML runtime) | F-018 |
| Images (ML router) | Optional image/multimodal extension | 🍀 Optional extension | F-051 |
| Tool outputs (wraps RTK) | `wrap` + `log_compaction` (generic) | ✅ Covered (not 100+ handlers) | F-022/F-012 |
| RAG chunks (content-routed selection) | `prose_extraction` default + optional RAG/vector extension | 🍀 Optional extension | F-018/F-049 |
| Logs (SmartCrusher/Kompress) | `log_compaction` | ✅ Covered (v0.1) | F-012 |
| Conversation history (live-zone) | `conversation` | ✅ Covered (v0.2) | F-017 |
| ContentRouter (auto type detection) | `InputFormat::Auto` heuristic | ✅ Covered (basic) | F-001 |

### Fidelity / quality / caching

| Headroom ability | tokenfold response | Status | Ref |
|------------------|-------------------|--------|-----|
| KV-cache prefix stability (CacheAligner, frozen prefix byte-identical) | Prompt-cache prefix preservation (`cache_boundary`) | 🟦 Decided (v0.1) | F-044 / D-013 |
| CCR reversible cache + retrieval | Local content-addressed evidence store + `retrieve`/MCP retrieve | ✅ Covered (v0.2) | F-045 |
| Lossless / lossy / hybrid classification | Transform taxonomy (lossless / semantics-preserving / lossy-w-evidence) | ✅ Covered | PLAN.md § Transform Types |
| Published downstream evals (GSM8K/TruthfulQA/SQuAD/BFCL) | Fidelity harness + contrastive KPI + accuracy@ratio curves | ✅ Covered (more rigorous) | F-016 |
| Holdout measurement group (live control) | Fidelity-audit sampling (v0.2 design) | ✅ Covered (design) | PLAN.md § Fidelity / Output-Quality Evaluation |
| Embedder runtime / vector HNSW (RAG relevance) | Optional RAG/vector extension; never required by default binary | 🍀 Optional extension | F-049 |

### Distribution & runtime

| Headroom ability | tokenfold response | Status | Ref |
|------------------|-------------------|--------|-----|
| pip / uv / npm / Docker | Static binary + `cargo install` + abi3 wheel (v0.2) | ✅ Covered — *and the differentiator* (single binary, zero ML runtime) | F-030 |
| Heavy ML runtime (ONNX, PyTorch-MPS, HF models, C++ HNSW) | Optional extension runtime only; default binary remains dependency-light | 🍀 Optional extension | F-049/F-051 |

**Coverage tally:** every headroom ability from the July 2026 review is now either **covered/planned in the default path**, **decided/tracked**, **partially covered by a narrower portable equivalent**, or **covered by an optional extension**. There are no ceded Headroom capabilities in the forward plan.

## Part 3 — Content-Type Coverage (cross-cutting)

| Content type | tokenfold | rtk | headroom |
|--------------|----------|-----|----------|
| Command output / logs | ✅ v0.1 | ✅ (100+ handlers) | ✅ (wraps rtk) |
| Git diffs | ✅ v0.1 | ✅ | ✅ |
| JSON payloads | ✅ v0.1 (`json_minify`) | ❌ | ✅ (SmartCrusher) |
| Tool/function schemas | ✅ v0.1 (`schema_compaction`) | ❌ | ✅ |
| Source code (AST) | ⚠️ v0.2 (`code_digest`) | ✅ | ✅ |
| Prose / text | ⚠️ v0.2 (`prose_extraction`, heuristic) | ❌ | ✅ (ML model) |
| Conversation history | ⚠️ v0.2 (`conversation`) | ❌ | ✅ (live-zone) |
| RAG chunks | 🍀 v0.3 optional RAG/vector extension; plain retrieval text can use `prose_extraction` | ❌ | ✅ |
| Images | 🍀 v0.3+ optional multimodal extension | ❌ | ✅ |

## Part 4 — Verdict

**vs `rtk`:** the tokenfold CLI **matches** rtk on the pipe and command-output zones and **exceeds** it on JSON/schema compression and on the quality proof rtk entirely lacks. It **trailed** on one thing — durable per-host hook auto-install — now closing via F-043. rtk is a narrower tool; tokenfold's CLI is a superset except install ergonomics.

**vs `headroom`:** headroom remains the breadth benchmark. Tokenfold now plans full feature coverage: v0.2 closes MCP/retrieve/stats/init/proxy gaps in the portable path, and v0.3+ covers adapters, RAG/vector, output shaping, image/multimodal, learn/session mining, and auth/update/admin as optional extensions. Tokenfold's differentiator is not absence of those features; it is that the default path stays a static Rust binary with exact accounting, typed safety reports, reversible local evidence, and downstream fidelity gates.

**Open items tracked:**

| Item | Kind | Tracked in |
|------|------|------------|
| Durable `init`/`wrap` + `doctor` | Decided — build | F-043 / D-012 |
| MCP server surface | Decided — build | F-042 / D-011 |
| Prompt-cache prefix preservation | Decided — build (v0.1) | F-044 / D-013 |
| Reversible retrieval + stats | Decided — build | F-045 / F-046 |
| Declarative command filters | Decided — build | F-047 |
| Framework adapters | Optional extension | F-048 / D-014 |
| RAG/vector routing/cache | Optional extension | F-049 / D-014 |
| Output shaping / holdout measurement | Optional extension | F-050 / D-014 |
| Images / multimodal | Optional extension | F-051 / D-014 |
| Learn/session mining | Optional extension | F-052 / D-014 |
| Auth/update/admin | Optional extension | F-053 / D-014 |

## Part 5 — Implementation Patterns Adopted

The July 2026 source review changed the plan from a pure feature checklist into an implementation strategy. These are the patterns tokenfold should copy or adapt:

| Pattern | Observed in | Tokenfold adaptation |
|---------|-------------|---------------------|
| Thin host hooks delegate to a native binary; backups and idempotent uninstall are product features, not polish | RTK, Headroom, Squeez-style hook tools | F-043 requires managed blocks, byte-restorable backups, `doctor`, and first-host v0.1 support |
| Command-output breadth scales through declarative filters before hundreds of bespoke handlers | RTK | F-047 adds TOML filter packs, inline fixture verification, precedence rules, and a `never_worse` guard |
| Local savings analytics make adoption visible | RTK `gain`/`discover`/`session`, Headroom stats/dashboard | F-046 adds report-first `stats`, optional redacted ledger, JSON/CSV export, and loopback dashboard export |
| Reversible retrieval should be content-addressed, TTL-bound, and separate from semantic/vector memory | Headroom CCR/MCP retrieve | F-045 adds a local evidence store and `retrieve`/MCP retrieve without requiring a vector DB |
| Prompt-cache safety needs frozen-prefix/live-zone rules, not only token-count reduction | Headroom CacheAligner, Kompact cache-alignment patterns | F-044 protects cache boundaries and reports `CacheReport` proof fields |
| AST/code compression needs explicit modes and language scopes | Skim, Headroom code paths | F-015 remains feature-gated and should grow mode names (`structure`, `signatures`, `types`, `pseudo`) only with downstream fixtures |
| Quality claims require downstream task evaluation, not token deltas alone | Headroom, Kompact, LLMLingua, ACON | F-016 gates lossy defaults on paired original/compressed runs and contrastive failure rates |
| Broad provider/framework integration becomes an adapter treadmill if it lands in core | Headroom, LiteLLM | F-048 keeps adapters optional and first-consumer-driven |
| ML/vector/image capabilities are useful but should not become baseline install cost | Headroom, LLMLingua, multimodal/OCR stacks | F-049 and F-051 ship as optional extensions |

**Anti-patterns to avoid:** heuristic token counts as budget truth, raw secret persistence in analytics, broad hook rewrites before redaction/report contracts stabilize, JSON key reordering that breaks prompt caches, and adapter/runtime dependencies in the base CLI.

## Sources
- [github.com/rtk-ai/rtk](https://github.com/rtk-ai/rtk)
- [github.com/headroomlabs-ai/headroom](https://github.com/headroomlabs-ai/headroom)
- Verified via the July 2026 competitive review; see `PLAN.md` § Research Summary.

