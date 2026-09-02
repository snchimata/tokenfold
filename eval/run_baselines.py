#!/usr/bin/env python3
"""v0.4-alpha learned-selector *baseline* harness (shadow-only).

This is the "baselines first" stage of the v0.4 learned-selector research plan:
before any model is trained, it runs a set of
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
  - RTK and RTK+tokenfold (external tool) as additional baselines, plus an achieved-token sweep
    of a third-party content-aware compressor.
  - Real Tier-B public-repo corpora and project-disjoint train/test splits.
  - Structural (diff-hunk / JSON-container / AST) segmentation; v0.4-alpha segments by line.
  - An LLM judge for task success (the current scorer is a deterministic containment proxy).
"""

from __future__ import annotations

import argparse
import json
import math
import atexit
import os
import re
import shutil
import subprocess
import tempfile
from datetime import datetime
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
    guarantee (a hard gate `--gate` asserts) rather than something a learned model must get
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
    deferred: ML stays off this harness's default path). Query-independent like `frequency`,
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

# Snapshot the deterministic set before any learned selector registers, so the report can label
# which scores came from a model vs. a stdlib heuristic.
DETERMINISTIC_SELECTORS = tuple(SELECTORS)


# --- optional learned selector (v0.4-beta, model code lives locally, never committed) ----------
# A trained keep/drop selector plugs in here as just another SELECTORS entry — no other harness
# change. Its code + weights live in a *gitignored* local module ($TOKENFOLD_LEARNED_MODULE,
# default `learned.selector`, under eval/) that owns the torch/transformers imports, so when the
# ML stack or the local module is absent the import fails and the selector is cleanly skipped,
# exactly like tiktoken and the tokenfold CLI above. Contract: the module
# exposes LEARNED_SELECTORS: dict[str, callable], each callable (units, query) -> list[float]
# emitting *scores only*; the harness's deterministic critical-atom forcing + ceiling allocator +
# byte-copy assembly still own the output (so 100% critical-atom survival stays structural, and
# `--gate` proves the learned selector cannot break it either).


def _load_learned_selectors() -> dict:
    import importlib

    eval_dir = str(Path(__file__).resolve().parent)
    if eval_dir not in sys.path:
        sys.path.insert(0, eval_dir)  # importable whether run as a script or imported by a test
    try:
        mod = importlib.import_module(
            os.environ.get("TOKENFOLD_LEARNED_MODULE", "learned.selector")
        )
        found = getattr(mod, "LEARNED_SELECTORS", {})
    except Exception:  # ML absent / module absent / weights missing -> skip, never crash
        return {}
    learned = {k: v for k, v in found.items() if callable(v)} if isinstance(found, dict) else {}
    # Integrity: a learned selector must never shadow a deterministic baseline. Overwriting one
    # would run the model under that name yet label it deterministic (set-difference below) and
    # silently drop the real baseline from the comparison. That defeats the whole report, so fail
    # loud instead — a naming clash is a bug in the local module, not something to paper over.
    clash = sorted(set(learned) & set(DETERMINISTIC_SELECTORS))
    if clash:
        raise ValueError(
            f"learned selector name(s) {clash} shadow deterministic baselines; rename them in the "
            "local model module — a learned selector must not overwrite a baseline."
        )
    return learned


LEARNED_SELECTORS = _load_learned_selectors()
SELECTORS.update(LEARNED_SELECTORS)


# --- whole-pipeline compressor baselines ------------------------------------------------------
# Unlike SELECTORS (which rank atomic units and get the harness's deterministic critical-atom
# forcing + hard ceiling), a COMPRESSOR runs an external best-effort pipeline over the whole
# source. It is NOT unit-selection: it may miss the exact ceiling (best effort) and the harness
# does not force critical atoms through it, so its critical-atom survival is *measured and
# reported*, never assumed. `deterministic-tokenfold` is the primary baseline to beat.


def _find_tokenfold() -> str | None:
    """Locate the tokenfold CLI: TOKENFOLD_BIN, then a local target build, then PATH.

    Between `target/release` and `target/debug`, prefers whichever was built MORE RECENTLY, not
    release unconditionally -- a stale release binary (e.g. from before a source change) would
    otherwise be silently preferred over a freshly-built debug binary that actually reflects the
    current source, with no signal to the caller that this happened. This doesn't eliminate
    staleness risk (a `TOKENFOLD_BIN` override, or a `target/release` newer than both current
    source AND target/debug, still needs the caller to have rebuilt intentionally) -- callers who
    need certainty should set TOKENFOLD_BIN explicitly to a binary they just built."""
    env = os.environ.get("TOKENFOLD_BIN")
    if env and Path(env).is_file():
        return env
    root = Path(__file__).resolve().parent.parent
    exe = "tokenfold.exe" if os.name == "nt" else "tokenfold"
    candidates = [
        root / sub / exe for sub in ("target/release", "target/debug")
    ]
    existing = [c for c in candidates if c.is_file()]
    if existing:
        newest = max(existing, key=lambda c: c.stat().st_mtime)
        return str(newest)
    return shutil.which("tokenfold")


_TOKENFOLD_BIN = _find_tokenfold()
_TOKENFOLD_BIN_MTIME = (
    datetime.fromtimestamp(Path(_TOKENFOLD_BIN).stat().st_mtime).isoformat(timespec="seconds")
    if _TOKENFOLD_BIN and Path(_TOKENFOLD_BIN).is_file()
    else None
)

_ISOLATED_CONFIG_PATH: str | None = None


def isolated_retrieval_config() -> str:
    """A `tokenfold.toml`-shaped config file every harness compressor subprocess call passes via
    `--config`, so a `store_originals`/`--lossy` run never touches the invoking user's real
    default retrieval store, AND never silently picks up a project-level `tokenfold.toml`/
    `.tokenfoldrc` (both gitignored, so one can legitimately exist locally) that `--config`-less
    invocations auto-discover from the current directory. Created once per process (a real
    filesystem directory — the CLI's lossy path refuses a `memory` backend) and reused, and
    removed again at interpreter exit: a lossy run persists every dropped item under it, so
    leaving it behind would scatter copies of fixture content through the system temp dir on
    every invocation."""
    global _ISOLATED_CONFIG_PATH
    if _ISOLATED_CONFIG_PATH is None:
        root = Path(tempfile.mkdtemp(prefix="tokenfold_eval_store_"))
        atexit.register(shutil.rmtree, root, ignore_errors=True)
        store_path = (root / "store").as_posix()
        config_path = root / "config.toml"
        config_path.write_text(
            f'[retrieval]\nbackend = "filesystem"\nstore_path = "{store_path}"\n',
            encoding="utf-8",
        )
        _ISOLATED_CONFIG_PATH = str(config_path)
    return _ISOLATED_CONFIG_PATH


def compress_tokenfold(source: str, budget: int) -> str | None:
    """Run `tokenfold compress --target-tokens <budget>` over `source` (auto-detected format),
    returning the compressed payload, or None when the CLI is unavailable/errors. Best-effort:
    tokenfold may return a payload above the budget when its lossless/evidence transforms cannot
    reach the target — that is a measured property, not a harness failure."""
    if not _TOKENFOLD_BIN:
        return None
    try:
        proc = subprocess.run(
            [
                _TOKENFOLD_BIN,
                "compress",
                "--quiet",
                "--target-tokens",
                str(budget),
                "--config",
                isolated_retrieval_config(),
            ],
            input=source.encode("utf-8"),
            capture_output=True,
            timeout=30,
        )
    except (OSError, subprocess.SubprocessError) as e:
        print(f"# tokenfold compressor subprocess error: {e}", file=sys.stderr)
        return None
    if proc.returncode != 0 or not proc.stdout:
        print(
            f"# tokenfold compressor exited {proc.returncode}: "
            f"{proc.stderr.decode('utf-8', errors='replace')[:500]}",
            file=sys.stderr,
        )
        return None
    return proc.stdout.decode("utf-8", errors="replace")


def compress_tokenfold_lossy(source: str, budget: int) -> str | None:
    """Run `tokenfold compress --lossy heuristic --lossy-ratio <r>` over `source`, where `r` is
    `budget / raw_tokens` (this harness's `budget` is a token-count ceiling; the CLI's lossy path
    takes a fraction of the *prunable pool* to keep — not the same denominator, but the closest
    honest translation without duplicating tokenfold's own tokenizer here). Best-effort like
    `compress_tokenfold`: returns None when the CLI is unavailable/errors. Non-JSON fixtures (and
    OpenAI/Anthropic-message-shaped JSON, since v0.4-alpha only runs lossy pruning on generic
    JSON — see `pipeline::apply_lossy_reduction`'s format gate) fall through as a lossless-only
    run (`json_prune` reports `NotApplicableToFormat`) rather than erroring — that is real,
    measured behavior, not a bug."""
    if not _TOKENFOLD_BIN:
        return None
    raw_tokens = count_tokens(source)
    ratio = min(0.99, budget / raw_tokens) if raw_tokens else 0.0
    try:
        proc = subprocess.run(
            [
                _TOKENFOLD_BIN,
                "compress",
                "--quiet",
                "--lossy",
                "heuristic",
                "--lossy-ratio",
                f"{ratio:.4f}",
                "--config",
                isolated_retrieval_config(),
            ],
            input=source.encode("utf-8"),
            capture_output=True,
            timeout=30,
        )
    except (OSError, subprocess.SubprocessError) as e:
        print(f"# tokenfold lossy compressor subprocess error: {e}", file=sys.stderr)
        return None
    if proc.returncode != 0 or not proc.stdout:
        print(
            f"# tokenfold lossy compressor exited {proc.returncode}: "
            f"{proc.stderr.decode('utf-8', errors='replace')[:500]}",
            file=sys.stderr,
        )
        return None
    return proc.stdout.decode("utf-8", errors="replace")


COMPRESSORS = {
    "deterministic-tokenfold": compress_tokenfold,
    "lossy-tokenfold": compress_tokenfold_lossy,
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

    The ceiling is checked on the assembled candidate, not by summing
    per-unit estimates (subword merges make per-unit costs non-additive) -- `cost()` below still
    does that exact full re-tokenize and remains the one thing that can ever accept a candidate
    the shortcuts below aren't sure about.

    Naive re-tokenizing the whole growing candidate on every single candidate is O(n^2) and, at
    the corpus's real size (some fixtures segment into thousands of units), measured in the
    multiple minutes for one allocate() call on the largest documents -- called 15x per fixture
    (5 selectors x 3 ratios), so a handful of oversized fixtures can dominate a whole run's
    wall-clock time. Two shortcuts below skip the expensive re-tokenize when they can already show
    the answer, in increasing order of how much they're allowed to assume:

    1. PROVEN, not assumed: a token can never cost fewer than 1 byte (tiktoken's BPE vocab has a
       byte-fallback for every byte value; the heuristic fallback is literally bytes/4, and
       ceil(x) <= x). So `count_tokens(text) <= len(text.encode())` always, unconditionally --
       summed per-unit BYTE length is a mathematically safe upper bound on the true joint cost.
       Whenever it alone already fits the budget, accept with zero risk, zero assumption.
    2. TESTED, not proven: per-unit TOKEN counts are NOT byte length -- concatenation can
       occasionally need MORE tokens than the sum of the parts (verified by fuzzing: real,
       reproducible boundary-merge counterexamples exist; cost() isn't even reliably monotonic as
       more text is added). But the size of that overshoot is bounded in practice: across tens of
       thousands of fuzzed trials over diverse content (prose, code/log syntax, unicode, digits,
       long runs of whitespace/repeated chars) and unit counts up to 8000, the worst observed
       overshoot was well under 0.5 tokens per unit in the trial set, generally falling well
       below that as the unit count grows. MARGIN_PER_UNIT below (0.5/unit + a flat floor) is
       comfortably above every worst case found. This is an empirically-calibrated safety margin,
       not a mathematical guarantee -- so it is used ONLY to decide whether to skip the exact
       check; a candidate it's unsure about (or, in principle, ever wrong about) still falls
       through to the exact `cost()` call, which is what actually decides.

    Only when NEITHER shortcut can already prove the answer does the exact O(current-size)
    re-tokenize run -- which in practice is a small fraction of candidates (most either clearly
    fit or clearly don't, well before the budget ceiling), turning the common case from O(n^2)
    into close to O(n)."""
    n = len(units)
    if keep_all:
        return list(range(n))

    def cost(idxs: set[int]) -> int:
        return count_tokens("".join(units[i] for i in sorted(idxs)))

    MARGIN_PER_UNIT = 0.5
    MARGIN_FLOOR = 32

    per_unit_bytes = [len(u.encode("utf-8")) for u in units]
    per_unit_tokens = [count_tokens(u) for u in units]
    kept = set(forced)
    kept_bytes = sum(per_unit_bytes[i] for i in kept)
    kept_tokens = sum(per_unit_tokens[i] for i in kept)

    # Ranked non-forced units: score desc, original order as a stable tie-break.
    candidates = sorted(
        (i for i in range(n) if i not in forced),
        key=lambda i: (-scores[i], i),
    )
    for i in candidates:
        if scores[i] == -math.inf:
            break  # forced_only: nothing beyond the floor
        trial_bytes = kept_bytes + per_unit_bytes[i]
        trial_tokens = kept_tokens + per_unit_tokens[i]
        margin = MARGIN_FLOOR + MARGIN_PER_UNIT * (len(kept) + 1)
        fits = (
            trial_bytes <= budget_tokens  # (1) proven-safe
            or trial_tokens + margin <= budget_tokens  # (2) tested-safe
            or cost(kept | {i}) <= budget_tokens  # exact fallback -- the only actual decider
        )
        if fits:
            kept.add(i)
            kept_bytes = trial_bytes
            kept_tokens = trial_tokens
    return sorted(kept)


# --- task scoring (deterministic proxy) -------------------------------------------------------


def _ws_strip(text: str) -> str:
    return re.sub(r"\s+", "", text)


def _logical_text(text: str) -> str:
    """Undo tokenfold's lossless columnar JSON representation before evidence scoring."""
    try:
        value = json.loads(text)
    except (json.JSONDecodeError, TypeError):
        return text

    def unfold(node):
        if isinstance(node, dict):
            if set(node) == {"__tf_cols__", "__tf_rows__"}:
                cols, rows = node["__tf_cols__"], node["__tf_rows__"]
                if isinstance(cols, list) and isinstance(rows, list):
                    return [
                        {str(key): unfold(value) for key, value in zip(cols, row)}
                        for row in rows
                        if isinstance(row, list) and len(row) == len(cols)
                    ]
            return {key: unfold(value) for key, value in node.items()}
        if isinstance(node, list):
            return [unfold(value) for value in node]
        return node

    return json.dumps(unfold(value), separators=(",", ":"), ensure_ascii=False)


def score_task(kept_text: str, fixture: dict) -> dict:
    """Deterministic downstream outcome: safety atoms and the answer's source evidence survive.

    `gold_answer` alone is too weak: a detached value can survive after the subject/predicate that
    makes it meaningful was dropped. Claim faithfulness therefore requires the complete unique
    source line containing the answer (or an explicit `supporting_evidence` span) to survive too.
    This is deterministic evidence grounding, not a semantic or LLM judge.

    Containment is whitespace-insensitive so a lossless reformat (e.g. a compressor minifying
    `"max_results": 25` to `"max_results":25`) still counts as surviving. Selector baselines keep
    original bytes, so this does not change their scores."""
    hay = _ws_strip(_logical_text(kept_text))
    atoms = fixture.get("critical_atoms", [])
    survived = sum(1 for a in atoms if _ws_strip(a) in hay)
    atom_survival = survived / len(atoms) if atoms else 1.0
    gold = fixture.get("gold_answer")
    answer_present = 1.0 if (not gold or _ws_strip(gold) in hay) else 0.0
    evidence = fixture.get("supporting_evidence") or next(
        line for line in fixture["source"].splitlines() if _ws_strip(gold) in _ws_strip(line)
    )
    claim_faithfulness = 1.0 if _ws_strip(evidence) in hay else 0.0
    success = (
        1.0
        if atom_survival == 1.0
        and answer_present == 1.0
        and claim_faithfulness == 1.0
        else 0.0
    )
    return {
        "task_success": success,
        "critical_atom_survival": atom_survival,
        "answer_present": answer_present,
        "claim_faithfulness": claim_faithfulness,
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
        run_one(fx, name, r)
        for name in SELECTORS
        for r in ratios
        for fx in fixtures
        if fx.get("evaluation_kind", "paired") == "paired"
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
                    "mean_claim_faithfulness": _mean(
                        [x["claim_faithfulness"] for x in group]
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
                    "mean_claim_faithfulness": _mean(
                        [x["claim_faithfulness"] for x in avail]
                    ),
                    "mean_achieved_ratio": _mean([x["achieved_ratio"] for x in avail]),
                    "over_budget_count": sum(1 for x in avail if x["over_budget"]),
                }
            )
    return {
        "harness": "v0.4-alpha-baselines",
        "tokenizer": TOKENIZER,
        "fixture_count": len(fixtures),
        "selector_fixture_count": sum(
            fx.get("evaluation_kind", "paired") == "paired" for fx in fixtures
        ),
        "selectors": list(SELECTORS),
        "deterministic_selectors": list(DETERMINISTIC_SELECTORS),
        # v0.4-beta: names registered from the gitignored local model module, [] when ML is absent.
        "learned_selectors": [s for s in SELECTORS if s not in DETERMINISTIC_SELECTORS],
        "compressors": list(COMPRESSORS),
        "tokenfold_available": _TOKENFOLD_BIN is not None,
        "tokenfold_bin": _TOKENFOLD_BIN,
        "tokenfold_bin_built_at": _TOKENFOLD_BIN_MTIME,
        "ratios": ratios,
        "summary": summary,
        "rows": selector_rows + compressor_rows,
    }


# --- fixtures + CLI ---------------------------------------------------------------------------


def load_fixtures(tasks_dir: Path) -> list[dict]:
    fixtures = []
    seen_ids = set()
    for path in sorted(tasks_dir.glob("*.json")):
        fixture = json.loads(path.read_text(encoding="utf-8"))
        _validate_fixture(fixture, path, seen_ids)
        seen_ids.add(fixture["id"])
        fixtures.append(fixture)
    return fixtures


def _validate_fixture(fixture: dict, path: Path, seen_ids: set[str]) -> None:
    """Reject fixtures that could make the deterministic scorer pass vacuously.

    The evaluator is only as meaningful as this contract: the task, answer, and safety atoms
    must all be explicit, non-empty, and grounded in the source. The answer must occur exactly
    once after the scorer's whitespace normalization, otherwise containment cannot identify one
    deterministic outcome.
    """
    required_strings = ("id", "family", "tier", "source", "query", "gold_answer")
    for field in required_strings:
        if not isinstance(fixture.get(field), str) or not fixture[field].strip():
            raise ValueError(f"{path}: {field} must be a non-empty string")
    if fixture["id"] in seen_ids:
        raise ValueError(f"{path}: duplicate fixture id {fixture['id']!r}")
    if fixture["tier"] != "A":
        raise ValueError(f"{path}: unsupported provenance tier {fixture['tier']!r}")
    if fixture.get("evaluation_kind", "paired") not in ("paired", "compressor"):
        raise ValueError(f"{path}: evaluation_kind must be 'paired' or 'compressor'")

    atoms = fixture.get("critical_atoms")
    if not isinstance(atoms, list) or not atoms or any(
        not isinstance(atom, str) or not atom.strip() for atom in atoms
    ):
        raise ValueError(f"{path}: critical_atoms must be a non-empty list of strings")
    if len(atoms) != len(set(atoms)):
        raise ValueError(f"{path}: critical_atoms must not contain duplicates")

    source = _ws_strip(fixture["source"])
    gold = _ws_strip(fixture["gold_answer"])
    if source.count(gold) != 1:
        raise ValueError(f"{path}: gold_answer must occur exactly once in source")
    evidence = fixture.get("supporting_evidence")
    if evidence is not None and (not isinstance(evidence, str) or not evidence.strip()):
        raise ValueError(f"{path}: supporting_evidence must be a non-empty string")
    evidence = _ws_strip(evidence) if evidence is not None else gold
    if source.count(evidence) != 1:
        raise ValueError(f"{path}: supporting_evidence must occur exactly once in source")
    for atom in atoms:
        normalized = _ws_strip(atom)
        if normalized not in source:
            raise ValueError(f"{path}: critical atom {atom!r} is not grounded in source")
        if normalized in gold or gold in normalized:
            raise ValueError(f"{path}: gold_answer must not also be a critical atom")


def _find_tf_ref_hashes(value) -> list[str]:
    """Recursively collects every `$tf_ref` marker's `hash` field out of a parsed JSON value."""
    hashes: list[str] = []
    if isinstance(value, dict):
        ref = value.get("$tf_ref")
        if len(value) == 1 and isinstance(ref, dict) and isinstance(ref.get("hash"), str):
            hashes.append(ref["hash"])
            return hashes
        for v in value.values():
            hashes.extend(_find_tf_ref_hashes(v))
    elif isinstance(value, list):
        for v in value:
            hashes.extend(_find_tf_ref_hashes(v))
    return hashes


def _lossy_smoke_checks() -> list[str]:
    """Gate checks the per-fixture loop below structurally cannot catch: whether
    `lossy-tokenfold` is even capable of activating (dropping anything) at all, whether every
    marker it emits actually round-trips through `tokenfold retrieve`, and whether
    `--lossy-preserve` still protects a whole array. A round-4 external review found BOTH real
    v04 lossy fixtures produce zero `$tf_ref` markers at every tested ratio, and the existing
    gate has no check that would ever catch a `lossy-tokenfold` that's silently inert -- this
    runs against a synthetic payload built specifically to be prunable (repeated-but-not-
    identical natural-language-ish text; a `$tf_ref` marker's hex hash tokenizes far less
    efficiently than real BPE-friendly text, so naive filler like `"x" * N` never activates it —
    same lesson already documented on `compress_tokenfold_lossy`), independent of what any real
    fixture happens to look like. Returns a list of failure strings (empty if everything holds)."""
    if not _TOKENFOLD_BIN:
        return []
    words = [
        "processed", "batch", "successfully", "no", "anomalies", "detected", "in", "shard",
        "cache", "warm", "hit", "ratio", "nominal", "replication", "lag", "within", "tolerance",
        "checksum", "verified", "for", "segment", "worker", "completed", "cycle", "queue",
        "drain", "normal",
    ]
    items = [
        {
            "id": i,
            "note": f"item {i}: "
            + " ".join(words[(i * 11 + j * 5 + (j * j) % 13) % len(words)] for j in range(120)),
        }
        for i in range(30)
    ]
    source = json.dumps({"items": items})
    raw_tokens = count_tokens(source)
    failures: list[str] = []

    compressed = compress_tokenfold_lossy(source, round(raw_tokens * 0.1))
    if compressed is None:
        return ["lossy smoke check: tokenfold binary present but the subprocess call failed"]
    hashes = _find_tf_ref_hashes(json.loads(compressed))
    if not hashes:
        failures.append(
            "lossy smoke check: lossy-tokenfold produced ZERO $tf_ref markers on a payload "
            "built specifically to be prunable -- the per-fixture checks below would trivially "
            "and silently pass against a lossy-tokenfold that never actually drops anything"
        )

    # Activation is necessary but not sufficient: dropping items is only worth doing if the
    # result is actually SMALLER than what the plain lossless pipeline achieves on the same
    # input. A round-5 external review measured the opposite -- lossy emitting ~3x the lossless
    # output while deleting nothing -- and nothing in this gate would have caught it, because
    # nothing here ever compared the two compressors against each other.
    lossless = compress_tokenfold(source, round(raw_tokens * 0.1))
    if lossless is None:
        failures.append("lossy smoke check: the lossless compressor call failed")
    else:
        lossy_tokens, lossless_tokens = count_tokens(compressed), count_tokens(lossless)
        if hashes and lossy_tokens >= lossless_tokens:
            failures.append(
                f"lossy smoke check: lossy-tokenfold spent {lossy_tokens} tokens where plain "
                f"lossless spent {lossless_tokens} on the same prunable payload -- dropping "
                "recoverable content has to buy something, or it is pure loss for no gain"
            )
    for h in hashes:
        proc = subprocess.run(
            [
                _TOKENFOLD_BIN,
                "--config",
                isolated_retrieval_config(),
                "retrieve",
                h,
                "--retrieve-namespace",
                "default",
            ],
            capture_output=True,
            timeout=30,
        )
        if proc.returncode != 0 or not proc.stdout:
            failures.append(
                f"lossy smoke check: `tokenfold retrieve {h}` failed (exit {proc.returncode}) "
                "-- a marker lossy-tokenfold emitted does not actually retrieve"
            )

    # --lossy-preserve must protect the whole array: zero markers with it set, at the SAME
    # aggressive ratio that reliably produced markers without it above.
    try:
        preserve_proc = subprocess.run(
            [
                _TOKENFOLD_BIN,
                "compress",
                "--quiet",
                "--lossy",
                "heuristic",
                "--lossy-ratio",
                "0.1",
                "--lossy-preserve",
                "items",
                "--config",
                isolated_retrieval_config(),
            ],
            input=source.encode("utf-8"),
            capture_output=True,
            timeout=30,
        )
    except (OSError, subprocess.SubprocessError) as e:
        failures.append(f"lossy smoke check: --lossy-preserve subprocess error: {e}")
        return failures
    if preserve_proc.returncode != 0:
        failures.append(
            f"lossy smoke check: --lossy-preserve items exited {preserve_proc.returncode}: "
            f"{preserve_proc.stderr.decode('utf-8', errors='replace')[:500]}"
        )
    elif b"$tf_ref" in preserve_proc.stdout:
        failures.append(
            "lossy smoke check: --lossy-preserve items still let a marker through -- the "
            "preserved array must come back completely untouched"
        )

    return failures


def run_gate(fixtures: list[dict], ratios: list[float]) -> int:
    """Assert the invariants v0.4-alpha guarantees by construction. Returns process exit code."""
    failures = []
    if _TOKENFOLD_BIN is None:
        # A round-4 external review found the previous version of this gate silently skipped
        # every compressor check when the binary was missing/broken and still reported "pass" --
        # `--gate` exists specifically to assert compressor invariants, so an unusable binary
        # must fail loudly, not degrade to only the pure-Python selector checks.
        failures.append(
            "tokenfold binary not found (TOKENFOLD_BIN unset and no target/release|debug build) "
            "-- --gate requires a working binary to assert compressor invariants"
        )
        artifact = {
            "gate": "fail",
            "tokenizer": TOKENIZER,
            "checked": 0,
            "tokenfold_available": False,
            "tokenfold_bin": None,
            "tokenfold_bin_built_at": None,
            "failures": failures,
        }
        print(json.dumps(artifact, indent=2))
        return 1

    failures.extend(_lossy_smoke_checks())
    for fx in fixtures:
        for r in ratios:
            selector_results = {}
            if fx.get("evaluation_kind", "paired") == "paired":
                for name in SELECTORS:
                    res = run_one(fx, name, r)
                    selector_results[name] = res
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
                top = selector_results["keep_all"]
                if top["task_success"] != 1.0:
                    failures.append(f"keep_all/{fx['id']}@{r}: task_success != 1.0")
                # Every paired fixture must produce a real task outcome at the tightest requested
                # ceiling: at least one deterministic pruning baseline fails while keep_all passes.
                if r == min(ratios) and all(
                    selector_results[name]["task_success"] == 1.0
                    for name in DETERMINISTIC_SELECTORS
                    if name != "keep_all"
                ):
                    failures.append(
                        f"{fx['id']}@{r}: no deterministic pruning baseline fails the task; "
                        "fixture is non-discriminating"
                    )
            # 4. Compressor baselines stay best-effort on budget/ratio (a real CLI subprocess,
            # not gated the way the pure-Python selectors above are), but critical-content
            # survival is a real data-safety property, not a ratio nicety, and a lossy
            # compressor specifically has real ways to violate it (recoverable array pruning
            # can drop the very records a downstream task needs)
            # -- so it IS gated here, for every compressor, whenever the binary is available.
            results = {}
            for name in COMPRESSORS:
                res = run_one_compressor(fx, name, r)
                results[name] = res
                if not res.get("available"):
                    # The binary IS present (checked above) but this specific call still
                    # failed/errored/timed out -- a round-4 external review found this used to
                    # be silently skipped (counted as "checked" without ever really being
                    # checked). A crash on one specific input is a real regression, not noise.
                    failures.append(
                        f"{name}/{fx['id']}@{r}: compressor unavailable/errored despite "
                        "tokenfold binary being present (see stderr above for the subprocess "
                        "failure reason)"
                    )
                    continue
                if res["critical_atom_survival"] != 1.0:
                    failures.append(
                        f"{name}/{fx['id']}@{r}: critical_atom_survival="
                        f"{res['critical_atom_survival']} (must be 1.0)"
                    )
                if res["task_success"] != 1.0:
                    failures.append(
                        f"{name}/{fx['id']}@{r}: deterministic task outcome failed "
                        f"(answer_present={res['answer_present']}, "
                        f"claim_faithfulness={res['claim_faithfulness']})"
                    )
            # 5. Opting into lossy must never come out WORSE than not opting in. Hitting a
            # budget stays best-effort (check 4's comment), but "I accepted data loss and got a
            # bigger payload than the lossless run would have given me" is not a near-miss, it's
            # a strictly-dominated outcome -- and it shipped: a round-5 external review measured
            # `--lossy-ratio 0.25` emitting ~3x the lossless bytes on two of the fixtures below
            # while dropping nothing at all. Nothing in this gate compared the two compressors
            # to each other, so 79 fixtures passed straight over it.
            lossy_res, lossless_res = results.get("lossy-tokenfold"), results.get(
                "deterministic-tokenfold"
            )
            if (
                lossy_res
                and lossless_res
                and lossy_res.get("available")
                and lossless_res.get("available")
                and lossy_res["achieved_tokens"] > lossless_res["achieved_tokens"]
            ):
                failures.append(
                    f"lossy-tokenfold/{fx['id']}@{r}: {lossy_res['achieved_tokens']} tokens vs "
                    f"deterministic-tokenfold's {lossless_res['achieved_tokens']} -- a lossy run "
                    "must never cost more than the lossless one on the same input"
                )

    artifact = {
        "gate": "pass" if not failures else "fail",
        "tokenizer": TOKENIZER,
        "checked": len(ratios)
        * (
            sum(fx.get("evaluation_kind", "paired") == "paired" for fx in fixtures)
            * len(SELECTORS)
            + len(fixtures) * len(COMPRESSORS)
        ),
        # Compressor baselines stay best-effort on hitting a budget/ratio (a real CLI subprocess
        # can legitimately fall short) -- but critical-content survival IS gated for every
        # available compressor (see check 4 above), not just reported.
        "tokenfold_available": _TOKENFOLD_BIN is not None,
        "tokenfold_bin": _TOKENFOLD_BIN,
        "tokenfold_bin_built_at": _TOKENFOLD_BIN_MTIME,
        "failures": failures,
    }
    print(json.dumps(artifact, indent=2))
    return 0 if not failures else 1


def _print_summary(report: dict) -> None:
    tf = "available" if report["tokenfold_available"] else "MISSING (skipped)"
    learned = report.get("learned_selectors") or ["none (ML absent, skipped)"]
    print(
        f"# v0.4-alpha baselines  (tokenizer: {report['tokenizer']['backend']}, "
        f"deterministic-tokenfold: {tf})"
    )
    print(f"# learned selectors: {', '.join(learned)}")
    print(f"# {report['fixture_count']} fixtures  ratios={report['ratios']}\n")
    print(
        f"{'baseline':<24}{'ratio':>6}{'task':>7}{'claim':>7}"
        f"{'crit':>7}{'achieved':>10}{'over':>6}"
    )
    for s in report["summary"]:
        if s["kind"] == "compressor" and s.get("available", 0) == 0:
            print(
                f"{s['baseline']:<24}{s['target_ratio']:>6}{'n/a':>7}{'n/a':>7}"
                f"{'n/a':>7}{'n/a':>10}{'-':>6}"
            )
            continue
        print(
            f"{s['baseline']:<24}{s['target_ratio']:>6}{s['mean_task_success']:>7}"
            f"{s['mean_claim_faithfulness']:>7}{s['mean_critical_atom_survival']:>7}"
            f"{s['mean_achieved_ratio']:>10}"
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
    try:
        ratios = [float(x) for x in args.ratios.split(",") if x.strip()]
    except ValueError:
        ratios = []
    if not ratios or any(not math.isfinite(r) or r <= 0.0 or r > 1.0 for r in ratios):
        print("ratios must be comma-separated numbers in (0, 1]", file=sys.stderr)
        return 2
    try:
        fixtures = load_fixtures(Path(args.tasks_dir))
    except (OSError, ValueError) as error:
        print(f"invalid fixture corpus: {error}", file=sys.stderr)
        return 2
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
