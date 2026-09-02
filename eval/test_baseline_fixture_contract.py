#!/usr/bin/env python3
"""Focused checks for the fail-closed deterministic evaluation fixture contract."""

from __future__ import annotations

import json
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import run_baselines as rb  # noqa: E402


VALID = {
    "id": "fixture_001",
    "family": "test",
    "tier": "A",
    "source": "answer = 42\naudit = AUD-7\n",
    "query": "what is the answer?",
    "gold_answer": "answer = 42",
    "critical_atoms": ["AUD-7"],
}


def _load(fixture: dict):
    with tempfile.TemporaryDirectory() as directory:
        Path(directory, "fixture.json").write_text(json.dumps(fixture), encoding="utf-8")
        return rb.load_fixtures(Path(directory))


def test_valid_fixture_loads():
    assert _load(VALID) == [VALID]


def test_missing_gold_answer_is_rejected_instead_of_vacuously_passing():
    fixture = dict(VALID)
    fixture.pop("gold_answer")
    try:
        _load(fixture)
    except ValueError as error:
        assert "gold_answer" in str(error)
    else:
        raise AssertionError("missing gold_answer must be rejected")


def test_ambiguous_gold_answer_is_rejected():
    fixture = dict(VALID, source="answer = 42\nanswer = 42\naudit = AUD-7\n")
    try:
        _load(fixture)
    except ValueError as error:
        assert "exactly once" in str(error)
    else:
        raise AssertionError("an ambiguous answer must be rejected")


def test_ungrounded_critical_atom_is_rejected():
    fixture = dict(VALID, critical_atoms=["MISSING"])
    try:
        _load(fixture)
    except ValueError as error:
        assert "not grounded" in str(error)
    else:
        raise AssertionError("an ungrounded safety atom must be rejected")


def test_empty_ratio_list_cannot_vacuously_pass_gate():
    assert rb.main(["--gate", "--ratios", ","]) == 2


def test_compressor_only_fixture_does_not_inflate_selector_scores():
    compressor_only = dict(VALID, id="compressor_001", evaluation_kind="compressor")
    compressors = rb.COMPRESSORS
    rb.COMPRESSORS = {}
    try:
        report = rb.build_report([VALID, compressor_only], [0.5])
    finally:
        rb.COMPRESSORS = compressors
    assert report["fixture_count"] == 2
    assert report["selector_fixture_count"] == 1
    assert {row["fixture"] for row in report["rows"]} == {VALID["id"]}


def test_claim_scoring_understands_lossless_columnar_json():
    fixture = {
        **VALID,
        "source": '{"items":[{"name":"worker","port":7443}],"audit":"AUD-7"}',
        "gold_answer": '"name":"worker","port":7443',
    }
    folded = (
        '{"items":{"__tf_cols__":["name","port"],'
        '"__tf_rows__":[["worker",7443]]},"audit":"AUD-7"}'
    )
    score = rb.score_task(folded, fixture)
    assert score["task_success"] == 1.0, score
    assert score["claim_faithfulness"] == 1.0, score


if __name__ == "__main__":
    test_valid_fixture_loads()
    test_missing_gold_answer_is_rejected_instead_of_vacuously_passing()
    test_ambiguous_gold_answer_is_rejected()
    test_ungrounded_critical_atom_is_rejected()
    test_empty_ratio_list_cannot_vacuously_pass_gate()
    test_compressor_only_fixture_does_not_inflate_selector_scores()
    test_claim_scoring_understands_lossless_columnar_json()
    print("ok: baseline fixture contract fails closed")
