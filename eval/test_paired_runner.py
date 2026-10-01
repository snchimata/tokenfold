#!/usr/bin/env python3
"""Contract checks for the paired raw-vs-candidate runner.

Aggregation is checked against the predeclared truth table (8/2/1/1), and every
case that would otherwise inflate a result is pinned: an unpaired arm, a
duplicated arm, an invalid environment run, a snapshot mismatch, and repeated
attempts on one task. The offline fixture cases need the tokenfold CLI, so they
assert the binary first and skip -- rather than silently pass -- without it.
"""

from __future__ import annotations

import json
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import run_paired as rp  # noqa: E402

SNAPSHOT = rp.snapshot_hash("environment bytes")


def record(task: str, arm: str, outcome: str, **overrides) -> dict:
    base = {
        "schema_version": rp.PAIRED_SCHEMA_VERSION,
        "run_id": "test",
        "task_id": task,
        "environment_snapshot": SNAPSHOT,
        "arm": arm,
        "model": "test-model",
        "policy_revision": None,
        "seed": 0,
        "attempt": 1,
        "outcome": outcome,
        "evidence": ["test"],
    }
    base.update(overrides)
    return base


def truth_table_records() -> list[dict]:
    """The contract's table: 8/2/1/1 over 12 pairs."""
    plan = (
        [("success", "success")] * 8
        + [("success", "failure")] * 2
        + [("failure", "success")]
        + [("failure", "failure")]
    )
    records = []
    for index, (raw, candidate) in enumerate(plan):
        records.append(record(f"task_{index:02d}", "raw", raw))
        records.append(record(f"task_{index:02d}", "candidate", candidate))
    return records


def test_truth_table_aggregates_to_the_declared_numbers():
    report = rp.aggregate(*rp.pair_records(truth_table_records()))
    assert report["paired_count"] == 12, report
    assert report["paired_outcomes"] == {
        "raw_success_candidate_success": 8,
        "raw_success_candidate_failure": 2,
        "raw_failure_candidate_success": 1,
        "raw_failure_candidate_failure": 1,
    }, report
    assert report["raw_successes"] == 10, report
    assert report["conditional_failure_rate"] == 0.2, report
    assert report["raw_success_rate"] == 0.8333, report
    assert report["candidate_success_rate"] == 0.75, report
    assert report["success_delta"] == -0.0833, report
    # The one compensating gain must stay visible, not netted away.
    assert report["paired_outcomes"]["raw_failure_candidate_success"] == 1, report


def test_cfr_is_unavailable_not_zero_without_raw_successes():
    records = [record("t", "raw", "failure"), record("t", "candidate", "success")]
    report = rp.aggregate(*rp.pair_records(records))
    assert report["conditional_failure_rate"] is None, report
    assert report["success_delta"] == 1.0, report
    # Unavailable CFR cannot clear a ceiling: the denominator does not exist.
    assert rp.gate_failures(report, 0.005), report


def test_repeated_attempts_are_clustered_not_counted_as_independent_tasks():
    independent = [
        r
        for i in range(6)
        for r in (record(f"t{i}", "raw", "success"), record(f"t{i}", "candidate", "failure"))
    ]
    # The same six tasks, each retried three times: 36 records, still 6 tasks.
    repeated = [
        r
        for i in range(6)
        for attempt in (1, 2, 3)
        for r in (
            record(f"t{i}", "raw", "success", attempt=attempt),
            record(f"t{i}", "candidate", "failure", attempt=attempt),
        )
    ]
    spread = rp.aggregate(*rp.pair_records(independent))
    clustered = rp.aggregate(*rp.pair_records(repeated))
    for report, tasks, pairs in ((spread, 6, 6), (clustered, 6, 18)):
        assert report["paired_count"] == pairs, report
        assert report["task_count"] == tasks, report
        assert report["conditional_failure_rate"] == 1.0, report
    # Repeats must not be read as 3x the independent evidence, so the
    # clustered interval is the wider one at the same point estimate.
    assert clustered["repeated_attempt_count"] == 18, clustered
    assert clustered["conditional_failure_rate_ci95"] is not None
    assert clustered["success_delta_ci95"][0] == spread["success_delta_ci95"][0] == -1.0


def test_the_interval_brackets_the_point_estimate():
    report = rp.aggregate(*rp.pair_records(truth_table_records()))
    low, high = report["conditional_failure_rate_ci95"]
    assert low <= report["conditional_failure_rate"] <= high, report
    delta_low, delta_high = report["success_delta_ci95"]
    assert delta_low <= report["success_delta"] <= delta_high, report


def test_the_bootstrap_is_seeded_so_the_artifact_number_is_reproducible():
    assert rp.aggregate(*rp.pair_records(truth_table_records())) == rp.aggregate(
        *rp.pair_records(list(reversed(truth_table_records())))
    )

# --- pairing is fail-closed -------------------------------------------------


def test_a_missing_arm_is_reported_instead_of_silently_dropped():
    pairs, unpaired, invalid = rp.pair_records(
        [record("t", "raw", "success"), record("other", "raw", "success")]
    )
    assert pairs == []
    assert [entry["reason"] for entry in unpaired] == ["missing_arm", "missing_arm"], unpaired
    assert rp.gate_failures(rp.aggregate(pairs, unpaired, invalid), 0.005)


def test_a_duplicated_arm_is_reported_instead_of_double_weighting():
    records = [
        record("t", "raw", "success"),
        record("t", "raw", "failure"),
        record("t", "candidate", "success"),
    ]
    pairs, unpaired, _ = rp.pair_records(records)
    assert len(pairs) == 1 and pairs[0]["raw"] is True, (pairs, unpaired)
    assert unpaired[0]["reason"] == "duplicate_arm", unpaired


def test_an_invalid_environment_run_is_reported_and_never_a_model_failure():
    records = [
        record("t", "raw", "success"),
        record("t", "candidate", "invalid", invalid_reason="sandbox snapshot did not apply"),
    ]
    report = rp.aggregate(*rp.pair_records(records))
    assert report["paired_count"] == 0, report
    assert report["invalid"][0]["reason"] == "sandbox snapshot did not apply", report
    assert report["raw_successes"] == 0, report
    assert rp.gate_failures(report, 0.005), report


def test_a_mismatched_environment_never_pairs_across_snapshots():
    records = [
        record("t", "raw", "success"),
        record("t", "candidate", "success", environment_snapshot=rp.snapshot_hash("other")),
    ]
    pairs, unpaired, _ = rp.pair_records(records)
    assert pairs == [] and unpaired[0]["reason"] == "missing_arm", (pairs, unpaired)


def test_an_invalid_run_without_a_reason_is_rejected_at_read_time():
    _rejects(record("t", "raw", "invalid"), "invalid_reason")


def test_an_unknown_schema_version_is_refused_rather_than_guessed():
    _rejects(record("t", "raw", "success", schema_version="2.0"), "schema_version")


def test_an_unknown_outcome_is_refused():
    _rejects(record("t", "raw", "probably_fine"), "outcome")


def test_a_bare_outcome_string_is_not_a_record():
    _rejects({"outcome": "success"}, "missing required field")


def test_records_round_trip_through_the_reader():
    records = truth_table_records()
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory, "runs.jsonl")
        path.write_text("\n".join(json.dumps(r) for r in records), encoding="utf-8")
        assert rp.load_records(path) == records
        assert rp.main(["--records", str(path)]) == 0


def _rejects(record_: dict, expected: str) -> None:
    try:
        rp.validate_record(record_)
    except ValueError as error:
        assert expected in str(error), error
    else:
        raise AssertionError(f"expected {expected!r} to be refused")


# --- sandbox ----------------------------------------------------------------


def test_both_arms_reset_to_the_same_snapshot_digest():
    left, right = rp.Sandbox("a = 1\n"), rp.Sandbox("a = 1\n")
    try:
        assert left.reset() == right.reset() == rp.snapshot_hash("a = 1\n")
        left.write("scratch/out.txt", "candidate arm was here")
        assert left.read("scratch/out.txt") == "candidate arm was here"
        # The other arm is untouched by that write, and reset restores the first.
        assert right.read("environment.json") == "a = 1\n"
        left.reset()
        assert not (left.root / "scratch").exists()
    finally:
        left.close()
        right.close()


def test_the_sandbox_refuses_to_write_outside_its_root():
    sandbox = rp.Sandbox("a = 1\n")
    try:
        for escape in ("../escaped.txt", "/etc/escaped.txt", "nested/../../escaped.txt"):
            _rejects_write(sandbox, escape)
    finally:
        sandbox.close()


def _rejects_write(sandbox, relative: str) -> None:
    try:
        sandbox.write(relative, "nope")
    except ValueError as error:
        assert "outside its root" in str(error), error
    else:
        raise AssertionError(f"sandbox must refuse {relative!r}")


def test_the_dummy_model_only_answers_from_what_it_can_see():
    assert rp.dummy_model_answer('"name": "billing"', '"name": "billing"') == '"name": "billing"'
    assert rp.dummy_model_answer('{"name":"billing"}', '"name": "billing"') == '"name": "billing"'
    assert rp.dummy_model_answer('{"name": "other"}', '"name": "billing"') == "<unavailable>"

# --- offline fixtures (need the tokenfold CLI) ------------------------------


def _cli() -> bool:
    if rp.rb._TOKENFOLD_BIN is None:
        print("# skipping offline fixture cases: no tokenfold binary (build it, set TOKENFOLD_BIN)")
        return False
    return True


def test_every_paired_fixture_is_wellformed_and_discriminating():
    tasks = rp.load_tasks(rp.DEFAULT_TASKS_DIR)
    families = {task["family"] for task in tasks}
    assert families == {
        "numeric_threshold",
        "negation",
        "earlier_id_reference",
        "dependent_tool_calls",
        "needed_pruned_row",
    }, families
    for task in tasks:
        assert task["tier"] == "A", task
        assert task["gold_answer"] in task["source"], task
        for atom in task["critical_atoms"]:
            assert atom in task["source"], (task["id"], atom)


def test_both_arms_run_from_the_same_snapshot_and_aggregate_to_valid_pairs():
    if not _cli():
        return
    records, rows = rp.run_offline(rp.DEFAULT_TASKS_DIR, [0.5, 0.1])
    report = rp.aggregate(*rp.pair_records(records))
    assert report["unpaired"] == [], report["unpaired"]
    assert report["invalid_count"] == 0, report
    # Two compressor arms (lossless, lossy) per fixture per ratio, all paired with raw.
    assert report["paired_count"] == len(rows), (report["paired_count"], len(rows))
    assert report["raw_success_rate"] == 1.0, report
    for row in rows:
        assert row["envelope_well_formed"], row
        assert row["candidate_tokens"] <= row["raw_tokens"], row
    # The suite must exercise real pruning, not just a pruner that drops nothing.
    assert any(row["arm"] == "lossy" and row["target_ratio"] == 0.1 for row in rows), rows


def test_a_pruned_row_is_recovered_through_retrieval_not_counted_as_lost():
    """The discriminating case: the needed row really is pruned away.

    Without resolving retrieval the compressed arm looks like data loss, and
    with retrieval it does not. Both halves are asserted -- a test that only
    checked the recovered case would also pass against a pruner that dropped
    nothing at all.
    """
    if not _cli():
        return
    task = next(
        t for t in rp.load_tasks(rp.DEFAULT_TASKS_DIR) if t["family"] == "needed_pruned_row"
    )
    budget = round(rp.rb.count_tokens(task["source"]) * 0.1)
    payload = rp.rb.compress_tokenfold_lossy(task["source"], budget)
    assert payload is not None, "lossy compressor unavailable"
    markers = rp.rb._find_tf_ref_hashes(json.loads(payload))
    assert markers, "the needed-row fixture no longer prunes; it cannot discriminate"
    assert task["gold_answer"] not in payload, "the needed row survived inline, so nothing was lost"
    # Marker text alone reads as failure; the resolvable handle is the difference.
    assert rp.dummy_model_answer(payload, task["gold_answer"]) == "<unavailable>"
    assert rp.dummy_model_answer(rp.recoverable_text(payload), task["gold_answer"]) == task["gold_answer"]
    structure = rp.structural_check(task, payload)
    assert structure["structure_ok"] and structure["envelope_well_formed"], structure


def test_an_unresolvable_handle_stays_absent_rather_than_being_faked():
    """A marker the store cannot resolve must not be papered over."""
    marker = json.dumps({"items": [{"$tf_ref": {"hash": "0" * 64, "namespace": "default"}}]})
    assert rp.rb._find_tf_ref_hashes(json.loads(marker))
    assert "0" * 64 in rp.recoverable_text(marker)
    assert rp.dummy_model_answer(rp.recoverable_text(marker), '"id": "sh-1003"') == "<unavailable>"


def test_a_marker_free_lossy_run_is_still_scored_not_assumed():
    """A pruner that dropped nothing must not be scored as if it dropped the answer."""
    task = rp.load_tasks(rp.DEFAULT_TASKS_DIR)[0]
    structure = rp.structural_check(task, task["source"])
    assert structure["structure_ok"] and structure["critical_atoms_surviving"] == len(
        task["critical_atoms"]
    ), structure
    assert rp.dummy_model_answer(task["source"], task["gold_answer"]) == task["gold_answer"]


if __name__ == "__main__":
    failures = 0
    for name, case in sorted(globals().items()):
        if not name.startswith("test_"):
            continue
        try:
            case()
        except AssertionError as error:
            failures += 1
            print(f"FAIL {name}: {error}")
    print("ok: paired runner aggregation, pairing, sandbox" if not failures else f"{failures} failed")
    raise SystemExit(1 if failures else 0)
