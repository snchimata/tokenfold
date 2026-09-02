# v0.4-alpha paired-task baseline corpus

Held-out **evaluation** fixtures for `eval/run_baselines.py`. They measure deterministic keep/drop
selectors against downstream tasks at exact token ceilings: no model is involved, nothing is
trained here, and nothing reaches a served path.

## Coverage (79 fixtures across 13 families)

Spanning the required content slices: `log_qa`, `log_multi_service`
(logs/tool QA), `diff_review`, `code_patch` (diff review / change localization), `code_build_error`
(build/test failures), `json_schema`, `tool_call_json` (JSON/schema + tool calls),
`long_context_needle` (long mixed context with an id/hash/path needle), `ccr_marker` (CCR
reconstruction), and `rust_holdout` + `typescript_holdout` (the project-disjoint Rust/TS hard
slices), plus one `lossy_mad_zero` and one `lossy_mid_array_plant` regression fixture for the real
lossy compressor. Each original family has 7 fixtures (`_001`, `_010`-`_015`). Every paired
fixture is gate-validated and confirmed to *differentiate* selectors (at least one selector fails
the task at the 25% ceiling,
so the report is discriminating rather than trivially 1.0 everywhere). `--gate` asserts this at
the tightest requested ceiling; adding a non-discriminating fixture fails the gate.

## Fixture schema

```json
{
  "id": "log_qa_001",
  "family": "log_qa | diff_review | json_schema | ...",
  "tier": "A",
  "evaluation_kind": "paired | compressor (optional; defaults to paired)",
  "source": "the raw captured text to compress",
  "query": "the downstream question the compressed context must still answer",
  "gold_answer": "substring that must survive for the task to be answerable",
  "supporting_evidence": "optional exact source span grounding the answer",
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
- **`supporting_evidence`** optionally names the complete source span needed to ground the answer.
  When omitted, the scorer uses the unique source line containing `gold_answer`. Task success now
  requires that evidence—not merely a detached answer token—to survive, and reports the result as
  deterministic `claim_faithfulness`. This catches gross loss of subject/predicate context; it is
  not a semantic or LLM judgment.
- **`notes`** (optional, added from `_010` onward) documents the discrimination design intent per
  fixture — which selectors are expected to fail the task at tight ceilings and why. The harness
  ignores it; it exists for humans auditing/extending the corpus.
- **`tier`** is `"A"` on every fixture and records the provenance class described below. It exists
  so a fixture from anywhere else would be visibly distinguishable; nothing else is accepted here.
- **`evaluation_kind`** is omitted/`"paired"` for selector-and-compressor tasks. The two structural
  lossy regression fixtures use `"compressor"`, because line-level critical-atom forcing keeps
  their entire anomalous JSON record and makes every selector pass vacuously; they are excluded
  from selector metrics rather than inflating them.

`load_fixtures` validates this contract before scoring: required fields and critical atoms must be
non-empty, every answer/atom must be grounded in `source`, fixture IDs and atoms must be unique,
and the whitespace-normalized `gold_answer` must occur exactly once. Invalid fixtures fail closed
instead of turning a missing answer or safety atom into a vacuous pass.

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
python eval/audit_quality_sample.py       # regenerate the deterministic human-audit checklist
python eval/audit_quality_sample.py --check
```

`HUMAN_AUDIT.md` deliberately remains failing/pending until a named human reviews every selected
fixture. Automation may select and hash the sample, but must not impersonate that reviewer.

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
