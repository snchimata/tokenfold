# v0.4-alpha paired-task baseline corpus

Fixtures for `eval/run_baselines.py` — the "baselines first" stage of the v0.4 learned-selector
research (`docs/solution-design/model-research.md`). Everything here is **shadow-only**: it
measures deterministic keep/drop selectors against downstream tasks; no model is involved and
nothing reaches a served path.

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

## Deferred to later v0.4-alpha work (not hidden)

- Additional baselines as `SELECTORS` entries: RTK, RTK+tokenfold, deterministic-tokenfold (Rust
  CLI subprocess), LLMLingua-style, and the unmodified Headroom Kompress-v2 achieved-token sweep.
- Tier-B public-repo corpora with license/revision manifests; project-disjoint train/test splits
  and near-dedup across splits.
- Structural segmentation (diff hunks, JSON containers, AST/code blocks) — v0.4-alpha segments by
  line.
- Real paired build/test/debug/patch execution and an LLM judge for *diagnosing* failures (never
  for satisfying a gate). The current scorer is a deterministic containment proxy.
