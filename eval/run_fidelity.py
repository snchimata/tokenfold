#!/usr/bin/env python3
"""Fidelity gate harness for tokenfold (bootstrap version).

WHAT THIS IS
------------
Two profiles of the fidelity gate described in `ENGINEERING.md` ("Fidelity
Tests") and `ROADMAP.md` (D-005, Fidelity Gate Thresholds) are implemented:

  * `smoke-first-consumer` - proves the harness *mechanism* end to end
    (argument parsing, fixture loading, per-fixture scoring, aggregation,
    gate artifact shape, exit codes) using fixtures small enough to
    hand-verify. Runs on merge + release (ENGINEERING.md's gate-profile
    table).
  * `full-lossy-promotion` - the release-gate profile that decides whether
    `log_compaction`/`diff_compaction` may leave `--experimental`. Adds an
    accuracy@ratio curve (bucketed by measured compression ratio), the
    ACON-style contrastive raw-passes/compressed-fails KPI, and
    critical-token needle-survival, across all four content types in
    `eval/tasks/full_lossy/` (command output, git diff, JSON, prose).

WHAT THIS IS NOT
----------------
Neither profile is a live LLM-judged accuracy@ratio harness. As of this
writing, no `ANTHROPIC_API_KEY` is configured for this project and the real
downstream-quality thresholds are still an open decision (see
`ROADMAP.md` D-005, status OPEN). So instead of calling a real model to
score "did the compressed text still let a downstream task succeed?", both
profiles substitute the same cheap, fully deterministic, stdlib-only proxy:

  * `quality_retention` is approximated by whitespace-token overlap between
    "original" and "compressed" (see `_quality_retention_proxy` below). This
    is a crude lexical measure, NOT a real downstream-task quality score.
    It cannot detect semantic drift, only gross token loss - and it treats
    every word as equally important, so it is known to under-score
    transforms (like `diff_compaction`) that deliberately drop unchanged
    context on the theory that context is recoverable/non-critical: a real
    judge could recognize that; this proxy cannot.
  * `smoke-first-consumer`'s `contrastive_failure_rate` is hardcoded to 0.0
    for every fixture (no contrastive check is attempted there at all).
    `full-lossy-promotion` computes a real, still-deterministic contrastive
    proxy instead (see `_contrastive_failure_proxy` below): a fixture counts
    as a contrastive failure when `compressed` drops below a quality floor
    or loses a critical token, even though `original` (trivially) "passes"
    by definition. This is still not a live judge, just a less trivial
    bootstrap stand-in than a hardcoded zero.
  * `critical_token_survival_rate` is the one metric in both profiles that
    is NOT a proxy - it is an exact, deterministic substring check, and is
    exactly as trustworthy as the harness is in general.

FUTURE WORK (tracked, not done here)
-------------------------------------
Upgrading to a live LLM-judged scorer - using a pinned model version and a
real `ANTHROPIC_API_KEY` - to compute genuine downstream-task
`quality_retention` and `contrastive_failure_rate` is a documented future
step (see `eval/tasks/FIXTURES.md` and `ENGINEERING.md` "Fidelity Tests").
This bootstrap only proves the gate mechanism works; it does not certify
that any transform meets the real D-005 thresholds once those are judged
by a live model against real downstream tasks.

ponytail: the task that added `full-lossy-promotion` allowed an *optional*
live-scoring code path behind an explicit flag when `ANTHROPIC_API_KEY` is
set. That path is deliberately not implemented here - no such key is
available in this environment, "optional" flexibility with no caller is
exactly the speculative flexibility ponytail says to skip, and the module
docstring already tracks the live-judge upgrade as future work above. Add
it when a real key and a real caller both exist.

DEPENDENCIES
------------
Python standard library only: json, argparse, hashlib, pathlib, sys,
statistics. No `requests`, `pytest`, or `numpy` is required to run either
gate (see `eval/pyproject.toml`).

USAGE
-----
    python eval/run_fidelity.py --gate --profile smoke-first-consumer
    python eval/run_fidelity.py --gate --profile full-lossy-promotion

`--gate` is required to actually execute an evaluation run - the gate is
meant to be an explicit, blocking action, not something that happens as a
side effect of merely invoking this script.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import statistics
import sys
from pathlib import Path

# ---------------------------------------------------------------------------
# Draft gate thresholds, per ROADMAP.md D-005 ("Fidelity Gate Thresholds",
# status OPEN as of this writing). These are the *draft* values agreed
# pre-Phase 2; final values are set after Phase 2 accuracy@ratio data is
# collected against a live judge. Until then, this bootstrap gate applies
# the Balanced-mode-equivalent draft numbers:
#   - quality_retention >= 0.95   (<=5% absolute downstream-score drop)
#   - contrastive_failure_rate <= 0.005
#   - critical_token_survival_rate >= 0.99
QUALITY_RETENTION_MIN = 0.95
CONTRASTIVE_FAILURE_RATE_MAX = 0.005
CRITICAL_TOKEN_SURVIVAL_RATE_MIN = 0.99

# `full-lossy-promotion` scores a fixture as a contrastive failure when the
# compressed side would plausibly no longer carry enough surrounding context
# for a downstream task to succeed, even if its critical tokens survive.
# 0.5 is a deliberately generous floor (a real judge would use task success,
# not a raw word-overlap threshold) - see module docstring's caveat about
# this proxy's known weak spot on context-dropping transforms.
CONTRASTIVE_QUALITY_FLOOR = 0.5

MODEL_VERSION = "deterministic-smoke-v0"
FULL_LOSSY_MODEL_VERSION = "deterministic-bootstrap-full-lossy-v0"
IMPLEMENTED_PROFILES = ("smoke-first-consumer", "full-lossy-promotion")

# Ratio buckets for the accuracy@ratio curve, keyed by measured compression
# ratio (1 - len(compressed)/len(original), the same "fraction of size
# removed" convention `pipeline.rs` uses for `max_ratio_*`). Boundaries are
# a small, fixed sweep - not tuned per transform - matching this profile's
# "small sweep of compression ratios" scope.
RATIO_BUCKETS = (
    ("light", 0.0, 0.4),
    ("medium", 0.4, 0.6),
    ("heavy", 0.6, 1.01),  # 1.01 so a ratio of exactly 1.0 still falls in range
)

SCRIPT_DIR = Path(__file__).resolve().parent
FIXTURES_DIR = SCRIPT_DIR / "tasks" / "smoke"
FULL_LOSSY_FIXTURES_DIR = SCRIPT_DIR / "tasks" / "full_lossy"
# full-lossy-promotion only gates the two transforms still behind
# --experimental (roadmap.md Task 9); json_minify/schema_compaction were
# already promoted in Phase 2 and aren't re-litigated here.
FULL_LOSSY_TRANSFORMS = ("log_compaction", "diff_compaction")


def _quality_retention_proxy(original: str, compressed: str) -> float:
    """Crude deterministic lexical-overlap proxy for "quality retention".

    This is NOT a real downstream-task quality score. It splits both
    strings into whitespace-separated word *sets* and returns
    len(intersection) / len(original_words): the fraction of unique
    original words that are still present (as whole tokens) somewhere in
    the compressed text. It cannot detect semantic drift, reordering
    within a token, or meaning changes that reuse the same words - it is
    only a cheap bootstrap stand-in until a live LLM judge is wired up.
    """
    original_words = set(original.split())
    compressed_words = set(compressed.split())
    if not original_words:
        # Guard against division by zero: an empty original trivially
        # "retains" everything (there is nothing to lose).
        return 1.0
    return len(original_words & compressed_words) / len(original_words)


def _critical_token_survival(compressed: str, critical_tokens: list) -> float:
    """1.0 if every critical token survives verbatim in `compressed`, else 0.0."""
    return 1.0 if all(token in compressed for token in critical_tokens) else 0.0


def _contrastive_failure(_original: str, _compressed: str) -> float:
    """Always 0.0 in this deterministic smoke harness.

    A real contrastive check - does a downstream task that passes on raw
    input fail on compressed input, per a live model? - requires a live
    judge and is documented future work (see module docstring and
    eval/tasks/FIXTURES.md). It is NOT computed here.
    """
    return 0.0


def _contrastive_failure_proxy(
    original: str, compressed: str, critical_tokens: list
) -> float:
    """Deterministic ACON-style contrastive proxy for `full-lossy-promotion`.

    `original` is defined to "pass" trivially (critical tokens are always
    authored to exist verbatim in `original`; see eval/tasks/FIXTURES.md).
    `compressed` "fails" - a contrastive failure, the headline safety KPI
    from plan.md's ACON-style framing - when either a critical token no
    longer survives, or the lexical-overlap proxy drops below
    `CONTRASTIVE_QUALITY_FLOOR` (too little surrounding context plausibly
    remains). Still a bootstrap proxy, not a live judge - see module
    docstring.
    """
    tokens_survive = _critical_token_survival(compressed, critical_tokens) == 1.0
    quality_ok = _quality_retention_proxy(original, compressed) >= CONTRASTIVE_QUALITY_FLOOR
    compressed_passes = tokens_survive and quality_ok
    return 0.0 if compressed_passes else 1.0


def _compression_ratio(original: str, compressed: str) -> float:
    """Fraction of `original`'s length removed by compression.

    Mirrors `pipeline.rs`'s `1.0 - (tokens_after / tokens_before)` convention
    for `max_ratio_*`, approximated here with character length (fixtures
    have no token counts) since this is a bootstrap harness, not the real
    estimator.
    """
    if not original:
        return 0.0
    return 1.0 - (len(compressed) / len(original))


def _ratio_bucket(ratio: float) -> str:
    """Assigns `ratio` to a bucket name from `RATIO_BUCKETS`."""
    for name, low, high in RATIO_BUCKETS:
        if low <= ratio < high:
            return name
    return RATIO_BUCKETS[-1][0]


def _load_fixtures_from(fixtures_dir: Path, pattern: str):
    """Load every `pattern`-matching fixture in `fixtures_dir`, sorted by
    filename for determinism.

    Returns a list of (path, raw_bytes, parsed_dict) tuples.
    """
    fixture_paths = sorted(fixtures_dir.glob(pattern))
    fixtures = []
    for path in fixture_paths:
        raw = path.read_bytes()
        data = json.loads(raw.decode("utf-8"))
        fixtures.append((path, raw, data))
    return fixtures


def _load_fixtures():
    """Load every task_*.json fixture from the smoke fixture set."""
    return _load_fixtures_from(FIXTURES_DIR, "task_*.json")


def run_gate(profile: str) -> int:
    """Run the fidelity gate for `profile` and print the JSON artifact.

    Returns the process exit code (0 for gate=pass, 1 for gate=fail).
    """
    fixtures = _load_fixtures()

    fixture_hashes = []
    quality_scores = []
    contrastive_scores = []
    critical_scores = []
    transform_ids = set()

    for path, raw, data in fixtures:
        digest = hashlib.sha256(raw).hexdigest()
        fixture_hashes.append(f"sha256:{digest}")

        original = data["original"]
        compressed = data["compressed"]
        critical_tokens = data["critical_tokens"]

        quality_scores.append(_quality_retention_proxy(original, compressed))
        contrastive_scores.append(_contrastive_failure(original, compressed))
        critical_scores.append(_critical_token_survival(compressed, critical_tokens))
        transform_ids.add(data["transform_id"])

    quality_retention = statistics.mean(quality_scores) if quality_scores else 0.0
    contrastive_failure_rate = (
        statistics.mean(contrastive_scores) if contrastive_scores else 0.0
    )
    critical_token_survival_rate = (
        statistics.mean(critical_scores) if critical_scores else 0.0
    )

    gate_pass = (
        quality_retention >= QUALITY_RETENTION_MIN
        and contrastive_failure_rate <= CONTRASTIVE_FAILURE_RATE_MAX
        and critical_token_survival_rate >= CRITICAL_TOKEN_SURVIVAL_RATE_MIN
    )

    artifact = {
        "profile": profile,
        "gate": "pass" if gate_pass else "fail",
        "model_version": MODEL_VERSION,
        "fixture_hashes": fixture_hashes,
        "total_cost_usd": 0.0,
        "quality_retention": quality_retention,
        "contrastive_failure_rate": contrastive_failure_rate,
        "critical_token_survival_rate": critical_token_survival_rate,
        "transforms_evaluated": sorted(transform_ids),
    }

    print(json.dumps(artifact, indent=2))

    return 0 if gate_pass else 1


def run_full_lossy_promotion_gate(profile: str) -> int:
    """Run the `full-lossy-promotion` gate and print the JSON artifact.

    Evaluates `eval/tasks/full_lossy/**` (all four content types: command
    output, git diff, JSON, prose) for `log_compaction` and
    `diff_compaction`, computing the same bootstrap metrics as
    `smoke-first-consumer` (see module docstring) plus:
      - an accuracy@ratio curve, bucketed by measured compression ratio
      - the ACON-style contrastive raw-passes/compressed-fails KPI
      - critical-token needle-survival (identical mechanism to the smoke
        gate's `critical_token_survival_rate`, just over a larger, more
        varied fixture set)

    Promotion out of `--experimental` (roadmap.md Task 9) requires each
    transform to *individually* clear the D-005 draft Balanced thresholds -
    a transform-blended average can't be used to promote both together,
    since one transform passing cleanly could otherwise mask the other
    failing. `per_transform` in the artifact carries that breakdown; the
    top-level `gate` is "pass" only when every evaluated transform clears
    individually.

    `diff_compaction` additionally has two behaviorally distinct forms
    selected by `task_scope` (see `pipeline.rs`'s
    `keep_line_bodies = policy.task_scope != TaskScope::ChangeSummary` and
    F-013 in roadmap.md): the default, body-preserving form (any task scope
    other than `ChangeSummary`) and the header-only form (`TaskScope::
    ChangeSummary` only, an explicit opt-in to a lossier tradeoff). Blending
    both into one `per_transform["diff_compaction"]` average conflates "is
    the shipped-by-default behavior safe?" with "is the opt-in lossiest
    behavior safe?" - two different questions with two different answers.
    `per_variant` breaks every transform's fixtures down further by
    `task_scope` so each can be judged on its own; this is the mechanism a
    caller should use to decide whether to promote just one form of a
    transform rather than treating `per_transform`'s blended number as the
    only signal.

    Returns the process exit code (0 for gate=pass, 1 for gate=fail).
    """
    fixtures = _load_fixtures_from(FULL_LOSSY_FIXTURES_DIR, "flp_*.json")

    fixture_hashes = []
    all_quality = []
    all_contrastive = []
    all_critical = []
    transform_ids = set()

    # transform_id -> {"quality": [...], "contrastive": [...], "critical": [...]}
    per_transform_scores: dict = {}
    # (transform_id, task_scope) -> {"quality": [...], "contrastive": [...], "critical": [...]}
    per_variant_scores: dict = {}
    # (transform_id, bucket) -> {"ratios": [...], "quality": [...]}
    per_bucket_scores: dict = {}

    for path, raw, data in fixtures:
        digest = hashlib.sha256(raw).hexdigest()
        fixture_hashes.append(f"sha256:{digest}")

        transform_id = data["transform_id"]
        task_scope = data["task_scope"]
        original = data["original"]
        compressed = data["compressed"]
        critical_tokens = data["critical_tokens"]

        quality = _quality_retention_proxy(original, compressed)
        contrastive = _contrastive_failure_proxy(original, compressed, critical_tokens)
        critical = _critical_token_survival(compressed, critical_tokens)
        ratio = _compression_ratio(original, compressed)
        bucket = _ratio_bucket(ratio)

        all_quality.append(quality)
        all_contrastive.append(contrastive)
        all_critical.append(critical)
        transform_ids.add(transform_id)

        bucket_key = per_transform_scores.setdefault(
            transform_id, {"quality": [], "contrastive": [], "critical": []}
        )
        bucket_key["quality"].append(quality)
        bucket_key["contrastive"].append(contrastive)
        bucket_key["critical"].append(critical)

        variant_key = per_variant_scores.setdefault(
            (transform_id, task_scope), {"quality": [], "contrastive": [], "critical": []}
        )
        variant_key["quality"].append(quality)
        variant_key["contrastive"].append(contrastive)
        variant_key["critical"].append(critical)

        curve_key = per_bucket_scores.setdefault((transform_id, bucket), {"ratios": [], "quality": []})
        curve_key["ratios"].append(ratio)
        curve_key["quality"].append(quality)

    quality_retention = statistics.mean(all_quality) if all_quality else 0.0
    contrastive_failure_rate = statistics.mean(all_contrastive) if all_contrastive else 0.0
    critical_token_survival_rate = statistics.mean(all_critical) if all_critical else 0.0

    per_transform = {}
    for transform_id in sorted(transform_ids):
        scores = per_transform_scores[transform_id]
        t_quality = statistics.mean(scores["quality"])
        t_contrastive = statistics.mean(scores["contrastive"])
        t_critical = statistics.mean(scores["critical"])
        t_pass = (
            t_quality >= QUALITY_RETENTION_MIN
            and t_contrastive <= CONTRASTIVE_FAILURE_RATE_MAX
            and t_critical >= CRITICAL_TOKEN_SURVIVAL_RATE_MIN
        )
        per_transform[transform_id] = {
            "gate": "pass" if t_pass else "fail",
            "quality_retention": t_quality,
            "contrastive_failure_rate": t_contrastive,
            "critical_token_survival_rate": t_critical,
            "fixture_count": len(scores["quality"]),
        }

    per_variant = []
    for transform_id, task_scope in sorted(per_variant_scores):
        scores = per_variant_scores[(transform_id, task_scope)]
        v_quality = statistics.mean(scores["quality"])
        v_contrastive = statistics.mean(scores["contrastive"])
        v_critical = statistics.mean(scores["critical"])
        v_pass = (
            v_quality >= QUALITY_RETENTION_MIN
            and v_contrastive <= CONTRASTIVE_FAILURE_RATE_MAX
            and v_critical >= CRITICAL_TOKEN_SURVIVAL_RATE_MIN
        )
        per_variant.append({
            "transform_id": transform_id,
            "task_scope": task_scope,
            "gate": "pass" if v_pass else "fail",
            "quality_retention": v_quality,
            "contrastive_failure_rate": v_contrastive,
            "critical_token_survival_rate": v_critical,
            "fixture_count": len(scores["quality"]),
        })

    accuracy_at_ratio = []
    bucket_order = [name for name, _lo, _hi in RATIO_BUCKETS]
    for transform_id in sorted(transform_ids):
        for bucket in bucket_order:
            key = (transform_id, bucket)
            if key not in per_bucket_scores:
                continue
            scores = per_bucket_scores[key]
            accuracy_at_ratio.append({
                "transform_id": transform_id,
                "ratio_bucket": bucket,
                "mean_ratio": statistics.mean(scores["ratios"]),
                "quality_retention": statistics.mean(scores["quality"]),
                "fixture_count": len(scores["ratios"]),
            })

    # Promotion is only meaningful if every candidate transform this profile
    # is scoped to (FULL_LOSSY_TRANSFORMS) was actually exercised.
    all_expected_evaluated = all(t in transform_ids for t in FULL_LOSSY_TRANSFORMS)
    gate_pass = all_expected_evaluated and all(
        entry["gate"] == "pass" for entry in per_transform.values()
    )

    artifact = {
        "profile": profile,
        "gate": "pass" if gate_pass else "fail",
        "model_version": FULL_LOSSY_MODEL_VERSION,
        "scorer": "deterministic-lexical-overlap-bootstrap",
        "fixture_hashes": fixture_hashes,
        "total_cost_usd": 0.0,
        "quality_retention": quality_retention,
        "contrastive_failure_rate": contrastive_failure_rate,
        "critical_token_survival_rate": critical_token_survival_rate,
        "transforms_evaluated": sorted(transform_ids),
        "per_transform": per_transform,
        "per_variant": per_variant,
        "accuracy_at_ratio": accuracy_at_ratio,
    }

    print(json.dumps(artifact, indent=2))

    return 0 if gate_pass else 1


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="run_fidelity.py",
        description=(
            "Tokenfold fidelity smoke-gate harness (deterministic bootstrap; "
            "see module docstring for what it is and is not)."
        ),
    )
    parser.add_argument(
        "--gate",
        action="store_true",
        help=(
            "Actually run the fidelity gate. Required to run an evaluation; "
            "the gate is meant to be an explicit, blocking action rather than "
            "a side effect of invoking this script. Without this flag, usage "
            "is printed and the script exits 0 without evaluating anything."
        ),
    )
    parser.add_argument(
        "--profile",
        type=str,
        default=None,
        help=(
            "Gate profile to run: 'smoke-first-consumer' or "
            "'full-lossy-promotion' (see ENGINEERING.md 'Fidelity Tests' for "
            "the full profile table)."
        ),
    )
    return parser


def main(argv=None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)

    if not args.gate:
        # The gate is an explicit, blocking action - invoking this script
        # without --gate is a no-op that just shows usage.
        parser.print_help()
        return 0

    if args.profile not in IMPLEMENTED_PROFILES:
        print(
            f"error: unknown or unimplemented profile {args.profile!r}. "
            f"Implemented profiles: {', '.join(IMPLEMENTED_PROFILES)}",
            file=sys.stderr,
        )
        return 2

    if args.profile == "full-lossy-promotion":
        return run_full_lossy_promotion_gate(args.profile)

    return run_gate(args.profile)


if __name__ == "__main__":
    sys.exit(main())
