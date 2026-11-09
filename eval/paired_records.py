"""Pure legacy paired-record helpers; execution remains in run_paired."""
from __future__ import annotations

from collections import defaultdict
import hashlib
import json
from pathlib import Path
import random

PAIRED_SCHEMA_VERSION = "1.0"


ARMS = ("raw", "candidate")


OUTCOMES = ("success", "failure", "invalid", "unknown")


_BOOTSTRAP_RESAMPLES = 2000


_BOOTSTRAP_SEED = 20260930


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
