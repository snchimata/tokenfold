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

Selectors: keep_all, forced_only, recency, frequency, bm25, llmlingua_style (a perplexity-free
self-information proxy). Compressor baselines: deterministic-tokenfold (Rust CLI).

Deliberately deferred (documented, not hidden — see `eval/tasks/v04/README.md`):
  - RTK and RTK+tokenfold (external tool) and the unmodified Headroom Kompress-v2 achieved-token
    sweep (needs the ML checkpoint) as additional baselines.
  - Real Tier-B public-repo corpora and project-disjoint train/test splits.
  - Structural (diff-hunk / JSON-container / AST) segmentation; v0.4-alpha segments by line.
  - An LLM judge for task success (the current scorer is a deterministic containment proxy).
"""

from __future__ import annotations

import argparse
import json
import math
import os
import re
import shutil
import subprocess
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


def sel_llmlingua_style(units: list[str], query: str) -> list[float]:
    """Perplexity-free LLMLingua-style proxy: keep high-information units, drop predictable /
    redundant ones. Scores each unit by mean per-token self-information (surprisal,
    `-log2 P(token)`) under a unigram model estimated from the document itself — a deterministic
    stand-in for LLMLingua's small-LM token perplexity (the real method needs an LM at inference,
    deferred: model-research.md keeps ML off the default path). Query-independent like `frequency`,
    but an information-theoretic surprisal rather than a `1/df` heuristic, so boilerplate lines of
    common tokens rank low and lines carrying rare/surprising content rank high."""
    counts: dict[str, int] = {}
    total = 0
    for unit in units:
        for tok in _tokens(unit):
            counts[tok] = counts.get(tok, 0) + 1
            total += 1
    if total == 0:
        return [0.0] * len(units)
    scores = []
    for unit in units:
        toks = _tokens(unit)
        if not toks:
            scores.append(0.0)
            continue
        surprisal = sum(-math.log2(counts[t] / total) for t in toks) / len(toks)
        scores.append(surprisal)
    return scores


SELECTORS = {
    "keep_all": sel_keep_all,
    "forced_only": sel_forced_only,
    "recency": sel_recency,
    "frequency": sel_frequency,
    "bm25": sel_bm25,
    "llmlingua_style": sel_llmlingua_style,
}


# --- whole-pipeline compressor baselines ------------------------------------------------------
# Unlike SELECTORS (which rank atomic units and get the harness's deterministic critical-atom
# forcing + hard ceiling), a COMPRESSOR runs an external best-effort pipeline over the whole
# source. It is NOT unit-selection: it may miss the exact ceiling (best effort) and the harness
# does not force critical atoms through it, so its critical-atom survival is *measured and
# reported*, never assumed. `deterministic-tokenfold` is the primary baseline to beat.


def _find_tokenfold() -> str | None:
    """Locate the tokenfold CLI: TOKENFOLD_BIN, then a local target build, then PATH."""
    env = os.environ.get("TOKENFOLD_BIN")
    if env and Path(env).is_file():
        return env
    root = Path(__file__).resolve().parent.parent
    exe = "tokenfold.exe" if os.name == "nt" else "tokenfold"
    for sub in ("target/release", "target/debug"):
        candidate = root / sub / exe
        if candidate.is_file():
            return str(candidate)
    return shutil.which("tokenfold")


_TOKENFOLD_BIN = _find_tokenfold()


def compress_tokenfold(source: str, budget: int) -> str | None:
    """Run `tokenfold compress --target-tokens <budget>` over `source` (auto-detected format),
    returning the compressed payload, or None when the CLI is unavailable/errors. Best-effort:
    tokenfold may return a payload above the budget when its lossless/evidence transforms cannot
    reach the target — that is a measured property, not a harness failure."""
    if not _TOKENFOLD_BIN:
        return None
    try:
        proc = subprocess.run(
            [_TOKENFOLD_BIN, "compress", "--quiet", "--target-tokens", str(budget)],
            input=source.encode("utf-8"),
            capture_output=True,
            timeout=30,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if not proc.stdout:
        return None
    return proc.stdout.decode("utf-8", errors="replace")


COMPRESSORS = {
    "deterministic-tokenfold": compress_tokenfold,
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


def _ws_strip(text: str) -> str:
    return re.sub(r"\s+", "", text)


def score_task(kept_text: str, fixture: dict) -> dict:
    """Deterministic proxy for downstream task success: every critical atom present AND the gold
    answer span present in the retained context. Not an LLM judge (which model-research.md
    reserves for diagnosing failures, never for satisfying a gate).

    Containment is whitespace-insensitive so a lossless reformat (e.g. a compressor minifying
    `"max_results": 25` to `"max_results":25`) still counts as surviving. Selector baselines keep
    original bytes, so this does not change their scores."""
    hay = _ws_strip(kept_text)
    atoms = fixture.get("critical_atoms", [])
    survived = sum(1 for a in atoms if _ws_strip(a) in hay)
    atom_survival = survived / len(atoms) if atoms else 1.0
    gold = fixture.get("gold_answer")
    answer_present = 1.0 if (not gold or _ws_strip(gold) in hay) else 0.0
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


def run_one_compressor(fixture: dict, name: str, target_ratio: float) -> dict:
    source = fixture["source"]
    raw_tokens = count_tokens(source)
    budget = round(raw_tokens * target_ratio)
    compressed = COMPRESSORS[name](source, budget)
    base = {
        "baseline": name,
        "fixture": fixture["id"],
        "family": fixture.get("family"),
        "target_ratio": target_ratio,
        "kind": "compressor",
    }
    if compressed is None:
        base["available"] = False
        return base
    achieved = count_tokens(compressed)
    base.update(
        {
            "available": True,
            "raw_tokens": raw_tokens,
            "budget_tokens": budget,
            "achieved_tokens": achieved,
            "achieved_ratio": round(achieved / raw_tokens, 4) if raw_tokens else 0.0,
            # Best effort: tokenfold may exceed the budget when lossless transforms can't reach
            # it. Informational, not a failure.
            "over_budget": achieved > budget,
        }
    )
    base.update(score_task(compressed, fixture))
    return base


def _mean(xs: list[float]) -> float:
    return round(sum(xs) / len(xs), 4) if xs else 0.0


def build_report(fixtures: list[dict], ratios: list[float]) -> dict:
    selector_rows = [
        run_one(fx, name, r) for name in SELECTORS for r in ratios for fx in fixtures
    ]
    compressor_rows = [
        run_one_compressor(fx, name, r) for name in COMPRESSORS for r in ratios for fx in fixtures
    ]
    summary = []
    for name in SELECTORS:
        for r in ratios:
            group = [x for x in selector_rows if x["baseline"] == name and x["target_ratio"] == r]
            summary.append(
                {
                    "baseline": name,
                    "kind": "selector",
                    "target_ratio": r,
                    "mean_task_success": _mean([x["task_success"] for x in group]),
                    "mean_critical_atom_survival": _mean(
                        [x["critical_atom_survival"] for x in group]
                    ),
                    "mean_achieved_ratio": _mean([x["achieved_ratio"] for x in group]),
                    "over_budget_count": sum(1 for x in group if x["over_budget"]),
                }
            )
    for name in COMPRESSORS:
        for r in ratios:
            group = [x for x in compressor_rows if x["baseline"] == name and x["target_ratio"] == r]
            avail = [x for x in group if x.get("available")]
            summary.append(
                {
                    "baseline": name,
                    "kind": "compressor",
                    "target_ratio": r,
                    "available": len(avail),
                    "of": len(group),
                    "mean_task_success": _mean([x["task_success"] for x in avail]),
                    "mean_critical_atom_survival": _mean(
                        [x["critical_atom_survival"] for x in avail]
                    ),
                    "mean_achieved_ratio": _mean([x["achieved_ratio"] for x in avail]),
                    "over_budget_count": sum(1 for x in avail if x["over_budget"]),
                }
            )
    return {
        "harness": "v0.4-alpha-baselines",
        "tokenizer": TOKENIZER,
        "fixture_count": len(fixtures),
        "selectors": list(SELECTORS),
        "compressors": list(COMPRESSORS),
        "tokenfold_available": _TOKENFOLD_BIN is not None,
        "ratios": ratios,
        "summary": summary,
        "rows": selector_rows + compressor_rows,
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
        # Compressor baselines are best-effort (measured, not gated); report availability only.
        "tokenfold_available": _TOKENFOLD_BIN is not None,
        "failures": failures,
    }
    print(json.dumps(artifact, indent=2))
    return 0 if not failures else 1


def _print_summary(report: dict) -> None:
    tf = "available" if report["tokenfold_available"] else "MISSING (skipped)"
    print(
        f"# v0.4-alpha baselines  (tokenizer: {report['tokenizer']['backend']}, "
        f"deterministic-tokenfold: {tf})"
    )
    print(f"# {report['fixture_count']} fixtures  ratios={report['ratios']}\n")
    print(f"{'baseline':<24}{'ratio':>6}{'task':>7}{'crit':>7}{'achieved':>10}{'over':>6}")
    for s in report["summary"]:
        if s["kind"] == "compressor" and s.get("available", 0) == 0:
            print(f"{s['baseline']:<24}{s['target_ratio']:>6}{'n/a':>7}{'n/a':>7}{'n/a':>10}{'-':>6}")
            continue
        print(
            f"{s['baseline']:<24}{s['target_ratio']:>6}{s['mean_task_success']:>7}"
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
