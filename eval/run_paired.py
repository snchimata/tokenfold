#!/usr/bin/env python3
"""Paired raw-vs-candidate run records and their aggregation (offline, stdlib only).

WHAT THIS IS
------------
The scoring half of the quality/economics runner: a *paired* record schema, a
fail-closed reader, and the aggregation the contract fixes in advance -- all
four paired outcomes, the conditional contrastive regression rate (CFR), the
absolute success delta, and an uncertainty bound.

    CFR = #{raw success AND candidate failure} / #{raw success}

No raw successes means CFR is *unavailable* (`None`, never `0.0`): the metric
has no denominator, and reporting zero there would invent evidence.

WHAT THIS IS NOT
----------------
This module does not measure downstream task quality by itself. It only
*aggregates* outcomes a runner produced, and its offline driver uses a
deterministic dummy model (a scripted reader, not an LLM) to prove the
mechanism end to end. A green run here certifies the record contract and the
arithmetic -- never that a transform is safe to promote. Live budgeted
execution and per-regression attribution stay outside this file.

The measurement fields a real runner copies in are the EP-02 per-attempt
`MeasurementEvent`; they are attached verbatim under `measurement` and are
optional here, so an offline record needs no provider round trip.

USAGE
-----
    python eval/run_paired.py --records runs.jsonl
    python eval/run_paired.py --records runs.jsonl --gate --max-cfr 0.005
    python eval/run_paired.py --run-offline --tasks-dir eval/tasks/paired

DEPENDENCIES
------------
Python standard library only, plus the existing `run_baselines` harness for
token counting, the isolated retrieval config and the `tokenfold` CLI
subprocesses (`--run-offline` only).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import random
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import run_baselines as rb  # noqa: E402

PAIRED_SCHEMA_VERSION = "1.0"
ARMS = ("raw", "candidate")
OUTCOMES = ("success", "failure", "invalid", "unknown")
_BOOTSTRAP_RESAMPLES = 2000
_BOOTSTRAP_SEED = 20260930

SCRIPT_DIR = Path(__file__).resolve().parent
DEFAULT_TASKS_DIR = SCRIPT_DIR / "tasks" / "paired"
# ---------------------------------------------------------------------------
# record schema
# ---------------------------------------------------------------------------

_STRING_FIELDS = ("task_id", "environment_snapshot", "arm", "model", "outcome")
_REQUIRED_FIELDS = (
    "schema_version",
    "run_id",
    "task_id",
    "environment_snapshot",
    "arm",
    "model",
    "outcome",
    "evidence",
)


def snapshot_hash(payload: str) -> str:
    """`sha256:<hex>` of the exact environment bytes both arms must start from.

    A pair is only a pair when both arms hashed the *same* starting state, so
    the digest is part of the record and part of the pairing key -- not a
    decoration added at report time.
    """
    return "sha256:" + hashlib.sha256(payload.encode("utf-8")).hexdigest()


def validate_record(record: dict, where: str = "record") -> None:
    """Reject anything that could silently distort an aggregate. Raises ValueError.

    Fail-closed on purpose: a dropped or mislabelled record is an evaluation
    defect, so it must stop the run rather than shrink a denominator.
    """
    if not isinstance(record, dict):
        raise ValueError(f"{where}: must be a JSON object")
    for field in _REQUIRED_FIELDS:
        if field not in record:
            raise ValueError(f"{where}: missing required field {field!r}")
    version = record["schema_version"]
    if version != PAIRED_SCHEMA_VERSION:
        raise ValueError(
            f"{where}: unsupported schema_version {version!r} "
            f"(this harness reads {PAIRED_SCHEMA_VERSION})"
        )
    for field in _STRING_FIELDS:
        if not isinstance(record[field], str) or not record[field].strip():
            raise ValueError(f"{where}: {field} must be a non-empty string")
    if record["arm"] not in ARMS:
        raise ValueError(f"{where}: arm must be one of {ARMS}, got {record['arm']!r}")
    if record["outcome"] not in OUTCOMES:
        raise ValueError(f"{where}: outcome must be one of {OUTCOMES}, got {record['outcome']!r}")
    if not record["environment_snapshot"].startswith("sha256:"):
        raise ValueError(f"{where}: environment_snapshot must be a 'sha256:' digest")
    for field in ("seed", "attempt"):
        if not isinstance(record[field], int) or isinstance(record[field], bool):
            raise ValueError(f"{where}: {field} must be an integer")
    if record["attempt"] < 1:
        raise ValueError(f"{where}: attempt is 1-based, got {record['attempt']}")
    evidence = record["evidence"]
    if not isinstance(evidence, list) or not evidence or any(
        not isinstance(item, str) or not item.strip() for item in evidence
    ):
        raise ValueError(f"{where}: evidence must be a non-empty list of strings")
    measurement = record.get("measurement")
    if measurement is not None and not isinstance(measurement, dict):
        raise ValueError(f"{where}: measurement must be an object when present")
    if record["outcome"] == "invalid" and not record.get("invalid_reason"):
        raise ValueError(
            f"{where}: an invalid environment run must carry invalid_reason; an invalid run "
            "is reported, never relabelled a model failure"
        )


def load_records(path: Path) -> list[dict]:
    """Read newline-delimited paired records, validating each one."""
    records = []
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        if not line.strip():
            continue
        record = json.loads(line)
        validate_record(record, f"{path}:{number}")
        records.append(record)
    if not records:
        raise ValueError(f"{path}: no paired records")
    return records

# ---------------------------------------------------------------------------
# pairing + aggregation
# ---------------------------------------------------------------------------


def _pair_key(record: dict) -> tuple:
    return (
        record["task_id"],
        record["environment_snapshot"],
        record["seed"],
        record["attempt"],
    )


def pair_records(records: list[dict]) -> tuple[list[dict], list[dict], list[dict]]:
    """Group records into arm pairs. Returns `(pairs, unpaired, invalid)`.

    - A pair needs exactly one `raw` and one `candidate` record for the same
      task/snapshot/seed/attempt. Nothing is inferred and nothing is dropped: an
      arm without its partner is returned in `unpaired`, and an
      `invalid`/`unknown` outcome is returned in `invalid` rather than counted
      as a model failure.
    - A duplicated (key, arm) is a runner bug -- it would double-weight one
      observation -- so it is reported as unpaired rather than silently merged.
    """
    buckets: dict[tuple, dict[str, dict]] = {}
    unpaired: list[dict] = []
    invalid: list[dict] = []
    for record in records:
        if record["outcome"] in ("invalid", "unknown"):
            invalid.append(record)
            continue
        bucket = buckets.setdefault(_pair_key(record), {})
        arm = record["arm"]
        if arm in bucket:
            unpaired.append(
                {
                    "reason": "duplicate_arm",
                    "task_id": record["task_id"],
                    "arm": arm,
                    "seed": record["seed"],
                    "attempt": record["attempt"],
                }
            )
            continue
        bucket[arm] = record

    pairs = []
    for key in sorted(buckets):
        bucket = buckets[key]
        if set(bucket) != set(ARMS):
            unpaired.append(
                {
                    "reason": "missing_arm",
                    "task_id": key[0],
                    "arm": sorted(set(ARMS) - set(bucket))[0],
                    "seed": key[2],
                    "attempt": key[3],
                }
            )
            continue
        raw, candidate = bucket["raw"], bucket["candidate"]
        pairs.append(
            {
                "task_id": key[0],
                "environment_snapshot": key[1],
                "seed": key[2],
                "attempt": key[3],
                "raw": raw["outcome"] == "success",
                "candidate": candidate["outcome"] == "success",
            }
        )
    return pairs, unpaired, invalid


def _conditional_failure_rate(pairs: list[dict]) -> float | None:
    """CFR over raw successes. `None` (unavailable) when nothing succeeded raw."""
    raw_successes = sum(1 for p in pairs if p["raw"])
    if raw_successes == 0:
        return None
    regressions = sum(1 for p in pairs if p["raw"] and not p["candidate"])
    return round(regressions / raw_successes, 4)


def _success_delta(pairs: list[dict]) -> float | None:
    if not pairs:
        return None
    raw_rate = sum(1 for p in pairs if p["raw"]) / len(pairs)
    candidate_rate = sum(1 for p in pairs if p["candidate"]) / len(pairs)
    return round(candidate_rate - raw_rate, 4)

def _cluster_bootstrap_ci(pairs: list[dict], stat) -> list[float] | None:
    """Percentile interval from resampling *tasks*, not records.

    Repeating one task three times is three observations of one thing; treating
    them as independent would report an interval narrower than the data can
    support. Resampling whole tasks keeps the cluster as the resampling unit.
    Seeded, so the number in the artifact is reproducible.
    """
    by_task: dict[str, list[dict]] = {}
    for pair in pairs:
        by_task.setdefault(pair["task_id"], []).append(pair)
    tasks = sorted(by_task)
    if len(tasks) < 2:
        # One cluster has no between-task variance to estimate.
        return None
    rng = random.Random(_BOOTSTRAP_SEED)
    values = []
    for _ in range(_BOOTSTRAP_RESAMPLES):
        sample = [tasks[rng.randrange(len(tasks))] for _ in tasks]
        value = stat([pair for task in sample for pair in by_task[task]])
        if value is not None:
            values.append(value)
    if len(values) < 2:
        return None
    values.sort()
    low = values[int(0.025 * len(values))]
    high = values[min(len(values) - 1, int(0.975 * len(values)))]
    return [round(low, 4), round(high, 4)]


def aggregate(pairs: list[dict], unpaired: list[dict], invalid: list[dict]) -> dict:
    """The predeclared report. Every count, denominator and bound is published.

    All four paired outcomes are reported side by side on purpose: a candidate
    that fixes tasks the raw arm failed is real movement, and a summary showing
    only the regression count would hide exactly the offsetting gain the
    contract forbids cancelling out.
    """
    cells = {
        "raw_success_candidate_success": sum(1 for p in pairs if p["raw"] and p["candidate"]),
        "raw_success_candidate_failure": sum(1 for p in pairs if p["raw"] and not p["candidate"]),
        "raw_failure_candidate_success": sum(1 for p in pairs if not p["raw"] and p["candidate"]),
        "raw_failure_candidate_failure": sum(1 for p in pairs if not p["raw"] and not p["candidate"]),
    }
    raw_successes = cells["raw_success_candidate_success"] + cells["raw_success_candidate_failure"]
    candidate_successes = cells["raw_success_candidate_success"] + cells["raw_failure_candidate_success"]
    repeats = sum(1 for pair in pairs if sum(1 for q in pairs if q["task_id"] == pair["task_id"]) > 1)
    return {
        "paired_count": len(pairs),
        "task_count": len({p["task_id"] for p in pairs}),
        "repeated_attempt_count": repeats,
        "paired_outcomes": cells,
        "raw_successes": raw_successes,
        "candidate_successes": candidate_successes,
        "raw_success_rate": round(raw_successes / len(pairs), 4) if pairs else None,
        "candidate_success_rate": round(candidate_successes / len(pairs), 4) if pairs else None,
        "conditional_failure_rate": _conditional_failure_rate(pairs),
        "conditional_failure_rate_ci95": _cluster_bootstrap_ci(pairs, _conditional_failure_rate),
        "success_delta": _success_delta(pairs),
        "success_delta_ci95": _cluster_bootstrap_ci(pairs, _success_delta),
        "unpaired": unpaired,
        "invalid_count": len(invalid),
        "invalid": [
            {"task_id": r["task_id"], "arm": r["arm"], "reason": r.get("invalid_reason")}
            for r in invalid
        ],
    }


# ---------------------------------------------------------------------------
# resettable sandbox + deterministic dummy model
# ---------------------------------------------------------------------------


class Sandbox:
    """A resettable, per-attempt copy of a task's mutable environment.

    Both arms get their own instance created from the same snapshot bytes, so
    "identical starting state" is a property of the code path rather than a
    claim in the report. Writes are confined to the sandbox root: an escaping or
    absolute path is refused, so an evaluation run cannot reach a production
    path even if a future runner asks it to.
    """

    def __init__(self, snapshot: str):
        self._snapshot = snapshot
        self._root = Path(tempfile.mkdtemp(prefix="tokenfold_paired_"))
        self.reset()

    @property
    def root(self) -> Path:
        return self._root

    def reset(self) -> str:
        """Restore the pristine snapshot and return its digest."""
        shutil.rmtree(self._root, ignore_errors=True)
        self._root.mkdir(parents=True)
        (self._root / "environment.json").write_text(self._snapshot, encoding="utf-8")
        return snapshot_hash(self.read("environment.json"))

    def read(self, relative: str) -> str:
        return (self._root / self._confine(relative)).read_text(encoding="utf-8")

    def write(self, relative: str, value: str) -> Path:
        target = self._root / self._confine(relative)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(value, encoding="utf-8")
        return target

    def _confine(self, relative: str) -> Path:
        candidate = Path(relative)
        # `.anchor` catches `C:\...` and also the driveless rooted forms Windows
        # accepts (`\etc\passwd`, which is absolute on the current drive even
        # though `Path.is_absolute()` reports False for it).
        if candidate.anchor or ".." in candidate.parts:
            raise ValueError(f"sandbox refuses a path outside its root: {relative!r}")
        return candidate

    def close(self) -> None:
        shutil.rmtree(self._root, ignore_errors=True)


def dummy_model_answer(observation: str, gold_answer: str) -> str:
    """A deterministic stand-in for the downstream model, and nothing more.

    It answers from the text it can actually see: the gold answer when the
    observation still carries it (whitespace-insensitively, as the existing
    scorer does), otherwise a fixed non-answer. That makes "the candidate arm
    failed" a statement about what survived compression -- the property under
    test -- and says nothing about any real model's reasoning.
    """
    if rb._ws_strip(gold_answer) in rb._ws_strip(rb._logical_text(observation)):
        return gold_answer
    return "<unavailable>"

# ---------------------------------------------------------------------------
# offline fixture driver
# ---------------------------------------------------------------------------


def load_tasks(tasks_dir: Path) -> list[dict]:
    """Load paired-task fixtures, failing closed on anything non-discriminating."""
    tasks = []
    seen = set()
    for path in sorted(tasks_dir.glob("*.json")):
        task = json.loads(path.read_text(encoding="utf-8"))
        for field in ("id", "family", "tier", "source", "query", "gold_answer", "critical_atoms"):
            if field not in task:
                raise ValueError(f"{path}: missing required field {field!r}")
        if task["id"] in seen:
            raise ValueError(f"{path}: duplicate task id {task['id']!r}")
        seen.add(task["id"])
        for atom in task["critical_atoms"]:
            if atom not in task["source"]:
                raise ValueError(f"{path}: critical atom {atom!r} is not grounded in source")
        if task["gold_answer"] not in task["source"]:
            raise ValueError(f"{path}: gold_answer must occur in source")
        tasks.append(task)
    if not tasks:
        raise ValueError(f"no paired-task fixtures found in {tasks_dir}")
    return tasks


def recoverable_text(payload: str) -> str:
    """Payload text plus every dropped item the host could retrieve.

    A `$tf_ref` marker is a *handle*: the content still exists in the retrieval
    store. Judging the candidate arm on marker text alone would score a working
    recovery path as data loss, so retrieval is resolved here; a retrieval that
    fails stays absent rather than being faked.
    """
    if "$tf_ref" not in payload:
        return payload
    try:
        hashes = rb._find_tf_ref_hashes(json.loads(payload))
    except ValueError:
        return payload
    recovered = []
    for digest in hashes:
        try:
            proc = subprocess.run(
                [
                    rb._TOKENFOLD_BIN,
                    "--config",
                    rb.isolated_retrieval_config(),
                    "retrieve",
                    digest,
                    "--retrieval-namespace",
                    "default",
                ],
                capture_output=True,
                timeout=30,
            )
        except (OSError, subprocess.SubprocessError):
            continue
        if proc.returncode == 0 and proc.stdout:
            recovered.append(proc.stdout.decode("utf-8", errors="replace"))
    return payload + "\n" + "\n".join(recovered)


def structural_check(task: dict, payload: str) -> dict:
    """Structural correctness, deliberately separate from downstream success.

    Structural = the payload is still well-formed JSON and still carries the
    atoms the task declares (inline or through a resolvable retrieval handle).
    Downstream = the dummy model could still answer. A payload can be perfectly
    well-formed and already useless, so conflating the two is how a broken
    transform gets reported as a quality regression -- or the reverse.
    """
    try:
        json.loads(payload)
        well_formed = True
    except ValueError:
        well_formed = False
    haystack = rb._ws_strip(recoverable_text(payload))
    surviving = [a for a in task["critical_atoms"] if rb._ws_strip(a) in haystack]
    return {
        "envelope_well_formed": well_formed,
        "critical_atoms": len(task["critical_atoms"]),
        "critical_atoms_surviving": len(surviving),
        "structure_ok": well_formed and len(surviving) == len(task["critical_atoms"]),
    }

def run_offline(tasks_dir: Path, ratios: list[float]) -> tuple[list[dict], list[dict]]:
    """Run every fixture through both arms. Returns `(records, rows)`.

    `raw` is the untouched source; `candidate` is the tokenfold CLI output at
    the requested ratio. Each arm runs in its own sandbox reset from the same
    snapshot bytes, and the two start digests are asserted equal rather than
    assumed -- that assertion is the whole point of the sandbox.
    """
    if rb._TOKENFOLD_BIN is None:
        raise ValueError(
            "tokenfold binary not found; --run-offline drives the real CLI "
            "(build it and set TOKENFOLD_BIN)"
        )
    records: list[dict] = []
    rows: list[dict] = []
    for task in load_tasks(tasks_dir):
        source = task["source"]
        digest = snapshot_hash(source)
        raw_tokens = rb.count_tokens(source)
        for ratio in ratios:
            budget = round(raw_tokens * ratio)
            for label, payload in (
                ("lossless", rb.compress_tokenfold(source, budget)),
                ("lossy", rb.compress_tokenfold_lossy(source, budget)),
            ):
                if payload is None:
                    continue
                raw_sandbox, candidate_sandbox = Sandbox(source), Sandbox(source)
                try:
                    starts = (raw_sandbox.reset(), candidate_sandbox.reset())
                finally:
                    raw_sandbox.close()
                    candidate_sandbox.close()
                assert starts == (digest, digest), f"{task['id']}: arms started from different snapshots"

                answer = dummy_model_answer(recoverable_text(payload), task["gold_answer"])
                succeeded = answer == task["gold_answer"]
                rows.append(
                    {
                        "task": task["id"],
                        "family": task["family"],
                        "arm": label,
                        "target_ratio": ratio,
                        "raw_tokens": raw_tokens,
                        "candidate_tokens": rb.count_tokens(payload),
                        **structural_check(task, payload),
                        "downstream_success": succeeded,
                    }
                )
                for arm, observation, ok in (
                    ("raw", source, True),
                    ("candidate", answer, succeeded),
                ):
                    records.append(
                        {
                            "schema_version": PAIRED_SCHEMA_VERSION,
                            "run_id": "offline-dummy-model",
                            "task_id": f"{task['id']}@{label}@{ratio}",
                            "environment_snapshot": digest,
                            "arm": arm,
                            "model": f"dummy-scorer-v0+{label}@{ratio}",
                            "policy_revision": f"{label}@{ratio}",
                            "seed": 0,
                            "attempt": 1,
                            "outcome": "success" if ok else "failure",
                            "evidence": [f"eval/tasks/paired/{task['id']}.json"],
                            "measurement": {
                                "local_before_tokens": raw_tokens,
                                "local_after_tokens": rb.count_tokens(observation),
                                "provider_usage": None,
                            },
                        }
                    )
    return records, rows

# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def gate_failures(report: dict, max_cfr: float) -> list[str]:
    """Assert the predeclared promotion thresholds. Returns failure strings."""
    failures = []
    if report["paired_count"] == 0:
        failures.append("no valid pairs: every record was unpaired or invalid")
    if report["unpaired"]:
        failures.append(f"{len(report['unpaired'])} unpaired arm(s): {report['unpaired'][:3]}")
    if report["invalid_count"]:
        failures.append(
            f"{report['invalid_count']} invalid/unknown environment run(s) must be resolved, "
            "not aggregated"
        )
    cfr = report["conditional_failure_rate"]
    if cfr is None:
        # Unavailable, not zero. With no raw successes the ceiling has no
        # denominator here, so it cannot be cleared.
        failures.append("conditional_failure_rate is unavailable (no raw successes)")
    elif cfr > max_cfr:
        failures.append(f"conditional_failure_rate {cfr} > max {max_cfr}")
    return failures


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Paired raw-vs-candidate run aggregation")
    parser.add_argument("--records", help="newline-delimited paired-run records to aggregate")
    parser.add_argument(
        "--tasks-dir",
        default=str(DEFAULT_TASKS_DIR),
        help="offline paired-task fixture directory (used with --run-offline)",
    )
    parser.add_argument(
        "--run-offline",
        action="store_true",
        help="drive the real tokenfold CLI over --tasks-dir and aggregate the result",
    )
    parser.add_argument(
        "--ratios",
        default="0.5,0.1",
        help="comma-separated target retention ratios; 0.1 is where lossy pruning "
        "actually engages (at 0.5 the walk keeps every candidate, which is real "
        "measured behaviour but discriminates nothing)",
    )
    parser.add_argument("--gate", action="store_true", help="assert thresholds; exit non-zero on failure")
    parser.add_argument(
        "--max-cfr",
        type=float,
        default=0.005,
        help="predeclared conditional-failure ceiling for --gate (default 0.005)",
    )
    return parser


def main(argv=None) -> int:
    args = build_parser().parse_args(argv)
    try:
        if args.records:
            records, rows = load_records(Path(args.records)), None
        elif args.run_offline:
            ratios = [float(x) for x in args.ratios.split(",") if x.strip()]
            if not ratios or any(not math.isfinite(r) or r <= 0.0 or r > 1.0 for r in ratios):
                raise ValueError("ratios must be comma-separated numbers in (0, 1]")
            records, rows = run_offline(Path(args.tasks_dir), ratios)
        else:
            print("one of --records or --run-offline is required", file=sys.stderr)
            return 2
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"paired run failed: {error}", file=sys.stderr)
        return 2

    pairs, unpaired, invalid = pair_records(records)
    report = aggregate(pairs, unpaired, invalid)
    report["schema_version"] = PAIRED_SCHEMA_VERSION
    if rows is not None:
        report["offline_rows"] = rows
    if args.gate:
        report["failures"] = gate_failures(report, args.max_cfr)
        report["gate"] = "pass" if not report["failures"] else "fail"
    print(json.dumps(report, indent=2))
    return 1 if args.gate and report["failures"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
