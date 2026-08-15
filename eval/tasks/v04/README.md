# v0.4-alpha paired-task baseline corpus

Held-out **evaluation** fixtures for `eval/run_baselines.py`. They measure deterministic keep/drop
selectors against downstream tasks at exact token ceilings: no model is involved, nothing is
trained here, and nothing reaches a served path.

## Coverage (77 fixtures across 11 families)

Spanning the required content slices: `log_qa`, `log_multi_service`
(logs/tool QA), `diff_review`, `code_patch` (diff review / change localization), `code_build_error`
(build/test failures), `json_schema`, `tool_call_json` (JSON/schema + tool calls),
`long_context_needle` (long mixed context with an id/hash/path needle), `ccr_marker` (CCR
reconstruction), and `rust_holdout` + `typescript_holdout` (the project-disjoint Rust/TS hard
slices). Each family has 7 fixtures (`_001`, `_010`-`_015`). Every fixture is gate-validated and
confirmed to *differentiate* selectors (at least one selector fails the task at the 25% ceiling,
so the report is discriminating rather than trivially 1.0 everywhere).

## Fixture schema

```json
{
  "id": "log_qa_001",
  "family": "log_qa | diff_review | json_schema | ...",
  "tier": "A",
  "source": "the raw captured text to compress",
  "query": "the downstream question the compressed context must still answer",
  "gold_answer": "substring that must survive for the task to be answerable",
  "critical_atoms": ["ids/hashes/paths that must survive regardless of the selector"],
  "notes": "optional: why this fixture discriminates selectors (which ones fail and why)"
}
```

- **`critical_atoms`** are force-kept by deterministic logic (units containing them are never
  dropped), so 100% critical-atom survival is a structural guarantee — a hard gate that
  `--gate` asserts — not something a selector must learn. Put audit/CCR-critical ids, hashes,
  and paths here.
- **`gold_answer`** should live in a unit that is *not* a critical atom, so whether the task is
  answerable genuinely depends on the selector + token budget. That is what differentiates the
  baselines (and, later, a learned selector) instead of every policy trivially scoring 1.0.
- **`notes`** (optional, added from `_010` onward) documents the discrimination design intent per
  fixture — which selectors are expected to fail the task at tight ceilings and why. The harness
  ignores it; it exists for humans auditing/extending the corpus.
- **`tier`** is `"A"` on every fixture and records the provenance class described below. It exists
  so a fixture from anywhere else would be visibly distinguishable; nothing else is accepted here.

## Provenance

Every fixture here is **project-owned synthetic material** — hand-authored traces and
fault-injected build output, written for this corpus. Nothing is captured from a user, a customer,
or a third-party repository, which is why the corpus is safe to track and safe to run in CI by
default.

That property is a precondition for adding anything, not an observation about what happens to be
here. A new fixture must be synthetic and project-owned; redact and secret-scan before committing,
and reject rather than store anything secret-shaped. Captured or third-party material does not
belong in this directory regardless of licence.

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

- Remaining baselines: RTK and RTK+tokenfold (external tool), plus an achieved-token sweep of a
  third-party content-aware compressor. (`deterministic-tokenfold` and `llmlingua_style` are now
  implemented — see above.)
- Broader corpora drawn from permissively-licensed public repositories, with licence and revision
  manifests recorded per file, plus near-duplicate detection against the existing fixtures.
- Structural segmentation (diff hunks, JSON containers, AST/code blocks) — v0.4-alpha segments by
  line.
- Real paired build/test/debug/patch execution and an LLM judge for *diagnosing* failures (never
  for satisfying a gate). The current scorer is a deterministic containment proxy.
