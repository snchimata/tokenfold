"""Offline contract checks; scripted responses are never model-quality evidence."""

import copy
import json
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import acon_benchmark as ab

DIGEST = "a" * 64


def test_explicit_arm_order_keeps_pairing_and_rejects_missing_arms():
    fake = ScriptedTransport()
    fake.responses = [response("billing")] * 3
    runtime = model(fake)
    task = {"id": "order", "source": "billing", "query": "Service?", "gold_answer": "billing"}
    order = ["acon-observation", "tokenfold-lossless", "raw"]
    _, reports, rows = ab.run_pilot([task], runtime, lambda task, seed: task["source"],
                                   lambda source, target: source, len, .5, 0, arm_order=order)
    assert [row["candidate"] for row in rows] == order
    assert all(report["paired_count"] == 1 for report in reports.values())
    assert len(runtime.calls) == 3
    raises(ValueError, lambda: ab.run_pilot([task], runtime, lambda *args: "billing",
        lambda source, target: source, len, .5, 0, arm_order=["raw", "raw", "acon-observation"]))
    assert len(runtime.calls) == 3


def test_output_budget_distinguishes_available_fallback_and_blocked_input():
    base={"candidate":"summary","task_id":"a","outcome":"invalid","total_usage":None,
          "billed_cost":None,"wall_ms":1,"payload_tokens":10,"target_met":False}
    available={**base,"execution":{"output_available_for_answering":True},
               "compression_receipt":{"disposition":"raw_fallback"}}
    blocked={**base,"task_id":"b","execution":{"output_available_for_answering":False,"blocked_by":"primary_model"}}
    unknown={**base,"task_id":"c"}
    budget=ab.economics_summary([available,blocked,unknown])["all_attempts"]["summary"]["observed_output_budget"]
    assert budget["available_outputs"]==budget["target_misses"]==budget["known_fallback_outputs"]==1
    assert budget["unavailable_outputs"]==budget["execution_unknown"]==1
    raw={**available,"candidate":"raw"}
    control=ab.economics_summary([raw])["all_attempts"]["raw"]["observed_output_budget"]
    assert control["available_outputs"]==control["target_misses"]==0 and control["raw_controls_excluded"]==1


def test_halted_primary_marks_unexecuted_compressors_without_charging_work():
    import native_model as nm
    def transport(port,path,body,timeout):
        if path=="models":return {"data":[{"id":"Qwen/Qwen3.5-0.8B"}]}
        raise nm.ModelError("client_deadline")
    runtime=nm.NativeModel("Qwen/Qwen3.5-0.8B","a"*64,8000,4,25,128,16384,transport)
    compressed=[]
    def compressor(*args):
        compressed.append(True);return "answer"
    task={"id":"halt","source":"source","query":"query","gold_answer":"answer"}
    _,_,rows=ab.run_pilot([task],runtime,compressor,compressor,len,.5,0,{"other":compressor})
    assert len(runtime.calls)==1 and not compressed
    assert rows[0]["execution"]["answer_method_attempted"]
    for row in rows[1:]:
        assert row["execution"]=={"compression_method_attempted":False,"answer_method_attempted":False,
                                 "output_available_for_answering":False,"blocked_by":"primary_model"}
        assert not row["model_calls"] and row["compression_receipt"] is None
        assert row["outcome"]=="invalid"
    total=ab.economics_summary(rows)["all_attempts"]["other"]
    assert total["blocked_before_methods"]==1 and total["execution_state_unknown"]==0
    assert total["observed_output_budget"]["available_outputs"]==0
    assert total["observed_output_budget"]["target_misses"]==0
    legacy=dict(rows[-1]);del legacy["execution"]
    assert ab.economics_summary([legacy])["all_attempts"]["other"]["execution_state_unknown"]==1
    assert ab.economics_summary([legacy])["all_attempts"]["other"]["observed_output_budget"]["execution_unknown"]==1


def test_failed_native_accounting_work_survives_without_fake_generation():
    import native_model as nm
    def transport(port,path,body,timeout):
        if path=="models":return {"data":[{"id":"Qwen/Qwen3.5-0.8B"}]}
        if path=="apply-template":return {"prompt":"templated"}
        if path=="tokenize":return {"tokens":[True]}
        raise AssertionError("generation must not run")
    runtime=nm.NativeModel("Qwen/Qwen3.5-0.8B","a"*64,8000,4,25,128,2048,transport,
                           native_template_preflight=True)
    task={"id":"accounting","source":"answer source","query":"query","gold_answer":"answer"}
    _,_,rows=ab.run_pilot([task],runtime,lambda *args:"source",lambda source,target:source,len,.5,0)
    assert not runtime.calls
    assert len(runtime.accounting_calls)==1
    assert rows[0]["preflight_accounting_by_model"][runtime.name+"@"+runtime.digest][0]["status"]=="failed"
    total=ab.economics_summary(rows)["preflight_accounting"][runtime.name+"@"+runtime.digest]
    assert total["attempts"]==1 and total["invalid"]==1 and total["endpoint_requests"]==2
    assert total["billed_cost"] is None and total["unknown_cost_records"]==1


def test_payload_accounting_counts_once_without_reusing_wrapper_counts():
    transport = ScriptedTransport()
    transport.responses = [response("gold")] * 3
    seen = []
    def count(text):
        seen.append(text)
        return len(text)
    task = {"id": "counts", "source": "gold source with wrappers", "query": "query", "gold_answer": "gold"}
    _, _, rows = ab.run_pilot([task], model(transport), lambda *args: "gold",
        lambda source, target: source, count, .5, 42, {"invalid-payload": lambda *args: None})
    assert seen.count(task["source"]) == 1
    assert seen.count("gold") == 1
    assert len(seen) == 8  # Previously 13: baseline + six prompt counts + six payload counts.
    for row in rows:
        if row["candidate"] == "invalid-payload":
            assert row["payload_tokens"] is None and row["target_met"] is False
            continue
        assert row["payload_tokens"] == len(row["payload"])
        assert row["target_met"] == (len(row["payload"]) <= row["target_payload_tokens"])
        assert row["answer_prompt_text_tokens"] == sum(len(m["content"]) for m in ab.answer_messages(task,row["payload"]))
    transport = ScriptedTransport()
    transport.responses = [response("gold")] * 3
    def changing_compressor(task, seed):
        task["source"] = "different source length"
        return task["source"]
    _, _, changed = ab.run_pilot([task], model(transport), changing_compressor,
        lambda source, target: source, len, .5, 42)
    assert changed[-1]["payload_tokens"] == len("different source length")


def test_stage_latency_retains_failures_and_distinguishes_unstarted_work():
    transport = ScriptedTransport()
    transport.responses = [response("gold"), response("gold"), ValueError("answer failed")]
    runtime = model(transport)
    task = {"id": "stages", "source": "gold source", "query": "query", "gold_answer": "gold"}
    def fail_compression(*args):
        raise ValueError("compression failed")
    _, _, rows = ab.run_pilot([task], runtime, fail_compression,
        lambda source, target: "gold", len, .5, 42, {"answer-failure": lambda *args: "gold"})
    by_name = {row["candidate"]: row for row in rows}
    raw = by_name["raw"]["stage_wall_ms"]
    assert raw["compression"] is None and raw["answer"] >= 0
    assert all(value is not None for value in by_name["tokenfold-lossless"]["stage_wall_ms"].values())
    compression = by_name["acon-observation"]["stage_wall_ms"]
    assert compression["compression"] >= 0 and compression["validation"] is None and compression["answer"] is None
    assert by_name["acon-observation"]["outcome"] == "invalid"
    answer = by_name["answer-failure"]
    assert answer["outcome"] == "invalid" and answer["stage_wall_ms"]["answer"] >= 0
    assert len(runtime.calls) == 3  # No inference retry or missing consumed attempt.


def test_partial_costs_never_become_known_zero_or_complete_economics():
    row = {"task_id": "a", "candidate": "raw", "outcome": "success",
           "total_usage": None, "usage_by_model": {"model-a": None},
           "billed_cost": 0.25, "wall_ms": 1, "payload_tokens": 1, "target_met": True}
    other = {**row, "task_id": "b", "billed_cost": None}
    result = ab.economics_summary([row, other])
    assert result["complete"] is True  # Attempt validity is not accounting completeness.
    total = result["all_attempts"]["raw"]
    assert total["billed_cost"] is None and total["known_billed_cost"] == 0.25
    assert total["unknown_cost_records"] == 1 and not total["cost_accounting_complete"]
    for unknown in (None, True, -1, float("nan"), float("inf")):
        total = ab.economics_summary([{**row, "billed_cost": 0},
                                     {**other, "billed_cost": unknown}])["all_attempts"]["raw"]
        assert total["known_billed_cost"] == 0 and total["billed_cost"] is None
        assert total["unknown_cost_records"] == 1
    total = ab.economics_summary([{**row, "billed_cost": 0}])["all_attempts"]["raw"]
    assert total["billed_cost"] == 0 and total["cost_accounting_complete"]
    receipt = {"valid_attempt": False, "model_calls": [
        {"usage": None, "cost": 0.5}, {"usage": None, "cost": None}], "wall_ms": 2}
    evaluation = ab.economics_summary([{**row, "judge_receipt": receipt}])["evaluation"]
    assert evaluation["billed_cost"] is None and evaluation["known_billed_cost"] == 0.5
    assert evaluation["unknown_cost_records"] == 1 and not evaluation["cost_accounting_complete"]


def test_latency_summary_counts_unknown_and_failed_stages_without_zero_fill():
    row = {"task_id": "a", "candidate": "raw", "outcome": "invalid",
           "total_usage": None, "billed_cost": None, "wall_ms": 30,
           "payload_tokens": 1, "target_met": True,
           "stage_wall_ms": {"preflight": 2, "compression": None, "validation": 3, "answer": 20}}
    rows = [row, {**row, "task_id": "b", "stage_wall_ms": {"answer": None}},
            {**row, "task_id": "c", "stage_wall_ms": {}},
            {**row, "task_id": "d", "stage_wall_ms": {"answer": float("nan")}}]
    total = ab.economics_summary(rows)["all_attempts"]["raw"]
    assert total["p99_wall_ms"] == 30
    answer = total["stage_latency"]["answer"]
    assert answer["timed_attempts"] == 1 and answer["untimed_attempts"] == 3
    assert answer["p50_wall_ms"] == answer["p95_wall_ms"] == answer["p99_wall_ms"] == 20
    compression = total["stage_latency"]["compression"]
    assert compression["timed_attempts"] == 0 and compression["untimed_attempts"] == 4
    assert compression["p99_wall_ms"] is None
    distribution = ab.economics_summary([{**row, "task_id": str(i), "wall_ms": i,
        "stage_wall_ms": {"answer": i}} for i in range(1, 101)])["all_attempts"]["raw"]
    assert distribution["p99_wall_ms"] == 99
    assert [distribution["stage_latency"]["answer"][f"p{p}_wall_ms"] for p in (50, 95, 99)] == [50, 95, 99]


class ScriptedTransport:
    def __init__(self):
        self.requests = []
        self.responses = []
        self.digest = DIGEST
        self.info = {"capabilities": ["completion"], "model_info": {"architecture": "test"}}

    def __call__(self, path, body, timeout):
        self.requests.append((path, copy.deepcopy(body), timeout))
        if path == "tags":
            return {"models": [{"name": "test:local", "digest": self.digest}]}
        if path == "show":
            return self.info
        result = self.responses.pop(0)
        if isinstance(result, Exception):
            raise result
        return result


def response(text="billing", **overrides):
    return {"done": True, "message": {"content": text}, "prompt_eval_count": 10,
            "eval_count": 2, **overrides}


def model(transport, calls=12, context=32768):
    return ab.LocalModel("test:local", DIGEST, calls, 1, 512, context, transport)


def raises(kind, fn):
    try:
        fn()
    except kind:
        return
    raise AssertionError(f"expected {kind.__name__}")


def test_separate_compressor_token_units_and_failed_consumption():
    fake=ScriptedTransport()
    student=model(fake)
    ledger=SimpleNamespace(name="Qwen/Qwen3.5-0.8B",digest="b"*64,calls=[])
    class Compressor:
        model=ledger
        valid=True
        usage={"input_tokens":100,"output_tokens":30}
        def __call__(self, task, target, seed):
            self.model.calls.append({"usage":self.usage,"cost":None,"status":"ok" if self.valid else "failed"})
            self.last_receipt={"valid_attempt":self.valid,"disposition":"generated_unverified" if self.valid else "raw_fallback"}
            return task["source"]
    compressor=Compressor()
    task={"id":"units","source":"billing","query":"Service?","gold_answer":"billing"}
    student_unit=student.name+"@"+student.digest
    compressor_unit=ledger.name+"@"+ledger.digest
    for valid in (True,False):
        compressor.valid=valid
        compressor.usage={"input_tokens":100,"output_tokens":30} if valid else None
        fake.responses=[response() for _ in range(4 if valid else 3)]
        _,_,rows=ab.run_pilot([task],student,lambda task,seed:task["source"],lambda source,target:source,len,.75,42,
            {"tokenfold-generative":compressor})
        row=rows[-1]
        assert row["total_usage"] is None and row["billed_cost"] is None
        assert len(row["compression_model_calls"])==1
        assert row["usage_by_model"][compressor_unit]==compressor.usage
        assert len(row["model_calls"])==(1 if valid else 0)
        if valid:
            assert row["usage_by_model"][student_unit]=={"input_tokens":10,"output_tokens":2}
        else:
            assert row["outcome"]=="invalid" and student_unit not in row["usage_by_model"]
        total=ab.economics_summary(rows)["all_attempts"]["tokenfold-generative"]
        assert total["total_usage"] is None and total["usage_by_model"][compressor_unit]==compressor.usage
        assert total["fallbacks"]==(0 if valid else 1)
    compressor.model=SimpleNamespace(name=student.name,digest=student.digest,calls=[])
    before=len(student.calls)
    raises(ValueError,lambda:ab.run_pilot([task],student,lambda task,seed:task["source"],lambda source,target:source,len,.75,42,
        {"tokenfold-generative":compressor}))
    assert len(student.calls)==before


def test_byte_identical_controls_retain_variable_student_outcomes_and_usage():
    fake = ScriptedTransport()
    fake.responses = [response('billing'), response('billing with explanation'), response('billing')]
    runtime = model(fake)
    task = {'id':'control','source':'billing','query':'Service?','gold_answer':'billing'}
    _, _, rows = ab.run_pilot([task], runtime, lambda t,s:t['source'], lambda s,b:s, len, .75, 42)
    assert rows[0]['payload'] == rows[1]['payload'] and rows[1]['outcome'] == 'failure'
    total = ab.economics_summary(rows)['all_attempts']['tokenfold-lossless']
    controls = total['byte_identical_raw_controls']
    assert controls['attempts'] == controls['answer_disagreements'] == controls['outcome_disagreements'] == 1
    assert total['failures'] == 1 and total['total_usage'] == {'input_tokens':10,'output_tokens':2}
    assert len(runtime.calls) == 3  # No cache/retry, metering and negative evidence preserved.


def test_model_refusals_before_inference():
    fake = ScriptedTransport()
    fake.digest = "b" * 64
    raises(ValueError, lambda: model(fake))
    fake.digest = DIGEST
    fake.info = {"capabilities": ["embedding"], "model_info": {"architecture": "test"}}
    raises(ValueError, lambda: model(fake))
    fake.info = {"capabilities": ["completion"], "model_info": {"architecture": "test"}, "remote_host": "hosted"}
    raises(ValueError, lambda: model(fake))
    raises(ValueError, lambda: ab.LocalModel("test:cloud", DIGEST, 4, 1, 10, 100, fake))
    raises(ValueError, lambda: ab.LocalModel("test", DIGEST, 4, float("nan"), 10, 100, fake))
    assert not any(path == "chat" for path, _, _ in fake.requests)


def test_non_thinking_is_explicit_and_default_prompt_is_unchanged():
    fake=ScriptedTransport()
    fake.responses=[response()]
    runtime=ab.LocalModel("test:local",DIGEST,2,1,512,32768,fake,non_thinking=True)
    runtime.chat([{"role":"user","content":"source"}],42)
    body=next(body for path,body,_ in fake.requests if path=="chat")
    assert body["think"] is False
    default=ScriptedTransport();default.responses=[response()]
    model(default).chat([{"role":"user","content":"source"}],42)
    body=next(body for path,body,_ in default.requests if path=="chat")
    assert "think" not in body


def test_failed_calls_consume_budget_and_do_not_retry():
    fake = ScriptedTransport()
    fake.responses = [OSError("PRIVATE BODY")]
    runtime = model(fake, calls=1)
    raises(OSError, lambda: runtime.chat([{"content": "source"}], 17))
    raises(ValueError, lambda: runtime.chat([{"content": "source"}], 17))
    assert len(runtime.calls) == 1
    assert runtime.calls[0]["usage"] is None
    assert runtime.calls[0]["status"] == "failed"
    assert "PRIVATE" not in json.dumps(runtime.calls)
    assert sum(path == "chat" for path, _, _ in fake.requests) == 1


def test_unknown_usage_limits_and_tag_changes():
    fake = ScriptedTransport()
    fake.responses = [response(prompt_eval_count=None), response(done_reason="length")]
    runtime = model(fake)
    assert runtime.chat([{"role": "user", "content": "source"}], 17) == "billing"
    assert runtime.calls[0]["usage"] is None
    body = [body for path, body, _ in fake.requests if path == "chat"][0]
    assert body["stream"] is False
    assert body["options"] == {"temperature": 0, "seed": 17, "num_predict": 512, "num_ctx": 32768}
    raises(ValueError, lambda: runtime.chat([{"content": "source"}], 17))
    fake.digest = "b" * 64
    raises(ValueError, lambda: runtime.chat([{"content": "source"}], 17))
    assert len(runtime.calls) == 2
    raises(ValueError, lambda: runtime.chat([{"content": "x" * 32768}], 17))


def test_worker_deadline_and_safe_error():
    with patch.object(ab.subprocess, "run", side_effect=subprocess.TimeoutExpired("worker", 0.01)) as run:
        raises(subprocess.TimeoutExpired, lambda: ab.bounded_request("chat", {}, 0.01))
        assert run.call_count == 1
        assert run.call_args.kwargs["timeout"] == 0.01
    failed = subprocess.CompletedProcess([], 1, "SECRET", "SECRET")
    with patch.object(ab.subprocess, "run", return_value=failed):
        try:
            ab.bounded_request("chat", {}, 1)
        except ValueError as exc:
            assert str(exc) == "local request failed"
        else:
            raise AssertionError("worker failure accepted")
    raises(ValueError, lambda: ab.request_json("https://hosted.example", {}))
    assert ab.NoRedirect().redirect_request(None, None, 302, None, None, "https://hosted.example") is None


def test_pilot_pairing_economics_and_no_gold_leak():
    task = {"id": "t", "source": "billing was not rolled back", "query": "Which service?", "gold_answer": "billing"}
    fake = ScriptedTransport()
    fake.responses = [response(), response(), response("# Refined Observation\nbilling"), response("wrong")]
    runtime = model(fake)

    def acon(task, seed):
        # Scripted stand-in checks runner wiring, NOT official ACON or quality.
        assert set(task) == {"id", "source", "query", "gold_answer"}
        return runtime.chat([{"role": "user", "content": task["source"] + task["query"]}], seed)

    records, reports, rows = ab.run_pilot([task], runtime, acon, lambda src, target: src,
                                         len, 0.5, 17)
    assert reports["tokenfold-lossless"]["conditional_failure_rate"] == 0
    assert reports["acon-observation"]["conditional_failure_rate"] == 1
    assert rows[-1]["total_usage"] == {"input_tokens": 20, "output_tokens": 4}
    assert len(rows[-1]["model_calls"]) == 2
    assert rows[-1]["billed_cost"] is None
    assert rows[1]["target_met"] is False  # Never claim an unmet budget is met.
    assert records["tokenfold-lossless"][0] == records["acon-observation"][0]
    with tempfile.TemporaryDirectory() as tmp:
        for name, group in records.items():
            path = Path(tmp) / (name + ".jsonl")
            path.write_text("".join(json.dumps(r) + "\n" for r in group), encoding="utf-8")
            assert ab.rp.load_records(path) == group
    messages = ab.answer_messages({**task, "gold_answer": "DO_NOT_LEAK"}, task["source"])
    assert "DO_NOT_LEAK" not in json.dumps(messages)


def test_compressor_failure_is_invalid_not_a_quality_win():
    fake = ScriptedTransport()
    fake.responses = [response(), response()]
    task = {"id": "t", "source": "billing", "query": "Which service?", "gold_answer": "billing"}

    def unavailable(task, seed):
        raise ValueError("PRIVATE PROMPT")

    _, reports, rows = ab.run_pilot([task], model(fake), unavailable, lambda s, b: s, len, 0.5, 0)
    assert reports["acon-observation"]["paired_count"] == 0
    assert reports["acon-observation"]["invalid_count"] == 1
    assert reports["acon-observation"]["conditional_failure_rate"] is None
    assert "PRIVATE" not in json.dumps(rows)
    assert rows[-1]["total_usage"] is None
    summary = ab.economics_summary(rows)
    assert summary["complete"] is False
    assert not summary["common_valid_task_ids"]
    assert summary["all_attempts"]["acon-observation"]["invalid"] == 1
    assert summary["all_attempts"]["acon-observation"]["total_usage"] is None


def test_checked_in_pilot_tasks():
    tasks = ab.rp.load_tasks(Path(ab.__file__).with_name("acon_corpus"))
    assert len(tasks) == 3
    assert all(not task["critical_atoms"] for task in tasks)


def test_repack_requires_smaller_exact_recovery():
    source = "original source text"
    with patch.object(ab.rp.rb, "compress_tokenfold", return_value="frame") as compress, \
            patch.object(ab.rp.rb, "count_tokens", side_effect=len), \
            patch.object(ab.rp.rb, "_TOKENFOLD_BIN", "approved-binary"), \
            patch.object(ab.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, source.encode())) as decode:
        assert ab.repack_selected(source) == "frame"
        compress.assert_called_once_with(source, 1)
        assert decode.call_args.args[0] == ["approved-binary", "decode", "-"]
        decode.return_value.stdout = b"altered text"
        assert ab.repack_selected(source) == source
        decode.return_value.returncode = 1
        assert ab.repack_selected(source) == source
        compress.return_value = source + " expansion"
        decode.reset_mock()
        assert ab.repack_selected(source) == source
        decode.assert_not_called()
        compress.return_value = None
        raises(ValueError, lambda: ab.repack_selected(source))


def test_hybrid_counts_ranker_once_and_retains_selection_receipt():
    task = {"id": "t", "source": "original source", "query": "Which service?", "gold_answer": "billing"}
    fake = ScriptedTransport()
    fake.responses = [response() for _ in range(5)]
    runtime = model(fake)

    class Selector:
        last_receipt = {"used_scorer": True}

        def __call__(self, task, target, seed):
            runtime.chat([{"content": task["source"]}], seed)
            return "selected source"

    with patch.object(ab, "repack_selected", return_value="frame") as repack:
        _, reports, rows = ab.run_pilot([task], runtime, lambda t, s: "billing", lambda s, b: s,
                                      len, 0.5, 0, {"tokenfold-model-select-lossless": Selector()})
    hybrid = rows[-1]
    repack.assert_called_once_with("selected source")
    assert hybrid["payload"] == "frame" and hybrid["selected_payload_tokens"] == len("selected source")
    assert hybrid["selection_receipt"]["used_scorer"]
    assert len(hybrid["model_calls"]) == 2  # One scorer, one answer; no extra inference for repack.
    assert hybrid["total_usage"] == {"input_tokens": 20, "output_tokens": 4}
    assert reports["tokenfold-model-select-lossless"]["candidate_successes"] == 1


def test_acon_pin_refuses_wrong_revision_or_modified_prompt():
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        with patch.object(ab.subprocess, "check_output", return_value="wrong-revision"):
            raises(ValueError, lambda: ab.load_acon(root, None))
        path = root / "prompt.jinja"
        path.write_text("modified", encoding="utf-8")
        with patch.object(ab.subprocess, "check_output", side_effect=[ab.ACON_REVISION, "prompt.jinja\n", b"original"]):
            raises(ValueError, lambda: ab.load_acon(root, None))


def test_external_guideline_is_literal_template_data_only():
    raises(SystemExit, lambda: ab.main(["--run-live", "--acon-root", "unused", "--model", "test:local",
                                       "--acon-guideline-prompt-family", "smolagents"]))
    raises(ValueError, lambda: ab.load_acon(Path("unused"), None, prompt_family="unknown"))
    raises(ValueError, lambda: ab.load_acon(Path("unused"), None, "history", prompt_family="smolagents"))
    text = "Changed rule. {{ task }}\n{{ history }}\n{{ observation }}\n# Refined Observation"
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / "prompt.jinja"
        path.write_text(text, encoding="utf-8")
        template, fingerprint, _ = ab.observation_guideline(path)
        assert len(fingerprint) == 64
        assert template.render(task="TASK", history="HISTORY", observation="SOURCE").startswith("Changed rule. TASK")
        for bad in (text + "{{ task }}", text.replace("{{ history }}", ""),
                    text.replace("{{ task }}", "{{ task.upper() }}"), text + "{% include 'other' %}",
                    text + "{{ lipsum.__globals__ }}", text + "{{", "x" * 65537):
            path.write_text(bad, encoding="utf-8")
            raises(ValueError, lambda: ab.observation_guideline(path))
        raises(ValueError, lambda: ab.load_acon(Path(directory), None, "history", path))


def test_history_guideline_is_bounded_literal_template_data():
    text = "History rule. {{ task }}\n{{ history }}\n{{ prev_summary }}\n### REASONING\n### COMPLETED"
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / "history.jinja"
        path.write_text(text, encoding="utf-8")
        template, fingerprint, raw = ab.history_guideline(path)
        assert raw == path.read_bytes().decode("utf-8") and len(fingerprint) == 64
        assert template.render(task="TASK", history="HISTORY", prev_summary="PREVIOUS").startswith("History rule. TASK")
        for bad in (text + "{{ task }}", text.replace("{{ prev_summary }}", ""),
                    text.replace("{{ history }}", "{{ observation }}"),
                    text.replace("{{ task }}", "{{ task.upper() }}"),
                    text.replace("### COMPLETED", ""), text + "{% include 'other' %}",
                    text + "{{ lipsum.__globals__ }}", text + "{{", "x" * 65537):
            path.write_text(bad, encoding="utf-8")
            raises(ValueError, lambda: ab.history_guideline(path))
        raises(ValueError, lambda: ab.load_acon(Path(directory), None, history_guideline_path=path))


def test_failed_generation_fallback_is_invalid_and_accounted():
    task = {"id": "t", "source": "billing", "query": "Which service?", "gold_answer": "billing"}
    fake = ScriptedTransport()
    fake.responses = [response(), response(), response(), response()]
    runtime = model(fake)

    class FailedGenerator:
        last_receipt = {"valid_attempt": False, "disposition": "fell_back", "reason": "validation_failed"}

        def __call__(self, task, target, seed):
            runtime.chat([{"content": task["source"]}], seed)
            return task["source"]

    _, reports, rows = ab.run_pilot([task], runtime, lambda t, s: "billing", lambda s, b: s,
                                   len, 0.5, 0, {"tokenfold-generative-2b": FailedGenerator()})
    row = rows[-1]
    assert row["payload"] == task["source"] and row["outcome"] == "invalid"
    assert len(row["model_calls"]) == 1  # Failed generation is metered; no answering call follows.
    assert reports["tokenfold-generative-2b"]["paired_count"] == 0
    assert reports["tokenfold-generative-2b"]["contrastive_confidence"]["cfr_upper_bound"] is None
    totals = ab.economics_summary(rows)["all_attempts"]["tokenfold-generative-2b"]
    assert totals["fallbacks"] == 1 and totals["invalid"] == 1
    assert totals["billed_cost"] is None


def check_official_acon(root):
    fake = ScriptedTransport()
    fake.responses = [response("# Refined Observation\nbilling")]
    runtime = model(fake)
    compress, provenance = ab.load_acon(Path(root), runtime)
    task = {"query": "Which service was not rolled back?", "source": "billing was not rolled back", "gold_answer": "DO_NOT_LEAK"}
    assert compress(task, 17) == "billing"
    body = [body for path, body, _ in fake.requests if path == "chat"][0]
    assert [m["role"] for m in body["messages"]] == ["system", "user"]
    assert body["messages"][0]["content"]  # The official system prompt must reach the model.
    assert task["query"] in body["messages"][1]["content"]
    assert task["source"] in body["messages"][1]["content"]
    assert "DO_NOT_LEAK" not in json.dumps(body)
    assert provenance["revision"] == ab.ACON_REVISION
    assert len(provenance["source_and_prompt_sha256"]) > 2
    # Execute the actual pinned history/combined classes, not a lookalike mock.
    from replay_context import replay_source
    history = [{"role": "user", "content": "Record service billing."},
               {"role": "assistant", "content": "Recorded billing."},
               {"role": "user", "content": "Continue from that record."}]
    replay = {"query": task["query"], "history_messages": history,
              "observation": task["source"], "gold_answer": "DO_NOT_LEAK"}
    replay["source"] = replay_source(history, replay["observation"])
    original = json.dumps(history)
    for mode in ("history", "combined"):
        fake.responses = [response("# History Summary\nRecorded billing.")]
        if mode == "combined":
            fake.responses.append(response("# Refined Observation\nbilling"))
        before = len(fake.requests)
        compress_history, history_provenance = ab.load_acon(Path(root), runtime, mode=mode)
        payload = compress_history(replay, 17)
        requests = [body for path, body, _ in fake.requests[before:] if path == "chat"]
        assert len(requests) == (2 if mode == "combined" else 1)
        assert [m["role"] for m in requests[0]["messages"]] == ["user", "assistant", "user"]
        assert requests[0]["messages"][-1]["content"].startswith(history[-1]["content"] + "\n[COMPRESSION START]")
        assert "DO_NOT_LEAK" not in json.dumps(requests)
        assert "Recorded billing." in payload and json.dumps(history) == original
        assert history_provenance["history_transport"].endswith("no-assistant-prefill")
        assistant_final = {**replay, "history_messages": history[:-1]}
        assistant_final["source"] = replay_source(history[:-1], replay["observation"])
        before_calls = len(runtime.calls)
        raises(ValueError, lambda: compress_history(assistant_final, 17))
        assert len(runtime.calls) == before_calls and json.dumps(history) == original
    print("PASS pinned official history/combined role transport; assistant-final refused before inference")
    fake.responses = [response("# Refined Observation\nbilling")]
    qa, qa_provenance = ab.load_acon(Path(root), runtime, prompt_family="smolagents")
    assert qa(task, 17) == "billing"
    qa_body = next(body for path, body, _ in reversed(fake.requests) if path == "chat")
    assert "Do not make new knowledge or facts" in qa_body["messages"][1]["content"]
    assert "Mention that this is refined observation by another module" in qa_body["messages"][1]["content"]
    assert "DO_NOT_LEAK" not in json.dumps(qa_body)
    assert qa_body["messages"][0]["content"] == body["messages"][0]["content"]
    assert qa_provenance["prompt_family"] == "smolagents"
    assert "experiments/smolagents/prompts/context_opt/prompt_obs.jinja" in qa_provenance["source_and_prompt_sha256"]
    with tempfile.TemporaryDirectory() as directory:
        guideline = Path(directory) / "prompt.jinja"
        guideline.write_text("Changed rule. {{ task }} {{ history }} {{ observation }}\n# Refined Observation", encoding="utf-8")
        fake.responses = [response("# Refined Observation\nbilling")]
        external, evidence = ab.load_acon(Path(root), runtime, guideline=guideline)
        assert external(task, 17) == "billing"
        request = next(body for path, body, _ in reversed(fake.requests) if path == "chat")
        assert request["messages"][1]["content"].startswith("Changed rule.")
        assert request["messages"][0]["content"] == body["messages"][0]["content"]
        assert evidence["guidelines"] == "external-guideline-not-qualified-optimized"
        assert evidence["observation_guideline_sha256"] == ab.observation_guideline(guideline)[1]
        fake.responses = [response("# Refined Observation\nbilling")]
        external_qa, qa_evidence = ab.load_acon(Path(root), runtime, guideline=guideline, prompt_family="smolagents")
        assert external_qa(task, 17) == "billing"
        qa_request = next(body for path, body, _ in reversed(fake.requests) if path == "chat")
        assert qa_request["messages"][1]["content"].startswith("Changed rule.")
        assert "DO_NOT_LEAK" not in json.dumps(qa_request)
        assert qa_evidence["prompt_family"] == "smolagents"
        assert qa_evidence["guidelines"] == "external-guideline-not-qualified-optimized"
        assert qa_evidence["observation_guideline_sha256"] == evidence["observation_guideline_sha256"]
        assert "experiments/smolagents/prompts/context_opt/prompt_obs.jinja" in qa_evidence["source_and_prompt_sha256"]
    print("PASS official pinned ACON with scripted transport (not quality evidence)")
    if not ab.rp.rb._TOKENFOLD_BIN or not ab.rp.rb.TOKENIZER["is_exact"]:
        print("SKIP CLI integration: requires built CLI and tiktoken")
        return
    tasks = ab.rp.load_tasks(Path(ab.__file__).with_name("acon_corpus"))
    fake.responses = [r for task in tasks for r in (
        response(task["gold_answer"]), response(task["gold_answer"]),
        response("# Refined Observation\n" + task["gold_answer"]), response(task["gold_answer"]),
        response(task["gold_answer"]),
    )]
    original_model = ab.LocalModel
    with tempfile.TemporaryDirectory() as directory:
        readable_args = ["--run-live", "--acon-root", str(root), "--model", "test:local",
                         "--model-digest", DIGEST, "--context-tokens", "32768", "--max-calls", "15",
                         "--output-dir", directory, "--readable-logfold-arm"]
        with patch.object(ab, "LocalModel", side_effect=lambda *a: original_model(*a, transport=fake)), \
                patch.object(ab, "bounded_request", return_value={"version": "scripted-test"}):
            assert ab.main(readable_args) == 0
        report = json.loads((Path(directory) / "report.json").read_text(encoding="utf-8"))
        rows = json.loads((Path(directory) / "economics.json").read_text(encoding="utf-8"))
        assert report["runtime"]["readable_logfold_guideline"] == "tokenfold-logfold-json-presentation-v2"
        assert report["comparisons"]["tokenfold-logfold-json"]["paired_count"] == 3
        assert "readable_logfold.py" in report["implementation_sha256"]
        assert sum(len(row["model_calls"]) for row in rows) == 15
        assert all(row["compression_receipt"]["model_readability"] == "unqualified"
                   for row in rows if row["candidate"] == "tokenfold-logfold-json")
    with tempfile.TemporaryDirectory() as tmp:
        fake.responses = [r for task in tasks for r in (
            response(task["gold_answer"]), response(task["gold_answer"]),
            response("# Refined Observation\n" + task["gold_answer"]), response(task["gold_answer"]),
            response(task["gold_answer"]),
        )]
        bm25_args = readable_args.copy()
        bm25_args.remove("--readable-logfold-arm")
        bm25_args[bm25_args.index("--output-dir") + 1] = tmp
        bm25_args[bm25_args.index("--max-calls") + 1] = "15"
        bm25_args.append("--bm25-select-arm")
        with patch.object(ab, "LocalModel", side_effect=lambda *a: original_model(*a, transport=fake)), \
                patch.object(ab, "bounded_request", return_value={"version": "scripted-test"}):
            assert ab.main(bm25_args) == 0
        report = json.loads((Path(tmp) / "report.json").read_text())
        assert report["runtime"]["bm25_select_guideline"] == "tokenfold-caller-bm25-native-select-v1"
        assert report["comparisons"]["tokenfold-bm25-select"]["paired_count"] == 3
        rows = json.loads((Path(tmp) / "economics.json").read_text())
        assert all(row["compression_receipt"]["model_calls"] == 0
                   for row in rows if row["candidate"] == "tokenfold-bm25-select")
    fake.responses = [r for task in tasks for r in (
        response(task["gold_answer"]), response(task["gold_answer"]),
        response("# Refined Observation\n" + task["gold_answer"]), response(task["gold_answer"]),
        response("# Refined Observation\n" + task["gold_answer"]), response(task["gold_answer"]),
    )]
    original_model = ab.LocalModel
    with tempfile.TemporaryDirectory() as directory:
        guideline, output = Path(directory) / "prompt.jinja", Path(directory) / "run"
        guideline.write_text("Changed rule. {{ task }} {{ history }} {{ observation }}\n# Refined Observation", encoding="utf-8")
        args = ["--run-live", "--acon-root", str(root), "--model", "test:local",
                "--model-digest", DIGEST, "--context-tokens", "32768", "--max-calls", "18",
                "--output-dir", str(output), "--acon-observation-guideline", str(guideline),
                "--acon-guideline-prompt-family", "smolagents"]
        run_pilot = ab.run_pilot

        def changed_guideline(*a, **kw):
            result = run_pilot(*a, **kw)
            guideline.write_text("{{ invalid", encoding="utf-8")
            return result

        with patch.object(ab, "LocalModel", side_effect=lambda *a: original_model(*a, transport=fake)), \
                patch.object(ab, "bounded_request", return_value={"version": "scripted-test"}), \
                patch.object(ab, "run_pilot", side_effect=changed_guideline):
            assert ab.main(args) == 1  # Preserve attempts, invalidate changed guideline provenance.
        report = json.loads((output / "report.json").read_text(encoding="utf-8"))
        rows = json.loads((output / "economics.json").read_text(encoding="utf-8"))
        assert not report["acon_unchanged"]
        assert report["runtime"]["acon_observation_guideline_sha256"] == report["acon"]["external_observation_guideline"]["observation_guideline_sha256"]
        assert report["runtime"]["acon_observation_guideline_prompt_family"] == "smolagents"
        assert report["acon"]["external_observation_guideline"]["prompt_family"] == "smolagents"
        assert "prompt_family" not in report["acon"]  # Official AppWorld base unchanged.
        assert all(r["paired_count"] == 3 for r in report["comparisons"].values())
        assert set(report["comparisons"]) == {"tokenfold-lossless", "acon-observation", "acon-observation-guideline"}
        assert sum(len(row["model_calls"]) for row in rows) == 18
        assert len(ab.rp.load_records(output / "acon-observation-guideline.jsonl")) == 6
    fake.responses = [r for task in tasks for r in (
        response(task["gold_answer"]), response(task["gold_answer"]),
        response("# Refined Observation\n" + task["gold_answer"]), response(task["gold_answer"]),
    )]
    original_model = ab.LocalModel
    with tempfile.TemporaryDirectory() as tmp:
        args = ["--run-live", "--acon-root", str(root), "--model", "test:local",
                "--model-digest", DIGEST, "--context-tokens", "32768", "--output-dir", tmp]
        with patch.object(ab, "LocalModel", side_effect=lambda *a: original_model(*a, transport=fake)), \
                patch.object(ab, "bounded_request", return_value={"version": "scripted-test"}):
            assert ab.main(args) == 0
        report = json.loads((Path(tmp) / "report.json").read_text(encoding="utf-8"))
        rows = json.loads((Path(tmp) / "economics.json").read_text(encoding="utf-8"))
        assert all(r["paired_count"] == 3 for r in report["comparisons"].values())
        assert len(rows) == 9
        assert sum(len(row["model_calls"]) for row in rows) == 12
        for path in Path(tmp).glob("*.jsonl"):
            assert len(ab.rp.load_records(path)) == 6
        # Refuse overwriting existing evidence before making any additional call.
        raises(SystemExit, lambda: ab.main(args))
    # Validation must execute as validation, not through a misleading test-suite alias.
    fake.responses = [r for task in tasks for r in (
        response(task["gold_answer"]), response(task["gold_answer"]),
        response("# Refined Observation\n" + task["gold_answer"]), response(task["gold_answer"]),
    )]
    from freeze_campaign import freeze
    with tempfile.TemporaryDirectory() as directory:
        protocol_path, freeze_path = Path(directory) / "protocol.json", Path(directory) / "freeze.json"
        protocol = {"version": 1, "scope": "smoke", "arms": ["raw", "tokenfold-lossless", "acon-observation"],
                    "models": {"answering_and_compressor": "test:local@" + DIGEST}, "seeds": [0],
                    "runtime": {"acon_revision": ab.ACON_REVISION, "transport": "ollama-loopback-killable-subprocess",
                                "answering_roles": ["system", "user"], "assistant_prefill": False,
                                "scorer_guideline": None,
                                "generative_guideline": None, "headroom_revision": None, "campaign_split": "validation"},
                    "budgets": {"context_tokens": 32768, "output_tokens": 512, "max_calls": 12,
                                "timeout_seconds": 120, "ratio": 0.5},
                    "quality": {"max_cfr": .01, "max_success_loss": .01, "confidence": .95, "min_raw_success_clusters": 299},
                    "suites": [{"split": "validation", "workload": "json", "path": "eval/research/acon_corpus"}]}
        protocol_path.write_text(json.dumps(protocol), encoding="utf-8")
        freeze_path.write_text(json.dumps(freeze(protocol_path, Path(ab.__file__).resolve().parents[2])), encoding="utf-8")
        output = Path(directory) / "run"
        validation_args = ["--run-live", "--acon-root", str(root), "--model", "test:local", "--model-digest", DIGEST,
                "--context-tokens", "32768", "--output-dir", str(output), "--campaign-split", "validation",
                "--campaign-protocol", str(protocol_path), "--campaign-freeze", str(freeze_path)]
        with patch.object(ab, "LocalModel", side_effect=lambda *a: original_model(*a, transport=fake)), \
                patch.object(ab, "bounded_request", return_value={"version": "scripted-test"}):
            assert ab.main(validation_args) == 0
        report = json.loads((output / "report.json").read_text(encoding="utf-8"))
        assert report["campaign"]["suite_split"] == "validation" and "test_directory" not in report["campaign"]
        assert report["campaign_unchanged"]
    # Losing the executable after inference must retain metered attempts and fail closed.
    fake.responses = [r for task in tasks for r in (
        response(task["gold_answer"]), response(task["gold_answer"]),
        response("# Refined Observation\n" + task["gold_answer"]), response(task["gold_answer"]),
    )]
    read_bytes, run_pilot = Path.read_bytes, ab.run_pilot
    finished = False

    def finish(*a, **kw):
        nonlocal finished
        result = run_pilot(*a, **kw)
        finished = True
        return result

    def unavailable_binary(path):
        if finished and path == Path(ab.rp.rb._TOKENFOLD_BIN):
            raise FileNotFoundError("test executable disappeared")
        return read_bytes(path)

    with tempfile.TemporaryDirectory() as tmp:
        args[args.index("--output-dir") + 1] = tmp
        with patch.object(ab, "LocalModel", side_effect=lambda *a: original_model(*a, transport=fake)), \
                patch.object(ab, "bounded_request", return_value={"version": "scripted-test"}), \
                patch.object(ab, "run_pilot", side_effect=finish), \
                patch.object(Path, "read_bytes", unavailable_binary):
            assert ab.main(args) == 1
        report = json.loads((Path(tmp) / "report.json").read_text(encoding="utf-8"))
        assert report["tokenfold_binary_unchanged"] is False
        assert len(json.loads((Path(tmp) / "economics.json").read_text(encoding="utf-8"))) == 9
        assert len(list(Path(tmp).glob("*.jsonl"))) == 2
    print("PASS full pilot with real CLI/official ACON and scripted model; no live quality evidence")
    source = "".join(f"INFO stable-prefix service=example sequence={i} repeated explanatory context\n" for i in range(40))
    packed = ab.repack_selected(source)
    assert ab.rp.rb.count_tokens(packed) < ab.rp.rb.count_tokens(source)
    decoded = subprocess.run([ab.rp.rb._TOKENFOLD_BIN, "decode", "-"], input=packed.encode(), capture_output=True, timeout=30)
    assert decoded.returncode == 0 and decoded.stdout == source.encode()
    print("PASS hybrid repack with real CLI: strictly smaller and byte-exact recovery")


def profile_removed_accounting(task_path):
    """Measure only removed encoding work; not pipeline or workload superiority."""
    import hashlib
    import time
    import importlib.metadata
    if not ab.rp.rb.TOKENIZER["is_exact"]:
        raise ValueError("profile requires the existing exact tokenizer")
    data = Path(task_path).read_bytes()
    source = json.loads(data)["source"]
    start = time.perf_counter()
    baseline = ab.rp.rb.count_tokens(source)
    first_ms = (time.perf_counter() - start) * 1000
    samples = []
    for _ in range(100):
        start = time.perf_counter()
        first, second = ab.rp.rb.count_tokens(source), ab.rp.rb.count_tokens(source)
        if first != baseline or second != baseline:
            raise ValueError("token counts changed while profiling")
        samples.append((time.perf_counter() - start) * 1000)
    samples.sort()
    print(json.dumps({"kind": "removed-accounting-work-not-compression-win",
        "task_sha256": hashlib.sha256(data).hexdigest(), "source_sha256": hashlib.sha256(source.encode()).hexdigest(),
        "python": sys.version, "tiktoken": importlib.metadata.version("tiktoken"),
        "source_bytes": len(source.encode()), "tokens": baseline, "first_encode_ms": first_ms,
        "repeated_pairs": 100, "independent_sources": 1, "removed_p50_ms": samples[49], "removed_p95_ms": samples[94],
        "replacement": "reuse already counted identical source scalar", "cost_usd": None}, indent=2))


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--accounting-profile":
        profile_removed_accounting(sys.argv[2])
        raise SystemExit(0)
    tests = [value for key, value in list(globals().items()) if key.startswith("test_")]
    for test in tests:
        test()
        print("PASS", test.__name__)
    print(f"{len(tests)} offline checks passed; no model-quality claims.")
    if len(sys.argv) == 3 and sys.argv[1] == "--acon-root":
        check_official_acon(sys.argv[2])
