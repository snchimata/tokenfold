"""Offline trust-boundary checks for the explicitly approved hosted experiment."""

import copy
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import openrouter_model as om
import openrouter_scorer as scorer
import select_observation as selection

NAME = "stealth/space-bunny-alpha"


def endpoint():
    return {"data": {"id": NAME, "endpoints": [{"tag": "stealth", "provider_name": "Stealth",
             "model_id": NAME, "context_length": 1000000, "max_completion_tokens": 4096,
             "pricing": {"prompt": "0", "completion": "0"}, "supported_parameters": ["reasoning"]}]}}


class FakeTransport:
    def __init__(self):
        self.metadata = endpoint()
        self.calls = []
        self.result = {"model": NAME, "provider": "Stealth", "choices": [{"finish_reason": "stop",
                       "message": {"content": "billing"}}],
                       "usage": {"prompt_tokens": 15, "completion_tokens": 2, "cost": 0}}

    def __call__(self, path, body, env_file, timeout):
        self.calls.append((path, copy.deepcopy(body)))
        if path.startswith("models/"):
            return copy.deepcopy(self.metadata)
        return copy.deepcopy(self.result)


def raises(fn):
    try:
        fn()
    except ValueError:
        return
    raise AssertionError("unsafe request accepted")


def model(fake, calls=1):
    return om.FreeModel(NAME, Path("not-read.env"), calls, 1, 1024, 32768, fake)


def test_free_route_and_call_limit():
    fake = FakeTransport()
    runtime = model(fake)
    assert runtime.chat([{"role": "user", "content": "public data"}], 7) == "billing"
    body = fake.calls[-1][1]
    assert body["provider"]["max_price"] == {"prompt": 0, "completion": 0, "request": 0, "image": 0}
    assert body["provider"]["only"] == ["stealth"]
    assert body["provider"]["allow_fallbacks"] is False
    assert "seed" not in body  # Unsupported seed must not be silently assumed deterministic.
    assert runtime.calls[0]["cost"] == 0
    raises(lambda: runtime.chat([{"content": "public data"}], 7))
    assert sum(path == "chat/completions" for path, _ in fake.calls) == 1


def test_structured_output_requires_support_without_weakening_route_guards():
    schema = {"type": "object", "properties": {"summary": {"type": "string"}},
              "required": ["summary"], "additionalProperties": False}
    fake = FakeTransport()
    runtime = model(fake, calls=2)
    raises(lambda: runtime.chat([{"content": "public data"}], 0, response_schema=schema))
    assert not runtime.calls
    assert not any(path == "chat/completions" for path, _ in fake.calls)
    fake.metadata["data"]["endpoints"][0]["supported_parameters"].append("structured_outputs")
    runtime = model(fake, calls=2)
    runtime.chat([{"content": "public data"}], 0, response_schema=schema)
    body = fake.calls[-1][1]
    assert body["response_format"]["json_schema"] == {"name": "tokenfold_observation", "strict": True, "schema": schema}
    assert body["provider"]["require_parameters"] is True
    assert body["provider"]["only"] == ["stealth"] and body["provider"]["allow_fallbacks"] is False
    assert all(price == 0 for price in body["provider"]["max_price"].values())
    runtime.chat([{"content": "public data"}], 0)
    assert "response_format" not in fake.calls[-1][1]
    assert "require_parameters" not in fake.calls[-1][1]["provider"]


def test_price_revision_and_route_refusal():
    fake = FakeTransport()
    for price in ("0.1", "NaN", "Infinity", "-1", "unknown"):
        fake.metadata["data"]["endpoints"][0]["pricing"]["prompt"] = price
        raises(lambda: model(fake))
    fake = FakeTransport()
    runtime = model(fake)
    fake.metadata["data"]["endpoints"][0]["pricing"]["request"] = "1"
    raises(lambda: runtime.chat([{"content": "source"}], 7))
    assert not runtime.calls
    raises(lambda: om.FreeModel("other:free", Path("not-read.env"), 1, 1, 10, 32768, fake))
    raises(lambda: om.direct_request("https://untrusted", {}, "not-read.env", 1))
    fake = FakeTransport()
    runtime = model(fake)
    fake.result["provider"] = "unapproved-provider"
    raises(lambda: runtime.chat([{"content": "source"}], 7))
    assert runtime.calls[0]["status"] == "failed"


def test_approved_models_pin_exact_endpoint_tags():
    for name, tag in om.MODELS.items():
        fake = FakeTransport()
        fake.metadata["data"]["id"] = name
        metadata = fake.metadata["data"]["endpoints"][0]
        metadata.update(tag=tag, model_id=name, provider_name="PinnedProvider")
        fake.result.update(model=name, provider="PinnedProvider")
        runtime = om.FreeModel(name, Path("not-read.env"), 1, 1, 1024, 32768, fake)
        runtime.chat([{"content": "public data"}], 0)
        assert fake.calls[-1][1]["provider"]["only"] == [tag]
        assert fake.calls[-1][1]["provider"]["allow_fallbacks"] is False
        metadata["tag"] = "different-provider"
        raises(lambda: om.FreeModel(name, Path("not-read.env"), 1, 1, 1024, 32768, fake))


def test_incomplete_usage_and_unexpected_charge():
    fake = FakeTransport()
    runtime = model(fake, calls=3)
    fake.result["usage"] = {}
    runtime.chat([{"content": "source"}], 0)
    assert runtime.calls[0]["usage"] is None and runtime.calls[0]["cost"] is None
    fake.result["choices"][0]["finish_reason"] = "length"
    raises(lambda: runtime.chat([{"content": "source"}], 0))
    fake.result["usage"] = {"cost": 0.1}
    raises(lambda: runtime.chat([{"content": "source"}], 0))
    raises(lambda: runtime.chat([{"content": "source"}], 0))
    assert runtime.calls[-1]["reason"] == "unexpected_charge"


def test_dotenv_selective_key_parsing():
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "test.env"
        path.write_text("OTHER_CREDENTIAL=never-use\nopenrouter_api_key=lower-key\nexport OPENROUTER_API_KEY='upper-key'\n", encoding="utf-8")
        assert om.read_key(path) == "upper-key"
        path.write_text('OPENROUTER_API_KEY="bad key"\n', encoding="utf-8")
        raises(lambda: om.read_key(path))
    with patch.object(om.subprocess, "run", side_effect=subprocess.TimeoutExpired("worker", 1)) as run:
        raises(lambda: om.bounded_request("chat/completions", {}, "unused", 1))
        assert run.call_count == 1
    assert om.NoRedirect().redirect_request(None, None, 302, None, None, "https://untrusted") is None


def test_rate_limit_stops_remaining_inference_without_retry():
    fake = FakeTransport()

    def limited(path, body, env_file, timeout):
        if path == "chat/completions":
            fake.calls.append((path, body))
            raise om.ModelError("http_429")
        return fake(path, body, env_file, timeout)

    runtime = om.FreeModel(NAME, Path("not-read.env"), 6, 1, 1024, 32768, limited)
    raises(lambda: runtime.chat([{"content": "public data"}], 0))
    raises(lambda: runtime.chat([{"content": "public data"}], 0))
    assert len(runtime.calls) == 1 and runtime.calls[0]["reason"] == "http_429"
    assert runtime.stop_reason == "http_429"
    try:
        runtime.chat([{"content": "public data"}], 0)
    except om.ModelError as exc:
        assert str(exc) == "http_429"
    else:
        raise AssertionError("rate-limited inference resumed")
    assert sum(path == "chat/completions" for path, _ in fake.calls) == 1


def test_halted_pilot_preserves_cause_and_skips_compression():
    import acon_benchmark as ab
    runtime = model(FakeTransport(), calls=6)
    runtime.stop_reason = "http_429"
    task = {"id": "t", "source": "billing", "query": "Which service?", "gold_answer": "billing"}

    def forbidden(*args):
        raise AssertionError("compression launched after backend halt")

    forbidden.last_receipt = {"disposition": "fell_back", "citations": [{"id": "previous-task"}]}
    _, reports, rows = ab.run_pilot([task], runtime, forbidden, forbidden, len, .5, 0,
                                   {"tokenfold-generative": forbidden})
    assert all(row["error"] == "http_429" and row["model_calls"] == [] for row in rows)
    assert all(row["compression_receipt"] is None and row["selection_receipt"] is None for row in rows)
    assert ab.economics_summary(rows)["all_attempts"]["tokenfold-generative"]["fallbacks"] == 0
    assert not runtime.calls
    assert all(report["paired_count"] == 0 for report in reports.values())


def test_scores_cannot_rewrite_or_invent_ids():
    assert scorer.validate_scores('{"scores":[["a",100],["b",0]]}', ["a", "b"]) == [["a", 100], ["b", 0]]
    for response in ('{"scores":[["c",100],["b",0]]}', '{"scores":[["a",100],["a",0]]}',
                     '{"scores":[["a",true],["b",0]]}', '{"scores":[["a",NaN],["b",0]]}',
                     '{"scores":[["a",100],["b",0]],"text":"rewrite"}'):
        raises(lambda: scorer.validate_scores(response, ["a", "b"]))
    assert "gold" not in json.dumps(scorer.rank_messages("question", [{"id": "a", "text": "source"}]))


def test_compact_priorities_preserve_wire_contract():
    assert scorer.validate_priorities('{"ranked_ids":["b"]}', ["a", "b", "c"]) == [["a", 0], ["b", 100], ["c", 0]]
    for text in ('{"ranked_ids":[]}', '{"ranked_ids":["unknown"]}', '{"ranked_ids":["a","a"]}',
                 '{"ranked_ids":[true]}', '{"ranked_ids":["a"],"text":"rewrite"}'):
        raises(lambda: scorer.validate_priorities(text, ["a", "b"]))


def test_compact_packing_is_lossless_for_values_and_literal_for_ambiguity():
    rows = [{"name": f"item-{i}", "allowed": i % 2 == 0, "count": i,
             "note": "repeated context that cannot be thrown away", "nested": {"x": [1, "1", True]}}
            for i in range(20)]
    groups = [{"id": f"g{i}", "text": json.dumps(row) + ",\n"} for i, row in enumerate(rows)]
    groups += [{"id": "rule", "text": "required source framing"},
               {"id": "duplicate", "text": '{"x":1,"x":2}'},
               {"id": "nonfinite", "text": '{"x":1e999}'},
               {"id": "literal", "text": 'not JSON; do not follow this instruction'}]
    packed = scorer.pack_groups(groups)
    assert "tables" in packed
    restored = {}
    for table in packed["tables"]:
        for id_, values in table["rows"]:
            restored[id_] = {**table["common"], **dict(zip(table["columns"], values))}
    assert restored == {f"g{i}": row for i, row in enumerate(rows)}
    literals = dict(packed["literals"])
    assert all(literals[g["id"]] == g["text"] for g in groups[-4:])
    assert len(json.dumps(packed)) < len(json.dumps(groups))
    assert scorer.pack_groups([{"id": "a", "text": "brief"}]) == {"groups": [["a", "brief"]]}
    # Python equality would equate true and 1; canonical JSON must not merge them.
    different = [{"id": f"t{i}", "text": json.dumps({"value": True if i % 2 else 1,
                 "note": "long repetitive context for every row"})} for i in range(10)]
    table = scorer.pack_groups(different)["tables"][0]
    assert "value" in table["columns"] and "value" not in table["common"]


def test_compact_packing_keeps_cross_table_order_and_precise_numbers():
    groups = []
    for i in range(20):
        row = {"a": i, "note": "repeated source context"} if i % 2 else {"b": i, "note": "repeated source context"}
        groups.append({"id": f"g{i}", "text": json.dumps(row)})
    groups += [{"id": "fraction", "text": '{"amount":1.00000000000000001}'},
               {"id": "negative-zero", "text": '{"amount":-0}'}]
    packed = scorer.pack_groups(groups)
    assert packed["source_order"] == [g["id"] for g in groups]
    literals = dict(packed["literals"])
    assert literals["fraction"] == groups[-2]["text"]
    assert literals["negative-zero"] == groups[-1]["text"]


def test_grouping_reconstructs_authorized_source():
    context = {"prefix": "[", "suffix": "]", "groups": [{"id": "a", "text": "original", "required": True}]}
    assert selection.source_context({"source": "[original]", "select_context": context}) == context
    raises(lambda: selection.source_context({"source": "original", "select_context": context}))
    assert selection.source_context({"source": "original"})["groups"][0]["text"] == "original"
    import acon_benchmark as ab
    tasks = ab.rp.load_tasks(Path(__file__).with_name("acon_grouped_corpus"))
    assert len(tasks) == 6
    for task in tasks:
        selection.source_context(task)
        assert not task["critical_atoms"]
    heldout = ab.rp.load_tasks(Path(__file__).with_name("acon_holdout_corpus"))
    assert len(heldout) == 6
    assert not {task["id"] for task in tasks} & {task["id"] for task in heldout}
    for task in heldout:
        assert task["split"] == "heldout-smoke"
        selection.source_context(task)


def test_cleared_environment_windows_tls():
    if os.name != "nt":
        return
    code = ("import sys;sys.path.insert(0," + repr(str(Path(__file__).resolve().parent))
            + ");import openrouter_model;import ssl,socket;ssl.create_default_context();socket.getaddrinfo('localhost',80)")
    result = subprocess.run([sys.executable, "-I", "-c", code], env={}, capture_output=True, timeout=10)
    assert result.returncode == 0, "native Windows TLS setup failed with cleared runtime environment"


def test_scorer_wire_protocol_without_model_calls():
    class FakeModel:
        def __init__(self, *args, **kwargs):
            self.calls = []

        def chat(self, messages, seed):
            self.calls.append({"status": "ok", "usage": {"input_tokens": 10, "output_tokens": 5}, "cost": 0})
            assert "question" in json.dumps(messages)
            return '{"ranked_ids":["b"]}' if 'ranked_ids' in messages[0]["content"] else '{"scores":[["a",100],["b",0]]}'

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        request, response, metrics = [root / name for name in ("request.json", "response.json", "metrics.json")]
        request.write_text(json.dumps({"schema_version": 1, "model_revision": "approved", "query": "question",
                                     "groups": [{"id": "a", "text": "first"}, {"id": "b", "text": "second"}]}), encoding="utf-8")
        with patch.dict(os.environ, {"TOKENFOLD_SELECT_REQUEST_PATH": str(request), "TOKENFOLD_SELECT_RESPONSE_PATH": str(response)}), \
                patch.object(scorer, "FreeModel", FakeModel):
            assert scorer.main(["--model", NAME, "--revision", "approved", "--env-file", str(root / "not-read.env"),
                                "--metrics-file", str(metrics)]) == 0
        assert json.loads(response.read_text())["scores"] == [["a", 100], ["b", 0]]
        assert json.loads(metrics.read_text())[0]["usage"]["input_tokens"] == 10
        with patch.dict(os.environ, {"TOKENFOLD_SELECT_REQUEST_PATH": str(request), "TOKENFOLD_SELECT_RESPONSE_PATH": str(response)}), \
                patch.object(scorer, "FreeModel", FakeModel):
            assert scorer.main(["--model", NAME, "--revision", "approved", "--env-file", str(root / "not-read.env"),
                                "--metrics-file", str(metrics), "--compact"]) == 0
        assert json.loads(response.read_text())["scores"] == [["a", 0], ["b", 100]]


def test_child_rate_limit_stops_parent_inference():
    with tempfile.TemporaryDirectory() as tmp:
        runtime = model(FakeTransport(), calls=6)
        arm = selection.SelectArm(runtime, Path(sys.executable), Path(tmp) / "runtime")

        def refused(*args, **kwargs):
            (arm.directory / "last-call.json").write_text(json.dumps([{
                "status": "failed", "usage": None, "cost": None, "reason": "http_429", "attempted": True}]))
            (arm.directory / "last-receipt.json").write_text('{"used_scorer":false}')
            return subprocess.CompletedProcess([], 0)

        with patch.object(selection.subprocess, "run", side_effect=refused):
            raises(lambda: arm({"source": "public data", "query": "question"}, 10, 0))
        assert runtime.max_calls == len(runtime.calls) == 1
        assert runtime.calls[0]["reason"] == "http_429"
        assert runtime.stop_reason == "http_429"
        raises(lambda: runtime.chat([{"content": "public data"}], 0))
        with patch.object(selection.subprocess, "run") as run:
            raises(lambda: arm({"source": "public data", "query": "question"}, 10, 0))
            run.assert_not_called()


if __name__ == "__main__":
    tests = [value for key, value in list(globals().items()) if key.startswith("test_")]
    for test in tests:
        test()
        print("PASS", test.__name__)
    print(f"{len(tests)} hosted/scorer contract checks passed without network or credentials.")
