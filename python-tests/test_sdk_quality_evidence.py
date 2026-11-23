"""Reconcile frozen answer-quality evidence offline; never invoke a scorer/provider."""
from decimal import Decimal
import hashlib
import json
from pathlib import Path
import statistics

import pytest

ROOT = Path(__file__).resolve().parents[1]
METRICS = ROOT / "tests/fixtures/readme_metrics.json"
pytestmark = pytest.mark.skipif(not METRICS.exists(), reason="repository evidence absent")


def canonical(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=True, separators=(",", ":"), allow_nan=False)


def digest(value):
    return hashlib.sha256(canonical(value).encode()).hexdigest()


def evidence():
    pins = json.loads(METRICS.read_text())["sdk_0_6_quality"]
    return pins, json.loads((ROOT / pins["source"]).read_text()), json.loads((ROOT / pins["corpus"]).read_text())


def correct(row, oracle):
    def unique(pairs):
        result = dict(pairs)
        if len(result) != len(pairs):
            raise ValueError("duplicate key")
        return result
    try:
        actual = json.loads(row["output"], object_pairs_hook=unique)
        return (row["finish_reason"] == "stop" and not row["refusal"] and type(actual) is dict
                and canonical(actual) == canonical(oracle))
    except (ValueError, TypeError):
        return False


def test_quality_pins_and_visible_readme_claims():
    pins, report, corpus = evidence()
    assert digest(report) == pins["source_canonical_json_sha256"]
    assert digest(corpus) == pins["corpus_canonical_json_sha256"] == report["corpus_canonical_json_sha256"]
    assert report["model"] == pins["model"] == corpus["protocol"]["request_template"]["model"]
    assert not pins["qualification"] and not report["qualification"]
    text = (ROOT / "README.md").read_text()
    assert "Exact recovery did not guarantee correct model answers" in text
    assert "45.6-63.9% on two fixtures, lossless" in text
    assert "without a versioned source" in text
    assert "| Explicit folded JSON vs. compact JSON | 4 x 2 | 8/8 | **6/8** | 24,496 -> 16,600 (**32.2% fewer**) |" in text
    assert "| Select at a 50% rendered-text budget vs. full context | 8 x 2 | 16/16 | 16/16 | 5,706 -> 3,416 (**40.1% fewer**) |" in text


def test_every_quality_reply_is_independently_regraded_and_bound_to_its_request():
    _, report, corpus = evidence()
    tasks = {t["id"]: t for t in corpus["tasks"]}
    assert len(tasks) == 12 and len(report["calls"]) == 48
    ids = set()
    for row in report["calls"]:
        task = tasks[row["task"]]
        key = row["task"], row["repeat"], row["arm"]
        assert key not in ids
        ids.add(key)
        assert row["oracle_pass"] == correct(row, task["oracle"])
        body = {**corpus["protocol"]["request_template"], "messages": task["arms"][row["arm"]]["messages"]}
        assert hashlib.sha256(json.dumps(body, sort_keys=True).encode()).hexdigest() == row["body_sha256"]
        assert row["returned_model"] == report["model"] and row["provider"] == "Nvidia"
        usage = row["usage"]
        assert all(type(usage[k]) is int and usage[k] >= 0 for k in ("prompt_tokens", "completion_tokens", "total_tokens"))
        assert usage["prompt_tokens"] + usage["completion_tokens"] == usage["total_tokens"]
        assert usage["completion_tokens"] <= body["max_tokens"]
        assert usage["is_byok"] is False and Decimal(str(usage["cost"])) == 0
    assert ids == {(id, repeat, arm) for id in tasks for repeat in range(2) for arm in ("baseline", "candidate")}
    base = {"output": '{"v":1}', "finish_reason": "stop", "refusal": None}
    assert correct(base, {"v": 1})
    for output in ('{"v":true}', '{"v":1.0}', '{"v":0,"v":1}', '{"v":NaN}', '```json\n{"v":1}\n```'):
        assert not correct({**base, "output": output}, {"v": 1})
    assert not correct({**base, "refusal": "refused"}, {"v": 1})


def test_quality_totals_retain_losses_cached_tokens_and_failed_probe():
    pins, report, _ = evidence()
    for family in ("core", "select"):
        summary = report["families"][family]
        for arm in ("baseline", "candidate"):
            rows = [r for r in report["calls"] if r["family"] == family and r["arm"] == arm]
            totals = summary["arms"][arm]
            assert totals["trials"] == len(rows) == pins[family]["trials"]
            assert totals["oracle_passes"] == sum(r["oracle_pass"] for r in rows) == pins[family][f"{arm}_correct"]
            for name in ("prompt", "completion"):
                assert totals[f"provider_{name}_tokens"] == sum(r["usage"][f"{name}_tokens"] for r in rows)
            assert totals["provider_cached_tokens"] == sum(r["usage"]["prompt_tokens_details"]["cached_tokens"] for r in rows)
            assert totals["provider_call_wall_median_ms"] == round(statistics.median(r["wall_ms"] for r in rows), 6)
        before, after = [summary["arms"][a]["provider_prompt_tokens"] for a in ("baseline", "candidate")]
        assert round(100 * (before - after) / before, 6) == summary["provider_prompt_reduction_pct"]
        assert round(summary["provider_prompt_reduction_pct"], 1) == pins[family]["provider_prompt_reduction_pct"]
    losses = [r for r in report["calls"] if not r["oracle_pass"]]
    assert [(r["task"], r["arm"]) for r in losses] == [("core-api-lookup", "candidate")] * 2
    assert report["families"]["core"]["paired"]["baseline_only"] == len(losses)
    first = report["prior_scopes"][0]
    assert (first["probe_http_status"], first["quality_calls_attempted"], first["quality_calls_unexecuted"]) == (429, 0, 48)
    assert first["probe_usage"] is None and first["probe_cost_usd"] is None
    accounting = report["all_attempt_accounting"]
    assert accounting["actual_generation_attempts"] == first["probe_attempts"] + report["nvidia_probe"]["attempts"] + len(report["calls"]) == 50
    assert accounting["unknown_usage_attempts"] == accounting["unknown_reported_cost_attempts"] == 1
    assert accounting["complete_campaign_spend_usd"] is None
    for name in ("prompt", "completion"):
        assert accounting[f"nvidia_observed_{name}_tokens"] == sum(r["usage"][f"{name}_tokens"] for r in report["calls"]) + report["nvidia_probe"]["usage"][f"{name}_tokens"]


def test_frozen_sdk_payloads_recount_recover_and_preserve_group_boundaries():
    from tokenfold import EncodingCounter, RenderedRequest
    from tokenfold.lowlevel import decode
    _, _, corpus = evidence()
    counter = EncodingCounter()
    positions = []
    for task in corpus["tasks"]:
        for arm in task["arms"].values():
            payload = arm["messages"][0]["content"]
            count = counter.count(RenderedRequest("text", json.dumps(payload), "rendered_text"))
            assert count.basis == "exact" and count.tokens == arm["rendered_tokens"]
            assert arm["accounting"]["scope"] == "rendered_text"
        baseline, candidate = task["arms"]["baseline"], task["arms"]["candidate"]
        if task["family"] == "core":
            source = ROOT / task["source"]
            assert hashlib.sha256(source.read_bytes()).hexdigest() == task["source_sha256"]
            assert json.loads(decode(candidate["messages"][0]["content"])) == json.loads(source.read_text())
        else:
            groups = task["groups"]
            kept = candidate["groups_kept"]
            assert kept == [g["id"] for g in groups if g["id"] in kept]
            assert candidate["groups_dropped"] == [g["id"] for g in groups if g["id"] not in kept]
            assert candidate["messages"][0]["content"] == task["query"] + "".join("\n\n" + g["content"] for g in groups if g["id"] in kept)
            assert candidate["scorer_succeeded"] and candidate["selection_applied"]
            assert candidate["budget_status"] == "met"
            assert candidate["target_tokens"] == baseline["rendered_tokens"] // 2
            assert candidate["rendered_tokens"] <= candidate["target_tokens"]
            positions.append(task["supporting_group_position"])
    assert positions == [0, 1, 2, 3, 4, 0, 1, 2]
