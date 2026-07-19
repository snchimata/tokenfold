# v0.4-alpha paired-task baseline corpus

Fixtures for `eval/run_baselines.py` — the "baselines first" stage of the v0.4 learned-selector
research (`docs/solution-design/model-research.md`). Everything here is **shadow-only**: it
measures deterministic keep/drop selectors against downstream tasks; no model is involved and
nothing reaches a served path.

## Coverage (11 Tier-A fixtures)

Spanning the required slices from model-research.md §Prerequisites: `log_qa`, `log_multi_service`
(logs/tool QA), `diff_review`, `code_patch` (diff review / change localization), `code_build_error`
(build/test failures), `json_schema`, `tool_call_json` (JSON/schema + tool calls),
`long_context_needle` (long mixed context with an id/hash/path needle), `ccr_marker` (CCR
reconstruction), and `rust_holdout` + `typescript_holdout` (the project-disjoint Rust/TS hard
slices). Every fixture is gate-validated and confirmed to *differentiate* selectors (at least one
selector fails the task at the 25% ceiling, so the report is discriminating rather than trivially
1.0 everywhere).

## Fixture schema

```json
{
  "id": "log_qa_001",
  "family": "log_qa | diff_review | json_schema | ...",
  "tier": "A | B | C",
  "source": "the raw captured text to compress",
  "query": "the downstream question the compressed context must still answer",
  "gold_answer": "substring that must survive for the task to be answerable",
  "critical_atoms": ["ids/hashes/paths that must survive regardless of the selector"]
}
```

- **`critical_atoms`** are force-kept by deterministic logic (units containing them are never
  dropped), so 100% critical-atom survival is a structural guarantee — a hard gate in
  model-research.md — not something a selector must learn. Put audit/CCR-critical ids, hashes,
  and paths here.
- **`gold_answer`** should live in a unit that is *not* a critical atom, so whether the task is
  answerable genuinely depends on the selector + token budget. That is what differentiates the
  baselines (and, later, a learned selector) instead of every policy trivially scoring 1.0.

## Governance tiers (model-research.md §Prerequisites and Data)

| Tier | Source | v0.4 use |
| --- | --- | --- |
| A | Project-owned synthetic traces / fault-injected builds | Training + evaluation |
| B | MIT/Apache/BSD/CC0 public repos with file/revision manifests | Training + project-disjoint eval |
| C | Explicitly opted-in, locally redacted user traces | Local shadow eval only; never centralized |

Current corpus is **Tier A only** (synthetic, safe as the default CI corpus). Redact and
secret-scan before persisting anything; reject rather than store secret-shaped content.

## Running

```bash
python eval/run_baselines.py            # human summary curve
python eval/run_baselines.py --json     # full JSON report (summary + per-row detail)
python eval/run_baselines.py --gate     # assert invariants; non-zero exit on failure
python eval/run_baselines.py --ratios 0.75,0.5,0.25
```

Install `tiktoken` (`pip install -e 'eval[exact]'`) for exact `o200k_base` ceilings; without it
the harness falls back to the same byte/4 heuristic as `tokenfold-core` and labels the report
`"backend": "heuristic"`.

## Baseline kinds: selectors vs. compressors

- **Selectors** (`keep_all`, `forced_only`, `recency`, `frequency`, `bm25`, `llmlingua_style`)
  rank atomic units. `llmlingua_style` is a perplexity-free proxy — it ranks units by mean
  per-token self-information (surprisal) under a document-derived unigram model, a deterministic
  stand-in for LLMLingua's small-LM perplexity. The harness force-keeps critical-atom units and
  enforces the exact token ceiling on them, so 100% critical-atom survival and the ceiling are
  guarantees.
- **Compressors** (`deterministic-tokenfold`) run a whole-pipeline best-effort compressor over
  the source — the harness does *not* force atoms through them, so their critical-atom survival
  and achieved ratio are **measured, not asserted**. `deterministic-tokenfold` shells out to the
  real Rust CLI, discovered via `TOKENFOLD_BIN`, then a local `target/{release,debug}` build,
  then `PATH`; when it isn't found the baseline is cleanly skipped (`n/a`) and the report/gate
  say so, so this harness still runs in a build-less CI. It is the primary **baseline to beat**:
  it is lossless/evidence-safe (task + critical survival ≈ 1.0) but often cannot reach aggressive
  budgets on low-repetition inputs — the exact gap a learned selector must close.

## Deferred to later v0.4-alpha work (not hidden)

- Remaining baselines: RTK and RTK+tokenfold (external tool) and the unmodified Headroom
  Kompress-v2 achieved-token sweep (needs the ML checkpoint). (`deterministic-tokenfold` and
  `llmlingua_style` are now implemented — see above.)
- Tier-B public-repo corpora with license/revision manifests; project-disjoint train/test splits
  and near-dedup across splits.
- Structural segmentation (diff hunks, JSON containers, AST/code blocks) — v0.4-alpha segments by
  line.
- Real paired build/test/debug/patch execution and an LLM judge for *diagnosing* failures (never
  for satisfying a gate). The current scorer is a deterministic containment proxy.
