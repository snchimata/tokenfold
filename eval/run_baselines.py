#!/usr/bin/env python3
"""v0.4-alpha learned-selector *baseline* harness (F-057, shadow-only).

This is the "baselines first" stage of the v0.4 research plan in
`docs/solution-design/model-research.md`: before any model is trained, it runs a set of
deterministic keep/drop selectors over paired downstream tasks at *exact provider-token
ceilings* and reports an achieved-token / task-score curve. There is no ML here and nothing
is written to any served path — a learned selector will later plug in as just another entry
in `SELECTORS` and be measured against these same baselines.

Design mirrors `run_fidelity.py`: standard-library only, JSON fixtures under `eval/tasks/`.
`tiktoken` is an *optional* import — when present, token ceilings are exact (`o200k_base`);
otherwise a byte/4 heuristic (identical to `tokenfold-core`'s fallback) is used and the report
labels itself accordingly. Nothing here touches the shipped Rust/npm/Python runtime.

Deliberately deferred (documented, not hidden — see `eval/tasks/v04/README.md`):
  - RTK, RTK+tokenfold, deterministic-tokenfold (Rust CLI subprocess), LLMLingua-style, and the
    unmodified Headroom Kompress-v2 achieved-token sweep are additional `SELECTORS` entries.
  - Real Tier-B public-repo corpora and project-disjoint train/test splits.
  - Structural (diff-hunk / JSON-container / AST) segmentation; v0.4-alpha segments by line.
  - An LLM judge for task success (the current scorer is a deterministic containment proxy).
"""

from __future__ import annotations

import argparse
import json
import math
import re
import sys
from pathlib import Path

# --- token counting: exact via tiktoken when available, else the core's byte/4 heuristic ------

try:  # pragma: no cover - depends on the local environment
    import tiktoken

    _ENC = tiktoken.get_encoding("o200k_base")

    def count_tokens(text: str) -> int:
        return len(_ENC.encode(text)) if text else 0

    TOKENIZER = {"backend": "tiktoken", "model": "o200k_base", "is_exact": True}
except Exception:  # pragma: no cover - exercised only without tiktoken installed

    def count_tokens(text: str) -> int:
        # Mirrors tokenfold-core's ByteHeuristicEstimator: ceil(bytes / 4), 0 for empty.
        if not text:
            return 0
        return -(-len(text.encode("utf-8")) // 4)

    TOKENIZER = {"backend": "heuristic", "model": None, "is_exact": False}


# --- segmentation + deterministic forcing -----------------------------------------------------


def segment(source: str) -> list[str]:
    """Atomic source units. v0.4-alpha uses line units (newline kept, so kept units reassemble
    byte-for-byte). Structural/diff/JSON/AST segmentation is deferred."""
    return source.splitlines(keepends=True)


def forced_indices(units: list[str], critical_atoms: list[str]) -> set[int]:
    """Units containing any declared critical atom are force-kept, regardless of the selector.

    This is the deterministic layer that makes 100% critical-atom survival a *structural*
    guarantee (a hard gate in model-research.md) rather than something a learned model must get
    right. A learned selector only ever ranks the *remaining* units."""
    return {
        i
        for i, unit in enumerate(units)
        if any(atom and atom in unit for atom in critical_atoms)
    }


_WORD = re.compile(r"[A-Za-z0-9_]+")


def _tokens(text: str) -> list[str]:
    return [w.lower() for w in _WORD.findall(text)]


# --- deterministic baseline selectors ---------------------------------------------------------
# Each returns a salience score per unit index (higher = keep sooner). The allocator forces
# critical units first, then fills the remaining token budget by descending score.


def sel_keep_all(units: list[str], query: str) -> list[float]:
    return [math.inf] * len(units)


def sel_forced_only(units: list[str], query: str) -> list[float]:
    return [-math.inf] * len(units)


def sel_recency(units: list[str], query: str) -> list[float]:
    # Prefer later units (classic for logs/streams).
    return [float(i) for i in range(len(units))]


def sel_frequency(units: list[str], query: str) -> list[float]:
    """Query-independent: prefer units carrying *rare* tokens, drop repetitive boilerplate.
    A unit's score is the sum over its distinct tokens of 1/document-frequency."""
    df: dict[str, int] = {}
    for unit in units:
        for tok in set(_tokens(unit)):
            df[tok] = df.get(tok, 0) + 1
    scores = []
    for unit in units:
        distinct = set(_tokens(unit))
        scores.append(sum(1.0 / df[t] for t in distinct) if distinct else 0.0)
    return scores


def sel_bm25(units: list[str], query: str) -> list[float]:
    """Okapi BM25 relevance of each unit to the task query (k1=1.5, b=0.75). Query-dependent."""
    k1, b = 1.5, 0.75
    docs = [_tokens(u) for u in units]
    q = set(_tokens(query))
    if not q or not units:
        return [0.0] * len(units)
    n = len(docs)
    avgdl = sum(len(d) for d in docs) / n if n else 0.0
    df: dict[str, int] = {}
    for d in docs:
        for t in set(d):
            if t in q:
                df[t] = df.get(t, 0) + 1
    scores = []
    for d in docs:
        dl = len(d)
        s = 0.0
        for t in q:
            if t not in df:
                continue
            tf = d.count(t)
            if tf == 0:
                continue
            idf = math.log(1 + (n - df[t] + 0.5) / (df[t] + 0.5))
            s += idf * (tf * (k1 + 1)) / (tf + k1 * (1 - b + b * (dl / avgdl if avgdl else 0)))
        scores.append(s)
    return scores


SELECTORS = {
    "keep_all": sel_keep_all,
    "forced_only": sel_forced_only,
    "recency": sel_recency,
    "frequency": sel_frequency,
    "bm25": sel_bm25,
}


# --- token-ceiling allocator ------------------------------------------------------------------


def allocate(
    units: list[str],
    forced: set[int],
    scores: list[float],
    budget_tokens: int,
    keep_all: bool,
) -> list[int]:
    """Force critical units, then greedily add the highest-scored eligible units whose inclusion
    keeps the *re-tokenized full candidate* within `budget_tokens`.

    Per model-research.md the ceiling is checked on the assembled candidate, not by summing
    per-unit estimates (subword merges make per-unit costs non-additive). ponytail: re-tokenizes
    the whole candidate each step -> O(n^2) tokenization; fine for fixtures, batch/increment for
    large corpora."""
    n = len(units)
    if keep_all:
        return list(range(n))

    kept = set(forced)

    def cost(idxs: set[int]) -> int:
        return count_tokens("".join(units[i] for i in sorted(idxs)))

    # Ranked non-forced units: score desc, original order as a stable tie-break.
    candidates = sorted(
        (i for i in range(n) if i not in forced),
        key=lambda i: (-scores[i], i),
    )
    for i in candidates:
        if scores[i] == -math.inf:
            break  # forced_only: nothing beyond the floor
        trial = kept | {i}
        if cost(trial) <= budget_tokens:
            kept = trial
    return sorted(kept)


# --- task scoring (deterministic proxy) -------------------------------------------------------


def score_task(kept_text: str, fixture: dict) -> dict:
    """Deterministic proxy for downstream task success: every critical atom present AND the gold
    answer span present in the retained context. Not an LLM judge (which model-research.md
    reserves for diagnosing failures, never for satisfying a gate)."""
    atoms = fixture.get("critical_atoms", [])
    survived = sum(1 for a in atoms if a in kept_text)
    atom_survival = survived / len(atoms) if atoms else 1.0
    gold = fixture.get("gold_answer")
    answer_present = 1.0 if (not gold or gold in kept_text) else 0.0
    success = 1.0 if atom_survival == 1.0 and answer_present == 1.0 else 0.0
    return {
        "task_success": success,
        "critical_atom_survival": atom_survival,
        "answer_present": answer_present,
    }


# --- one run + aggregation --------------------------------------------------------------------


def run_one(fixture: dict, baseline: str, target_ratio: float) -> dict:
    source = fixture["source"]
    units = segment(source)
    raw_tokens = count_tokens(source)
    budget = round(raw_tokens * target_ratio)
    atoms = fixture.get("critical_atoms", [])
    forced = forced_indices(units, atoms)
    scores = SELECTORS[baseline](units, fixture.get("query", ""))
    kept = allocate(units, forced, scores, budget, keep_all=(baseline == "keep_all"))
    kept_text = "".join(units[i] for i in kept)
    achieved = count_tokens(kept_text)
    forced_floor = count_tokens("".join(units[i] for i in sorted(forced)))
    result = {
        "baseline": baseline,
        "fixture": fixture["id"],
        "family": fixture.get("family"),
        "target_ratio": target_ratio,
        "raw_tokens": raw_tokens,
        "budget_tokens": budget,
        "achieved_tokens": achieved,
        "achieved_ratio": round(achieved / raw_tokens, 4) if raw_tokens else 0.0,
        "forced_floor_tokens": forced_floor,
        # keep_all is the uncompressed upper bound, not subject to the ceiling.
        "over_budget": baseline != "keep_all" and achieved > budget,
    }
    result.update(score_task(kept_text, fixture))
    return result


def _mean(xs: list[float]) -> float:
    return round(sum(xs) / len(xs), 4) if xs else 0.0


def build_report(fixtures: list[dict], ratios: list[float]) -> dict:
    rows = [
        run_one(fx, name, r)
        for name in SELECTORS
        for r in ratios
        for fx in fixtures
    ]
    summary = []
    for name in SELECTORS:
        for r in ratios:
            group = [x for x in rows if x["baseline"] == name and x["target_ratio"] == r]
            summary.append(
                {
                    "baseline": name,
                    "target_ratio": r,
                    "mean_task_success": _mean([x["task_success"] for x in group]),
                    "mean_critical_atom_survival": _mean(
                        [x["critical_atom_survival"] for x in group]
                    ),
                    "mean_achieved_ratio": _mean([x["achieved_ratio"] for x in group]),
                    "over_budget_count": sum(1 for x in group if x["over_budget"]),
                }
            )
    return {
        "harness": "v0.4-alpha-baselines",
        "tokenizer": TOKENIZER,
        "fixture_count": len(fixtures),
        "baselines": list(SELECTORS),
        "ratios": ratios,
        "summary": summary,
        "rows": rows,
    }


# --- fixtures + CLI ---------------------------------------------------------------------------


def load_fixtures(tasks_dir: Path) -> list[dict]:
    fixtures = []
    for path in sorted(tasks_dir.glob("*.json")):
        fixtures.append(json.loads(path.read_text(encoding="utf-8")))
    return fixtures


def run_gate(fixtures: list[dict], ratios: list[float]) -> int:
    """Assert the invariants v0.4-alpha guarantees by construction. Returns process exit code."""
    failures = []
    for fx in fixtures:
        for r in ratios:
            for name in SELECTORS:
                res = run_one(fx, name, r)
                # 1. Deterministic forcing => 100% critical-atom survival for every selector.
                if res["critical_atom_survival"] != 1.0:
                    failures.append(
                        f"{name}/{fx['id']}@{r}: critical_atom_survival="
                        f"{res['critical_atom_survival']} (must be 1.0)"
                    )
                # 2. Ceiling respected (unless the forced floor alone exceeds the budget).
                if (
                    name != "keep_all"
                    and res["achieved_tokens"] > res["budget_tokens"]
                    and res["forced_floor_tokens"] <= res["budget_tokens"]
                ):
                    failures.append(
                        f"{name}/{fx['id']}@{r}: achieved {res['achieved_tokens']} > budget "
                        f"{res['budget_tokens']} while floor fit"
                    )
            # 3. keep_all is the upper bound: full task success.
            top = run_one(fx, "keep_all", r)
            if top["task_success"] != 1.0:
                failures.append(f"keep_all/{fx['id']}@{r}: task_success != 1.0")

    artifact = {
        "gate": "pass" if not failures else "fail",
        "tokenizer": TOKENIZER,
        "checked": len(fixtures) * len(ratios) * len(SELECTORS),
        "failures": failures,
    }
    print(json.dumps(artifact, indent=2))
    return 0 if not failures else 1


def _print_summary(report: dict) -> None:
    print(f"# v0.4-alpha baselines  (tokenizer: {report['tokenizer']['backend']})")
    print(f"# {report['fixture_count']} fixtures  ratios={report['ratios']}\n")
    print(f"{'baseline':<13}{'ratio':>6}{'task':>7}{'crit':>7}{'achieved':>10}{'over':>6}")
    for s in report["summary"]:
        print(
            f"{s['baseline']:<13}{s['target_ratio']:>6}{s['mean_task_success']:>7}"
            f"{s['mean_critical_atom_survival']:>7}{s['mean_achieved_ratio']:>10}"
            f"{s['over_budget_count']:>6}"
        )


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description="v0.4-alpha deterministic baseline harness")
    p.add_argument(
        "--tasks-dir",
        default=str(Path(__file__).parent / "tasks" / "v04"),
        help="directory of paired-task fixtures",
    )
    p.add_argument(
        "--ratios",
        default="0.5,0.25",
        help="comma-separated target token-retention ratios (default: 0.5,0.25)",
    )
    p.add_argument("--gate", action="store_true", help="assert invariants; exit non-zero on fail")
    p.add_argument("--json", action="store_true", help="print the full JSON report")
    return p


def main(argv=None) -> int:
    args = build_parser().parse_args(argv)
    ratios = [float(x) for x in args.ratios.split(",") if x.strip()]
    fixtures = load_fixtures(Path(args.tasks_dir))
    if not fixtures:
        print(f"no fixtures found in {args.tasks_dir}", file=sys.stderr)
        return 2
    if args.gate:
        return run_gate(fixtures, ratios)
    report = build_report(fixtures, ratios)
    if args.json:
        print(json.dumps(report, indent=2))
    else:
        _print_summary(report)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
