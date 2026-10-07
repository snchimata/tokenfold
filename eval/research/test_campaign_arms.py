"""Offline protocol tests for development corpus/replay/generated-summary arms."""

import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import acon_benchmark as ab
import build_campaign_corpus as corpus
import generate_observation as gen
import headroom_observation as hr
from replay_context import replay_source, validate_replay
from select_observation import source_context


class Model:
    def __init__(self, reply):
        self.reply, self.messages = reply, []

    def chat(self, messages, seed):
        self.messages.append(messages)
        if isinstance(self.reply, Exception):
            raise self.reply
        return self.reply


class CampaignArmTests(unittest.TestCase):
    def test_explicit_recovery_capture_preserves_invalid_payload_without_admitting_it(self):
        import sys
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); capture = root / "capture.json"
            provenance = {"revision": "a" * 40, "imported_module": "headroom/compression/universal.py"}
            with patch.object(hr, "checkout_provenance", return_value=provenance):
                arm = hr.HeadroomArm(root, "a" * 40, Path(sys.executable), root / "scratch", 5,
                                     capture_path=capture, guard_binary=root / "binary")
            response = {"payload": '["<<ccr:abcdef123456>>"]',
                        "receipt": {"healthy": False, "valid_attempt": False, "reason": "unimplemented-ccr-recovery"}}
            with patch.object(gen, "native_guard") as guard, patch.object(hr.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, json.dumps(response))) as worker:
                with self.assertRaisesRegex(ValueError, "degraded"):
                    arm({"source": "source", "query": "query", "gold_answer": "NEVER_EXPORT"}, 1, 0)
                saved = capture.read_bytes()
                self.assertEqual(json.loads(saved)["payload"], response["payload"])
                self.assertNotIn(b"NEVER_EXPORT", saved)
                self.assertEqual(guard.call_count, 2)
                with self.assertRaisesRegex(ValueError, "overwrite"):
                    arm({"source": "source", "query": "query"}, 1, 0)
                self.assertEqual(worker.call_count, 1)
                self.assertEqual(capture.read_bytes(), saved)
    def test_nested_recovery_preserves_wrapper_and_rejects_foreign_scope(self):
        marker = "<<ccr:abcdef123456 2_rows_offloaded>>"
        source = '{ "metadata": {"protected": 1}, "features": [{"id":1},{"id":1}] }'
        payload = json.dumps({"metadata": {"protected": 1}, "features": [marker]})
        recovered = '[{"id":1},{"id":1}]'
        self.assertEqual(hr.restore_authorized_rows(source, payload, recovered, ["features"], marker), source)
        for bad in (json.dumps({"metadata": {"protected": True}, "features": [marker]}),
                    json.dumps({"metadata": {"protected": 2}, "features": [marker]}),
                    payload.replace("abcdef123456", "bbbbbb123456")):
            with self.assertRaises(ValueError):
                hr.restore_authorized_rows(source, bad, recovered, ["features"], marker)
        with self.assertRaises(ValueError): hr.restore_authorized_rows(source, payload, '[{"id":1}]', ["features"], marker)
        with self.assertRaises(ValueError): hr.restore_authorized_rows(source, payload, recovered, ["missing"], marker)
    def test_cli_binding_preflight_stops_before_any_model_setup(self):
        task = {"query": "Latest ID?", "source": '[{"id":"a","time":2,"mag":5}]',
                "select_context": {"prefix": "[", "suffix": "]",
                    "groups": [{"id": "wrong", "text": '{"id":"a","time":2,"mag":5}'}]}}
        profile = {"record_path": [], "columns": {key: [key] for key in ("id", "time", "mag")},
                   "selection": {"id_column": "id", "rank_column": "time", "direction": "max", "tie": "id-ascending",
                                 "filters": [{"column": "mag", "op": "gte", "value": 4.5}]}}
        with tempfile.TemporaryDirectory() as root:
            root = Path(root); path = root / "profile.json"; path.write_text(json.dumps(profile))
            with patch.object(ab.rp, "load_tasks", return_value=[task]), patch.object(gen, "native_guard"), patch.object(ab, "LocalModel") as model:
                with self.assertRaises(SystemExit):
                    ab.main(["--run-live", "--acon-root", str(root), "--model", "test:local", "--generative-arm",
                             "--generative-comparison-profile", str(path), "--output-dir", str(root / "out")])
                model.assert_not_called()
    def test_comparison_profile_cli_refuses_missing_arm_and_unknown_keys_before_models(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            profile = root / "comparison.json"
            profile.write_text(json.dumps({"record_path": [], "columns": {"id": ["id"]}, "gold_answer": "DO_NOT_EXPORT"}))
            base = ["--run-live", "--acon-root", str(root), "--model", "test:local", "--output-dir", str(root / "out"),
                    "--generative-comparison-profile", str(profile)]
            with patch.object(ab, "LocalModel") as model:
                for args in (base, base + ["--generative-arm"]):
                    with self.assertRaises(SystemExit): ab.main(args)
                model.assert_not_called()
    def test_explicit_selection_coverage_filters_ties_and_missing_values(self):
        plan = {"id_column": "id", "rank_column": "time", "direction": "max", "tie": "id-ascending",
                "filters": [{"column": "mag", "op": "gte", "value": 4.5}, {"column": "depth", "op": "lte", "value": 70}]}
        rows = [{"id": "z", "mag": 5, "depth": 10, "time": 2},
                {"id": "a", "mag": 5, "depth": 10, "time": 2},
                {"id": "new", "mag": 5, "depth": None, "time": 3}]
        self.assertFalse(gen.selection_covered(rows, plan, ["z"]))
        self.assertTrue(gen.selection_covered(rows, plan, ["a", "z"]))
        self.assertFalse(gen.selection_covered(rows, {**plan, "filters": [{"column": "mag", "op": "gte", "value": 9}]}, ["a"]))
        with self.assertRaises(ValueError): gen.selection_covered(rows, {**plan, "rank_column": "typo"}, [])
        groups = [{"id": "a", "text": '{"id":"a","mag":5,"depth":10,"time":2}'},
                  {"id": "z", "text": ',{"id":"z","mag":5,"depth":10,"time":1}'}]
        task = {"source": "[" + "".join(g["text"] for g in groups) + "]", "query": "Latest qualifying ID?",
                "select_context": {"prefix": "[", "suffix": "]", "groups": groups}}
        model = Model(json.dumps({"summary": "z is latest", "source_ids": ["z"]}))
        with patch.object(gen, "native_guard"):
            arm = gen.GenerativeArm(model, Path("binary"), len, source_ids=True,
                comparison_record_path=[], comparison_columns={key: [key] for key in ("id", "mag", "depth", "time")},
                comparison_selection=plan)
            self.assertEqual(arm(task, 500, 0), task["source"])
        self.assertEqual(len(model.messages), 1)
        self.assertEqual(arm.last_receipt["reason"], "selection_coverage_failed")
        self.assertFalse(arm.last_receipt["valid_attempt"])
        # IDs alone are insufficient: caller labels can be swapped while source bytes stay exact.
        swapped = {**task, "select_context": {**task["select_context"], "groups": [
            {**groups[0], "id": "z"}, {**groups[1], "id": "a"}]}}
        before = len(model.messages)
        with patch.object(gen, "native_guard"):
            with self.assertRaisesRegex(ValueError, "does not contain"):
                arm(swapped, 500, 0)
        self.assertEqual(len(model.messages), before)
        model.reply = json.dumps({"summary": "a has time 2", "source_ids": ["a"]})
        with patch.object(gen, "native_guard"):
            selected_arm = gen.GenerativeArm(model, Path("binary"), len, source_ids=True,
                comparison_record_path=[], comparison_columns={key: [key] for key in ("id", "mag", "depth", "time")},
                comparison_selection=plan, preselect_comparison=True)
            selected_arm(task, 500, 0)
        request = json.loads(model.messages[-1][-1]["content"])
        self.assertEqual(list(request["groups"]), ["a"])
        self.assertEqual(len(request["source_derived_comparison_table"]), 2)
        self.assertEqual(selected_arm.last_receipt["preselection"]["group_ids"], ["a"])
    def test_comparison_and_candidate_duplicate_json_keys_fail_closed(self):
        for source in ('{"rows":[],"rows":[{"id":"a"}]}', '{"rows":[{"id":"a","id":"b"}]}'):
            with self.assertRaises(ValueError):
                gen.comparison_table(source, ["rows"], {"id": ["id"]})
        model = Model('{"summary":"first","summary":"second","source_ids":["whole-source"]}')
        task = {"source": "source " * 100, "query": "Summarize source."}
        with patch.object(gen, "native_guard"):
            arm = gen.GenerativeArm(model, Path("binary"), len, source_ids=True)
            self.assertEqual(arm(task, 400, 0), task["source"])
        self.assertFalse(arm.last_receipt["valid_attempt"])
        self.assertEqual(arm.last_receipt["reason"], "summary_schema_failed")
        self.assertNotIn("second", json.dumps(arm.last_receipt))
    def test_explicit_comparison_table_retains_all_records_and_nulls(self):
        source = json.dumps({"rows": [{"id": "b", "point": [1, 2, 3]}, {"id": "a"}]})
        columns = {"identity": ["id"], "depth": ["point", 2]}
        self.assertEqual(gen.comparison_table(source, ["rows"], columns),
                         [{"identity": "b", "depth": 3}, {"identity": "a", "depth": None}])
        for bad in ({"nested": []}, {"bad": [True]}, {"bad": ["point", -1]}):
            with self.assertRaises(ValueError): gen.comparison_table(source, ["rows"], bad)
        model = Model(json.dumps({"summary": "b has depth 3", "source_ids": ["whole-source"]}))
        task = {"source": source, "query": "Which depth?", "gold_answer": "DO_NOT_EXPORT"}
        with patch.object(gen, "native_guard"):
            arm = gen.GenerativeArm(model, Path("binary"), len, source_ids=True,
                comparison_record_path=["rows"], comparison_columns=columns)
            arm(task, 1, 0)  # Protected-budget skip; builder independently tested above.
            arm(task, 1000, 0)
        data = json.loads(model.messages[-1][-1]["content"])
        self.assertEqual(len(data["source_derived_comparison_table"]), 2)
        self.assertEqual(data["groups"]["whole-source"], source)
        self.assertNotIn("DO_NOT_EXPORT", json.dumps(model.messages))
    def test_independent_cli_allowance_failure_retains_usage_and_prevents_retry(self):
        from types import SimpleNamespace
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory); approval=root/"approval.json"
            approval.write_text(json.dumps({"model_revision":"Qwen/Qwen3.5-0.8B"}))
            model=SimpleNamespace(name="Qwen/Qwen3.5-0.8B",calls=[],max_calls=2,stop_reason=None,verify=lambda:None)
            arm=gen.CliGenerativeArm(model,root/"binary",approval,root/"scratch",usage_allowances=(16384,128))
            def worker(args,**kwargs):
                Path(args[-1]).write_text(json.dumps({"disposition":"generated_unverified","runtime_invoked":True,
                    "inference_usage":{"input_tokens":20,"output_tokens":129},"wall_ms":1}))
                return subprocess.CompletedProcess(args,0,stdout="REJECTED",stderr="")
            with patch.object(gen.subprocess,"run",side_effect=worker) as run:
                with self.assertRaises(ValueError): arm(self.task(),20,0)
                self.assertFalse(arm.last_receipt["valid_attempt"])
                with self.assertRaises(ValueError): arm(self.task(),20,0)
                self.assertEqual(run.call_count,1)
            self.assertEqual(model.calls[0]["usage"],{"input_tokens":20,"output_tokens":129})
            self.assertTrue(model.calls[0]["attempted"])
            self.assertEqual(model.calls[0]["reason"],"reported_usage_allowance_exceeded")
            self.assertNotIn("REJECTED",json.dumps(model.calls))

    def test_restart_audit_reports_miss_and_mismatch_without_exporting_content(self):
        from types import SimpleNamespace
        import sys
        source = json.dumps([{"secret_test_marker": "DO_NOT_EXPORT", "id": 1}])
        stored = SimpleNamespace(original_content=source)
        keys = ["abcdef123456"]
        store = SimpleNamespace(retrieve=lambda key: stored)
        modules = {"headroom.ccr.tool_injection": SimpleNamespace(
                       CCRToolInjector=lambda: SimpleNamespace(scan_for_markers=lambda messages: keys)),
                   "headroom.cache.compression_store": SimpleNamespace(get_compression_store=lambda: store)}
        request = {"source": source, "payload": "<<ccr:abcdef123456>>"}
        with patch.dict(sys.modules, modules):
            result = hr.audit_store_restart(request)
            self.assertTrue(result["retrievals"][0]["source_matches"])
            nested = hr.audit_store_restart({**request, "source": json.dumps({"features": json.loads(source)}),
                                            "source_row_path": ["features"]})
            self.assertTrue(nested["retrievals"][0]["source_matches"])
            self.assertEqual(nested["source_row_path"], ["features"])
            for path in (["missing"], [True], ["features", -1]):
                with self.assertRaises(ValueError):
                    hr.audit_store_restart({**request, "source_row_path": path})
            self.assertNotIn("DO_NOT_EXPORT", json.dumps(result))
            stored.original_content = '[{"id":2}]'
            self.assertFalse(hr.audit_store_restart(request)["retrievals"][0]["source_matches"])
            store.retrieve = lambda key: None
            self.assertEqual(hr.audit_store_restart(request)["retrievals"][0]["status"], "miss")
            with self.assertRaises(ValueError):
                hr.audit_store_restart({**request, "source": '["<<ccr:abcdef123456>>"]'})
            keys.clear()
            with self.assertRaises(ValueError):
                hr.audit_store_restart(request)

    def test_recovery_audit_preserves_duplicate_rows_and_reports_mirror_misses(self):
        source = json.dumps([{"id": 1}, {"id": 1}, {"id": 2}])
        payload = json.dumps([{"id": 1}, {"_ccr_dropped": "<<ccr:abcdef123456 2_rows_offloaded>>"}])
        recovered = source
        result = hr.audit_recovery(source, payload, ["abcdef123456"] * 2,
            lambda key: recovered, lambda key: recovered, lambda key: None)
        self.assertTrue(result["exact_row_multiset_restored"])
        self.assertEqual(len(result["retrievals"]), 1)
        self.assertTrue(result["retrievals"][0]["python_mirror_matches"])
        self.assertFalse(result["retrievals"][0]["reopened_store_matches"])
        result = hr.audit_recovery(source, payload, ["abcdef123456"],
            lambda key: json.dumps([{"id": 2}]), lambda key: None, lambda key: None)
        self.assertFalse(result["exact_row_multiset_restored"])
        wrapped = payload + '\n<headroom:tool_digest sha256="0123456789abcdef">'
        self.assertTrue(hr.audit_recovery(source, wrapped, ["abcdef123456"],
            lambda key: recovered, lambda key: recovered, lambda key: recovered)["exact_row_multiset_restored"])
        with self.assertRaises(ValueError):
            hr.audit_recovery(source, payload + " uncounted text", ["abcdef123456"], None, None, None)
        outside = json.dumps([{"id": 1}, {"id": "outside-source"}])
        self.assertFalse(hr.audit_recovery(source, outside, ["abcdef123456"],
            lambda key: recovered, lambda key: recovered, lambda key: recovered)["exact_row_multiset_restored"])
        result = hr.audit_recovery(source, payload, ["abcdef123456"],
            lambda key: json.dumps([{"id": 1}, {"id": "outside-source"}]), lambda key: None, lambda key: None)
        self.assertFalse(result["exact_row_multiset_restored"])
        with self.assertRaises(ValueError):
            hr.audit_recovery(source, payload, [], lambda key: recovered, None, None)
        with self.assertRaises(ValueError):
            hr.audit_recovery('["<<ccr:abcdef123456>>"]', payload, ["abcdef123456"], None, None, None)

    def test_history_guideline_requires_explicit_replay_arms(self):
        with patch.object(ab.rp, "load_tasks") as load:
            with self.assertRaises(SystemExit):
                ab.main(["--run-live", "--acon-root", "unused", "--model", "test:local",
                         "--output-dir", "unused", "--acon-history-guideline", "unused"])
            load.assert_not_called()

    def task(self):
        return {"source": "RULE:billing was NOT retried; many unrelated words here." + " background" * 40,
                "query": "Which service failed?", "gold_answer": "DO_NOT_EXPORT_GOLD",
                "select_context": {"prefix": "RULE:", "groups": [
                    {"id": "required", "text": "billing was NOT retried;", "required": True},
                    {"id": "data", "text": " many unrelated words here."},
                    {"id": "noise", "text": " background" * 40}]}}

    def test_cli_generation_exports_source_only_and_meters_failed_attempts(self):
        class Local:
            name="test:model"
            max_calls=2
            def __init__(self): self.calls=[]
            def verify(self): pass
        model=Local()
        with tempfile.TemporaryDirectory() as root:
            root=Path(root); approval=root/"approval.json"
            approval.write_text(json.dumps({"model_revision":model.name}))
            arm=gen.CliGenerativeArm(model,root/"tokenfold.exe",approval,root/"worker")
            def run(args,**kwargs):
                assert "DO_NOT_EXPORT_GOLD" not in kwargs["input"]
                receipt={"disposition":"raw_fallback","runtime_invoked":True,"fallback_reason":"incomplete_response",
                         "inference_usage":{"input_tokens":100,"output_tokens":50},"wall_ms":3}
                Path(args[-1]).write_text(json.dumps(receipt))
                return subprocess.CompletedProcess(args,0,stdout=self.task()["source"],stderr="")
            with patch.object(gen.subprocess,"run",side_effect=run):
                self.assertEqual(arm(self.task(),20,42),self.task()["source"])
            self.assertFalse(arm.last_receipt["valid_attempt"])
            self.assertEqual(model.calls[0]["usage"],{"input_tokens":100,"output_tokens":50})
            self.assertIsNone(model.calls[0]["cost"])
            with patch.object(gen.subprocess,"run") as worker:
                with self.assertRaisesRegex(ValueError, "halted"):arm(self.task(),20,42)
                worker.assert_not_called()
            fresh = gen.CliGenerativeArm(Local(),root/"tokenfold.exe",approval,root/"fresh-worker")
            approval.write_text(json.dumps({"model_revision":"changed"}))
            with self.assertRaisesRegex(ValueError, "approval changed"):fresh(self.task(),20,42)
            self.assertEqual(len(model.calls),1)

    def test_cli_generation_halts_uncertain_shared_ledger_but_not_budget_fallbacks(self):
        class Local:
            name="test:model"
            max_calls=2
            def __init__(self): self.calls=[]; self.stop_reason=None
            def verify(self): pass
        usage={"input_tokens":100,"output_tokens":50}
        receipts=[
            ({"disposition":"raw_fallback","runtime_invoked":True,"fallback_reason":"incomplete_response","inference_usage":usage},True,1),
            ({"disposition":"raw_fallback","runtime_invoked":True,"fallback_reason":"over_budget","inference_usage":usage},False,1),
            ({"disposition":"raw_fallback","runtime_invoked":False,"fallback_reason":"already_within_budget"},False,0),
            ({"disposition":"raw_fallback","runtime_invoked":False,"fallback_reason":"already_within_budget","inference_usage":usage},True,1),
            ({"disposition":"generated_unverified","runtime_invoked":False,"inference_usage":usage},True,1),
            ({"disposition":"generated_unverified","inference_usage":usage},True,1),
            ({"disposition":[],"runtime_invoked":True,"inference_usage":usage},True,1),
            (None,True,1),
        ]
        for receipt, halted, consumed in receipts:
            with self.subTest(receipt=receipt), tempfile.TemporaryDirectory() as root:
                root=Path(root); model=Local(); approval=root/"approval.json"
                approval.write_text(json.dumps({"model_revision":model.name}))
                arm=gen.CliGenerativeArm(model,root/"tokenfold.exe",approval,root/"worker")
                def run(args,**kwargs):
                    if receipt is None:raise subprocess.TimeoutExpired(args,40)
                    Path(args[-1]).write_text(json.dumps(receipt))
                    return subprocess.CompletedProcess(args,0,stdout=self.task()["source"],stderr="")
                with patch.object(gen.subprocess,"run",side_effect=run):
                    contradictory = receipt is not None and receipt.get("runtime_invoked") is False and (
                        receipt.get("inference_usage") is not None or receipt.get("disposition") == "generated_unverified")
                    if receipt is None or not isinstance(receipt.get("disposition"),str) or "runtime_invoked" not in receipt or contradictory:
                        with self.assertRaises((ValueError,subprocess.TimeoutExpired)):arm(self.task(),20,42)
                    else:arm(self.task(),20,42)
                self.assertEqual(len(model.calls),consumed)
                self.assertEqual(arm.stop_reason is not None,halted)
                self.assertEqual(model.stop_reason is not None,halted)
                if consumed:
                    self.assertEqual(model.calls[0]["usage"],usage if receipt is not None else None)
                    self.assertEqual(model.calls[0]["status"],"failed" if halted else "ok")
                    if contradictory:
                        self.assertIsNone(model.calls[0]["attempted"])
                        self.assertEqual(model.calls[0]["reason"],"inconsistent_runtime_receipt")
                if halted:
                    with patch.object(gen.subprocess,"run") as worker:
                        with self.assertRaisesRegex(ValueError,"halted"):arm(self.task(),20,42)
                        worker.assert_not_called()
                    self.assertEqual(len(model.calls),consumed)
        with tempfile.TemporaryDirectory() as root:
            root=Path(root); model=Local(); approval=root/"approval.json"
            approval.write_text(json.dumps({"model_revision":model.name}))
            arm=gen.CliGenerativeArm(model,root/"tokenfold.exe",approval,root/"worker")
            with patch.object(model,"verify",side_effect=ValueError("unavailable")),patch.object(gen.subprocess,"run") as worker:
                with self.assertRaises(ValueError):arm(self.task(),20,42)
                worker.assert_not_called()
            self.assertEqual(model.stop_reason,"model_validation_failed")
            self.assertEqual(model.calls,[])

    def test_corpus_is_stable_development_data_and_exactly_grouped(self):
        for split in ("train", "validation", "test"):
            for workload in corpus.WORKLOADS:
                task = corpus.make_task(workload, split, 1)
                self.assertEqual(task, corpus.make_task(workload, split, 1))
                self.assertIn(task["gold_answer"], task["source"])
                self.assertEqual(task["critical_atoms"], [])
                source_context(task)
                validate_replay(task)
                self.assertNotEqual(task["source"], corpus.make_task(workload, split, 2)["source"])
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "corpus"
            self.assertEqual(corpus.main(["--output-dir", str(output)]), 0)
            self.assertEqual(len(ab.rp.load_tasks(output / "test")), 5)
            with self.assertRaises(FileExistsError):
                corpus.main(["--output-dir", str(output)])

    def test_replay_refuses_opaque_or_inconsistent_history(self):
        for bad in ([], [{"role": "tool", "content": "data"}],
                    [{"role": "user", "content": "data", "tool_calls": []}]):
            with self.assertRaises(ValueError):
                replay_source(bad, "observation")
        task = corpus.make_task("multi-turn", "test", 1)
        task["history_messages"][0]["content"] += " changed"
        with self.assertRaises(ValueError):
            validate_replay(task)

    def test_generated_summary_keeps_protection_and_never_receives_gold(self):
        task = self.task()
        reply = json.dumps({"summary": "Routine context.", "citations": [{"id": "data", "quote": "unrelated"}]})
        model = Model(reply)
        with patch.object(gen, "native_guard") as guard:
            arm = gen.GenerativeArm(model, Path("binary"), len)
            payload = arm(task, 400, 0)
        protected = "RULE:billing was NOT retried;"
        self.assertTrue(payload.startswith(protected))
        data = json.loads(payload[len(protected):])
        self.assertEqual(data["summary_unverified"], "Routine context.")
        self.assertEqual(data["source_evidence"], [{"id": "data", "text": " many unrelated words here."}])
        self.assertEqual(arm.last_receipt["evidence_group_ids"], ["data"])
        self.assertEqual(arm.last_receipt["disposition"], "generated")
        self.assertIn("unverified", arm.last_receipt["semantic_verification"])
        self.assertNotIn(task["gold_answer"], json.dumps(model.messages))
        self.assertEqual(len(model.messages), 1)
        self.assertIn("self-contained factual statements, not a bare answer", model.messages[0][0]["content"])
        self.assertIn("entity-to-attribute/action relationships", model.messages[0][0]["content"])
        self.assertEqual(arm.last_receipt["guideline"], "tokenfold-generated-observation-research-v3")
        self.assertEqual(guard.call_count, 3)

    def test_failures_fallback_without_leaking_rejected_reply(self):
        task = self.task()
        replies = ["PRIVATE PROVIDER BODY", ValueError("PRIVATE PROVIDER BODY"),
                   json.dumps({"summary": "PRIVATE", "citations": [{"id": "invented", "quote": "unrelated"}]}),
                   json.dumps({"summary": "PRIVATE", "citations": [{"id": "data", "quote": "invented"}]}),
                   json.dumps({"summary": "PRIVATE", "citations": [], "extra": True})]
        with patch.object(gen, "native_guard"):
            for reply in replies:
                arm = gen.GenerativeArm(Model(reply), Path("binary"), len)
                self.assertEqual(arm(task, 50, 0), task["source"])
                self.assertFalse(arm.last_receipt["valid_attempt"])
                self.assertNotIn("PRIVATE", json.dumps(arm.last_receipt))

    def test_source_evidence_is_literal_ordered_deduplicated_and_budgeted(self):
        groups = [{"id": "a", "text": "[LineId 1] first=001\r\n"},
                  {"id": "b", "text": "[LineId 2] last=002"},
                  {"id": "noise", "text": "unrelated " * 200}]
        task = {"source": "".join(g["text"] for g in groups), "query": "Which record?",
                "gold_answer": "DO_NOT_EXPORT", "select_context": {"groups": groups}}
        reply = json.dumps({"summary": "Short unverified summary.", "citations": [
            {"id": "b", "quote": "002"}, {"id": "a", "quote": "001"}, {"id": "b", "quote": "002"}]})
        model = Model(reply)
        with patch.object(gen, "native_guard") as guard:
            arm = gen.GenerativeArm(model, Path("binary"), len)
            payload = arm(task, 400, 0)
            self.assertEqual(json.loads(payload)["source_evidence"], groups[:2])
            self.assertEqual(arm.last_receipt["evidence_group_ids"], ["a", "b"])
            self.assertEqual(guard.call_args.args[1], payload)
            self.assertEqual(arm.last_receipt["candidate_budget"]["payload_tokens"], len(payload))
            counted = []
            arm.count_tokens = lambda text: counted.append(text) or len(text)
            self.assertNotIn("DO_NOT_EXPORT", json.dumps(model.messages))
            self.assertEqual(arm(task, 50, 0), task["source"])
            self.assertEqual(arm.last_receipt["reason"], "over_budget")
            self.assertTrue(arm.last_receipt["valid_attempt"])
            self.assertEqual(counted.count(payload), 1)
            self.assertEqual(arm.last_receipt["candidate_budget"]["target_tokens"], 50)
            self.assertGreater(arm.last_receipt["candidate_budget"]["payload_tokens"], 50)
            model.reply = json.dumps({"summary": "Short summary.", "citations": [
                {"id": "noise", "quote": "unrelated"}]})
            self.assertEqual(arm(task, 4000, 0), task["source"])
            self.assertEqual(arm.last_receipt["reason"], "not_smaller")

    def test_structured_generation_is_bounded_and_still_checks_literal_quotes(self):
        from test_acon_benchmark import ScriptedTransport, model, response
        transport = ScriptedTransport()
        transport.responses = [response(json.dumps({"summary": "Routine context.",
                                                   "citations": [{"id": "data", "quote": "invented"}]}))]
        runtime = model(transport)
        with patch.object(gen, "native_guard"):
            arm = gen.GenerativeArm(runtime, Path("binary"), len, structured=True)
            self.assertEqual(arm(self.task(), 50, 0), self.task()["source"])
        request = next(body for path, body, _ in transport.requests if path == "chat")
        self.assertEqual(arm.last_receipt["guideline"], "tokenfold-generated-observation-structured-v4")
        self.assertIn("self-contained factual statements", request["messages"][0]["content"])
        schema = request["format"]
        self.assertEqual(schema["properties"]["citations"]["items"]["properties"]["id"]["enum"], ["data", "noise"])
        self.assertFalse(arm.last_receipt["valid_attempt"])
        self.assertEqual(arm.last_receipt["reason"], "summary_citation_failed")
        self.assertEqual(len(runtime.calls), 1)
        transport.responses = [response("OK")]
        runtime.chat([{"role": "user", "content": "Return OK"}], 0)
        self.assertNotIn("format", transport.requests[-1][1])

    def test_protected_budget_and_no_growth_fallback(self):
        task = self.task()
        model = Model("never called")
        with patch.object(gen, "native_guard"):
            arm = gen.GenerativeArm(model, Path("binary"), len)
            self.assertEqual(arm(task, 1, 0), task["source"])
            self.assertEqual(model.messages, [])
            self.assertEqual(arm.last_receipt["reason"], "protected_budget")
            model.reply = json.dumps({"summary": "long " * 100, "citations": [{"id": "data", "quote": "words"}]})
            self.assertEqual(arm(task, 500, 0), task["source"])
            self.assertEqual(arm.last_receipt["reason"], "not_smaller")
            self.assertTrue(arm.last_receipt["valid_attempt"])

    def test_source_id_generation_compiles_literal_evidence_and_rejects_bad_ids(self):
        task = self.task()
        model = Model(json.dumps({"summary": "Routine context.", "source_ids": ["data", "data"]}))
        with patch.object(gen, "native_guard"):
            arm = gen.GenerativeArm(model, Path("binary"), len, source_ids=True)
            payload = arm(task, 400, 0)
            protected = "RULE:billing was NOT retried;"
            evidence = json.loads(payload[len(protected):])["source_evidence"]
            self.assertEqual(evidence, [{"id": "data", "text": " many unrelated words here."}])
            self.assertEqual(arm.last_receipt["citations"], [{"id": "data", "quote": evidence[0]["text"]}])
            self.assertEqual(arm.last_receipt["guideline"], gen.SOURCE_IDS_GUIDELINE)
            self.assertEqual(arm.last_receipt["citation_origin"], "native-complete-source-groups-not-model-quotes")
            self.assertNotIn(task["gold_answer"], json.dumps(model.messages))
            self.assertEqual(arm(task, 30, 0), task["source"])
            self.assertEqual(arm.last_receipt["reason"], "over_budget")
            for bad in (["invented"], ["required"], [None], []):
                model.reply = json.dumps({"summary": "PRIVATE", "source_ids": bad})
                self.assertEqual(arm(task, 400, 0), task["source"])
                self.assertFalse(arm.last_receipt["valid_attempt"])
                self.assertNotIn("PRIVATE", json.dumps(arm.last_receipt))
            model.reply = json.dumps({"summary": "PRIVATE", "source_ids": ["data"], "citations": []})
            self.assertEqual(arm(task, 400, 0), task["source"])
            self.assertFalse(arm.last_receipt["valid_attempt"])

    def test_structured_source_ids_authorize_only_original_groups(self):
        from test_acon_benchmark import ScriptedTransport, model, response
        transport = ScriptedTransport()
        transport.responses = [response(json.dumps({"summary": "Context.", "source_ids": ["data"]}))]
        runtime = model(transport)
        with patch.object(gen, "native_guard"):
            arm = gen.GenerativeArm(runtime, Path("binary"), len, source_ids=True, structured=True)
            arm(self.task(), 400, 0)
        request = next(body for path, body, _ in transport.requests if path == "chat")
        self.assertEqual(request["format"]["properties"]["source_ids"]["items"]["enum"], ["data", "noise"])
        self.assertNotIn("citations", request["format"]["properties"])
        self.assertTrue(arm.last_receipt["valid_attempt"])

    def test_native_secret_guard_runs_before_model(self):
        if not ab.rp.rb._TOKENFOLD_BIN:
            self.skipTest("built Tokenfold CLI unavailable")
        binary = Path(ab.rp.rb._TOKENFOLD_BIN)
        task = self.task()
        for field in ("source", "query"):
            bad = {**task, field: "Authorization: Bearer abcDEF123.token-value"}
            if field == "source":
                bad.pop("select_context")
            model = Model("never called")
            with self.assertRaises(ValueError):
                gen.GenerativeArm(model, binary, len)(bad, 50, 0)
            self.assertEqual(model.messages, [])
        with self.assertRaises(ValueError):
            gen.native_guard(binary, "sk-ABCDEFGHIJ1234567890abcdefgh", "query")
        model = Model(json.dumps({"summary": "sk-ABCDEFGHIJ1234567890abcdefgh",
                                 "citations": [{"id": "data", "quote": "words"}]}))
        arm = gen.GenerativeArm(model, binary, len)
        self.assertEqual(arm(task, 100, 0), task["source"])
        self.assertFalse(arm.last_receipt["valid_attempt"])
        self.assertNotIn("sk-", json.dumps(arm.last_receipt))

    def test_headroom_worker_is_bounded_and_does_not_inherit_credentials(self):
        import os
        import sys
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.object(hr, "checkout_provenance", return_value={"revision": "a" * 40}):
                arm = hr.HeadroomArm(root, "a" * 40, Path(sys.executable), root / "scratch", 5)
            response = {"payload": "source", "receipt": {"healthy": True}}
            with patch.dict(os.environ, {"OPENROUTER_API_KEY": "do-not-inherit"}), \
                    patch.object(hr.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, json.dumps(response))) as run:
                self.assertEqual(arm({"source": "source"}, 1, 0), "source")
                self.assertEqual(run.call_args.kwargs["timeout"], 5)
                environment = run.call_args.kwargs["env"]
                self.assertNotIn("OPENROUTER_API_KEY", environment)
                self.assertEqual(environment["HF_HUB_OFFLINE"], "1")
                response["receipt"]["healthy"] = False
                run.return_value.stdout = json.dumps(response)
                with self.assertRaises(ValueError):
                    arm({"source": "source"}, 1, 0)
                run.side_effect = subprocess.TimeoutExpired("worker", 5)
                with self.assertRaises(subprocess.TimeoutExpired):
                    arm({"source": "source"}, 1, 0)

    def test_smart_worker_request_receipt_and_artifact_identity(self):
        import hashlib
        import sys
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            artifact = root / "_core.pyd"
            artifact.write_bytes(b"test-artifact-not-loaded")
            provenance = {"revision": "a" * 40, "imported_module": "headroom/transforms/smart_crusher.py"}
            with patch.object(hr, "checkout_provenance", side_effect=lambda *args: dict(provenance)):
                arm = hr.HeadroomArm(root, "a" * 40, Path(sys.executable), root / "scratch", 5, artifact)
                self.assertTrue(arm.unchanged())
                receipt = {"healthy": False, "valid_attempt": False, "reason": "unimplemented-ccr-recovery"}
                response = {"payload": "<<ccr:abcd>>", "receipt": receipt}
                with patch.object(hr.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, json.dumps(response))) as run:
                    with self.assertRaises(ValueError):
                        arm({"source": "source", "query": "query", "gold_answer": "NEVER_EXPORT"}, 1, 0)
                    request = json.loads(run.call_args.kwargs["input"])
                    self.assertEqual(request["query"], "query")
                    self.assertNotIn("NEVER_EXPORT", run.call_args.kwargs["input"])
                    self.assertEqual(request["native_core_sha256"], hashlib.sha256(artifact.read_bytes()).hexdigest())
                    self.assertEqual(arm.last_receipt, receipt)
                artifact.write_bytes(b"changed")
                self.assertFalse(arm.unchanged())
                with self.assertRaisesRegex(ValueError, "artifact changed"):
                    hr.smart_worker(request)

    def test_comparator_output_guards_precede_answer_export(self):
        from test_acon_benchmark import ScriptedTransport, model, response
        for name in ("headroom-smartcrusher-default", "acon-qa-observation",
                     "acon-history-guideline", "acon-combined-history-guideline"):
            transport = ScriptedTransport()
            transport.responses = [response("gold")] * 3
            runtime = model(transport)
            task = {"id": "guard-test", "source": "safe source", "query": "query", "gold_answer": "gold"}
            with patch.object(gen, "native_guard", side_effect=ValueError("unsafe comparator output")) as guard:
                records, reports, rows = ab.run_pilot([task], runtime, lambda *a: "safe",
                    lambda *a: "safe", len, .5, 42, {name: lambda *a: "unsafe-output"})
            self.assertEqual(guard.call_args.args[1:], ("unsafe-output", "query"))
            self.assertEqual(records[name][1]["outcome"], "invalid")
            self.assertEqual(len(runtime.calls), 3)
            self.assertNotIn("unsafe-output", json.dumps(transport.requests))



def check_native_smart(root, python, core):
    """Optional real pinned-native canary; does not download weights or call a provider."""
    with tempfile.TemporaryDirectory() as directory:
        arm = hr.HeadroomArm(Path(root), "1cf496612e781ef8d67ff87ee4f78037492f0bc7",
                             Path(python), Path(directory) / "worker", 30, Path(core))
        source = json.dumps([{"id": i, "name": "Alice" if i == 33 else "User" + str(i),
                              "status": "ready"} for i in range(64)])
        payload = arm({"source": source, "query": "Find Alice.", "gold_answer": "NEVER_EXPORT"}, 100, 42)
        assert "Alice" in payload and len(payload.encode()) < len(source.encode())
        assert arm.last_receipt["healthy"] and arm.last_receipt["valid_attempt"]
        assert arm.last_receipt["transforms"] and arm.unchanged()
    print("PASS real pinned SmartCrusher tool snapshot; not live model quality or CCR qualification")


def check_native_recovery(root, python, core):
    """Synthetic protocol canary only; no answering model or qualification claim."""
    import hashlib
    import os
    with tempfile.TemporaryDirectory() as directory:
        directory = Path(directory)
        arm = hr.HeadroomArm(Path(root), "1cf496612e781ef8d67ff87ee4f78037492f0bc7",
                             Path(python), directory / "worker", 30, Path(core))
        source = json.dumps([{"id": i, "status": "ok"} for i in range(50)])
        request = {"root": str(arm.root), "source": source, "query": "Inspect current records.",
                   "native_core": str(arm.native_core),
                   "native_core_sha256": hashlib.sha256(arm.native_core.read_bytes()).hexdigest(),
                   "audit_recovery": True, "audit_force_lossy": True}
        environment = {k: os.environ[k] for k in ("SystemRoot", "WINDIR", "USERPROFILE", "HOME", "TEMP", "TMP")
                       if k in os.environ}
        environment.update(HF_HUB_OFFLINE="1", TRANSFORMERS_OFFLINE="1",
                           HEADROOM_WORKSPACE_DIR=str(arm.scratch),
                           HEADROOM_CONFIG_DIR=str(arm.scratch / "config"))
        reply = subprocess.run([str(arm.python), "-I", str(Path(hr.__file__).resolve()), "--worker"],
            input=json.dumps(request), encoding="utf-8", capture_output=True, timeout=30, env=environment)
        assert reply.returncode == 0, "recovery worker failed: " + str(reply.returncode) + " " + reply.stderr.strip()
        receipt = json.loads(reply.stdout)["receipt"]
        audit = receipt["recovery_audit"]
        assert audit["exact_row_multiset_restored"] and audit["retrievals"]
        assert all(r["python_mirror_matches"] and r["reopened_store_matches"] for r in audit["retrievals"])
        assert receipt["valid_attempt"] is False and receipt["reason"] == "unimplemented-ccr-recovery"
        assert arm.unchanged()
        print(json.dumps(audit, indent=2))
        # The writer process above is terminal; this is a distinct process, not
        # detach/reopen within its interpreter. Keep process overhead separate.
        import time
        payload = json.loads(reply.stdout)["payload"]
        for label, audit_source, audit_payload in (
                ("authorized-source", source, payload),
                ("different-source", json.dumps([{"id": "not-the-stored-source"}]), payload),
                ("missing-key", source, '<<ccr:000000000000 50_rows_offloaded>>')):
            restart_request = {"root": str(arm.root), "source": audit_source,
                               "payload": audit_payload, "audit_restart": True}
            start = time.perf_counter()
            reader = subprocess.run([str(arm.python), "-I", str(Path(hr.__file__).resolve()), "--worker"],
                input=json.dumps(restart_request), encoding="utf-8", capture_output=True, timeout=30, env=environment)
            wall_ms = (time.perf_counter() - start) * 1000
            assert reader.returncode == 0, "separate reader failed: " + reader.stderr.strip()
            result = json.loads(reader.stdout)
            assert all(r["source_matches"] is (label == "authorized-source") for r in result["retrievals"])
            assert all(r["status"] == ("miss" if label == "missing-key" else "hit") for r in result["retrievals"])
            assert "original_content" not in reader.stdout
            print(json.dumps({"case": label, "reader_process_wall_ms": wall_ms, "audit": result}, indent=2))
        assert arm.unchanged()
    print("PASS native recovery audit; synthetic canary, not stateful/downstream qualification")


def check_official_history(root):
    from test_acon_benchmark import ScriptedTransport, model, response
    task = corpus.make_task("multi-turn", "test", 1)
    for mode in ("history", "combined"):
        transport = ScriptedTransport()
        transport.responses = [response("# History Summary\nApproved destination: region-123456")]
        if mode == "combined":
            transport.responses.append(response("# Refined Observation\nNo new approved changes."))
        compress, provenance = ab.load_acon(Path(root), model(transport), mode)
        original = json.dumps(task, sort_keys=True)
        payload = compress(task, 17)
        self_calls = [body for path, body, _ in transport.requests if path == "chat"]
        assert len(self_calls) == (2 if mode == "combined" else 1)
        assert task["query"] in self_calls[0]["messages"][-1]["content"]
        assert "[COMPRESSION START]" in self_calls[0]["messages"][-1]["content"]
        assert all(m["role"] == original_m["role"] for m, original_m in zip(self_calls[0]["messages"], task["history_messages"]))
        assert json.dumps(task, sort_keys=True) == original
        assert "region-123456" in payload
        assert provenance["guidelines"] == "base-official-not-optimized"
        assert task["gold_answer"] not in json.dumps(self_calls[0]["messages"][-1]["content"].split("[COMPRESSION START]")[-1])
    print("PASS official pinned history/combined replay wiring; not live quality evidence")
    task = {**task, "gold_answer": "DO_NOT_LEAK_REFERENCE"}
    with tempfile.TemporaryDirectory() as directory:
        history_path = Path(directory) / "history.jinja"
        history_path.write_text("HISTORY-RULE {{ task }} {{ history }} {{ prev_summary }}\n"
                                "### REASONING\n### COMPLETED", encoding="utf-8")
        observation_path = Path(directory) / "observation.jinja"
        observation_path.write_text("OBSERVATION-RULE {{ task }} {{ history }} {{ observation }}\n"
                                    "# Refined Observation", encoding="utf-8")
        for mode in ("history", "combined"):
            transport = ScriptedTransport()
            transport.responses = [response("# History Summary\nApproved destination: region-123456")]
            if mode == "combined":
                transport.responses.append(response("# Refined Observation\nNo new approved changes."))
            runtime = model(transport)
            compress, provenance = ab.load_acon(Path(root), runtime, mode,
                guideline=observation_path if mode == "combined" else None,
                history_guideline_path=history_path)
            original = json.dumps(task, sort_keys=True)
            payload = compress(task, 17)
            calls = [body for path, body, _ in transport.requests if path == "chat"]
            assert len(calls) == (2 if mode == "combined" else 1)
            assert task["gold_answer"] not in json.dumps(calls)
            assert "HISTORY-RULE" in calls[0]["messages"][-1]["content"]
            assert "[COMPRESSION START]" in calls[0]["messages"][-1]["content"]
            assert all(m["role"] == original_m["role"] for m, original_m in zip(calls[0]["messages"], task["history_messages"]))
            assert json.dumps(task, sort_keys=True) == original and "region-123456" in payload
            assert provenance["history_guideline_sha256"] == ab.history_guideline(history_path)[1]
            assert provenance["guidelines"] == "external-guideline-not-qualified-optimized"
            if mode == "combined":
                assert calls[1]["messages"][-1]["content"].startswith("OBSERVATION-RULE")
                assert "region-123456" in calls[1]["messages"][-1]["content"]
                assert provenance["observation_guideline_sha256"] == ab.observation_guideline(observation_path)[1]
            before = len(runtime.calls)
            original_bytes = history_path.read_bytes()
            history_path.write_bytes(original_bytes + b"\nChanged.")
            try:
                compress(task, 17)
            except ValueError as exc:
                assert "history guideline changed" in str(exc)
            else:
                raise AssertionError("changed history guideline was accepted")
            assert len(runtime.calls) == before
            history_path.write_bytes(original_bytes)
    print("PASS fingerprinted external history/combined guidelines; not optimized-baseline qualification")


def check_pilot_integration(acon_root, smart=False, source_ids=False, history_guidelines=False):
    """Real CLI/classes, scripted transport and frozen development settings; no network."""
    from test_acon_benchmark import ScriptedTransport, DIGEST, model, response
    from freeze_campaign import freeze
    import contextlib
    import io

    if not ab.rp.rb._TOKENFOLD_BIN or not ab.rp.rb.TOKENIZER["is_exact"]:
        raise ValueError("integration requires built CLI and exact tiktoken")
    repository = Path(ab.__file__).resolve().parents[2]
    with tempfile.TemporaryDirectory(dir=repository / "tmp") as directory:
        directory = Path(directory)
        corpus.main(["--output-dir", str(directory / "corpus")])
        protocol = {"version": 1, "scope": "smoke",
                    "arms": ["raw", "tokenfold-lossless", "acon-observation", "acon-history", "acon-combined", "tokenfold-generative"],
                    "models": {"answering_and_compressor": "test:local@" + DIGEST}, "seeds": [17],
                    "runtime": {"acon_revision": ab.ACON_REVISION, "transport": "ollama-loopback-killable-subprocess",
                                "answering_roles": ["system", "user"], "assistant_prefill": False,
                                "scorer_guideline": None,
                                "generative_guideline": gen.GUIDELINE, "headroom_revision": None},
                    "budgets": {"context_tokens": 32768, "output_tokens": 512, "max_calls": 47,
                                "timeout_seconds": 1, "ratio": 0.5},
                    "quality": {"max_cfr": 0.01, "max_success_loss": 0.01, "confidence": 0.99,
                                "min_raw_success_clusters": 459},
                    "suites": [{"split": "test", "workload": "mixed",
                                "path": (directory / "corpus/test").relative_to(repository).as_posix()}]}
        protocol_path, frozen_path = directory / "protocol.json", directory / "frozen.json"
        if source_ids:
            protocol["runtime"]["generative_guideline"] = gen.SOURCE_IDS_GUIDELINE
            protocol["runtime"]["generative_source_ids_structured"] = False
        if history_guidelines:
            guideline = directory / "history.jinja"
            guideline.write_text("HISTORY-RULE {{ task }} {{ history }} {{ prev_summary }}\n"
                                 "### REASONING\n### COMPLETED", encoding="utf-8")
            protocol["runtime"]["acon_history_guideline_sha256"] = ab.history_guideline(guideline)[1]
            protocol["arms"].extend(["acon-history-guideline", "acon-combined-history-guideline"])
            protocol["budgets"]["max_calls"] += 17
        if smart:
            import hashlib
            artifact = directory / "_core.pyd"
            artifact.write_bytes(b"scripted-native-artifact")
            from bm25_select import HEADING_GUIDELINE
            protocol["arms"].extend(["headroom-universal-default", "headroom-smartcrusher-default", "tokenfold-bm25-heading", "acon-qa-observation"])
            protocol["budgets"]["max_calls"] += 25
            protocol["runtime"]["bm25_heading_guideline"] = HEADING_GUIDELINE
            protocol["runtime"]["acon_qa_observation_prompt"] = "smolagents/prompt_obs"
            protocol["runtime"].update(headroom_revision="a" * 40,
                headroom_smart_core_sha256=hashlib.sha256(artifact.read_bytes()).hexdigest(),
                headroom_smart_guideline="headroom-smartcrusher-default-tool-snapshot-v1")
        protocol_path.write_text(json.dumps(protocol), encoding="utf-8")
        frozen_path.write_text(json.dumps(freeze(protocol_path, repository)), encoding="utf-8")

        class Transport(ScriptedTransport):
            def __call__(self, path, body, timeout):
                if path == "chat":
                    messages = body["messages"]
                    if messages[0].get("role") == "system" and "Summarize only authorized" in messages[0]["content"]:
                        data = json.loads(messages[-1]["content"])
                        identity, text = next(iter(data["groups"].items()))
                        reply = {"summary": "Retained context.", "source_ids": [identity]} if source_ids else {
                            "summary": "Retained context.", "citations": [{"id": identity, "quote": text[:30]}]}
                        self.responses.append(response(json.dumps(reply)))
                    else:
                        self.responses.append(response("scripted-context"))
                return super().__call__(path, body, timeout)

        transport = Transport()
        original_model = ab.LocalModel
        arguments = ["--run-live", "--acon-root", str(acon_root), "--model", "test:local", "--model-digest", DIGEST,
                     "--tasks-dir", str(directory / "corpus/test"), "--output-dir", str(directory / "run"),
                     "--acon-history-arms", "--generative-arm", "--context-tokens", "32768", "--output-tokens", "512",
                     "--max-calls", "47", "--timeout", "1", "--seed", "17", "--ratio", "0.5",
                     "--campaign-protocol", str(protocol_path), "--campaign-freeze", str(frozen_path)]
        instances = []
        if source_ids:
            arguments.append("--generative-source-ids")
        if history_guidelines:
            arguments.extend(["--acon-history-guideline", str(guideline)])
            arguments[arguments.index("--max-calls") + 1] = "64"

        class Comparator:
            def __init__(self, root, revision, python, scratch, timeout, native_core=None):
                self.native_core = native_core
                self.provenance = {"revision": revision}
                if native_core:
                    self.provenance["native_core_sha256"] = hashlib.sha256(native_core.read_bytes()).hexdigest()
                self.last_receipt = None
                instances.append(self)

            def __call__(self, task, target, seed):
                self.last_receipt = {"healthy": True, "valid_attempt": True}
                return task["source"]

            def unchanged(self):
                return True

        if smart:
            import sys
            arguments[arguments.index("--max-calls") + 1] = "72"
            arguments.append("--bm25-heading-arm")
            arguments.append("--acon-qa-observation-arm")
            arguments.extend(["--headroom-root", str(directory), "--headroom-revision", "a" * 40,
                              "--headroom-python", sys.executable, "--headroom-smart-core", str(artifact)])
        with patch.object(ab, "LocalModel", side_effect=lambda *a: original_model(*a, transport=transport)), \
                patch.object(ab, "bounded_request", return_value={"version": "scripted"}), \
                patch.object(hr, "HeadroomArm", Comparator), \
                contextlib.redirect_stdout(io.StringIO()):
            assert ab.main(arguments) == 0
        report = json.loads((directory / "run/report.json").read_text())
        assert report["campaign_unchanged"] and report["tasks_unchanged"] and report["acon_unchanged"]
        assert set(report["comparisons"]) == set(protocol["arms"]) - {"raw"}
        assert all(r["paired_count"] == 5 and r["contrastive_confidence"]["confidence"] == 0.99
                   for r in report["comparisons"].values())
        assert len([body for path, body, _ in transport.requests if path == "chat"]) == (72 if smart else 64 if history_guidelines else 47)
        if history_guidelines:
            assert report["runtime"]["acon_history_guideline_sha256"] == ab.history_guideline(guideline)[1]
            assert report["acon"]["replay_arms"]["history"]["guidelines"] == "base-official-not-optimized"
            assert all(p["guidelines"] == "external-guideline-not-qualified-optimized"
                       for p in report["acon"]["external_history_guideline_arms"].values())
            calls = [body for path, body, _ in transport.requests if path == "chat"]
            assert sum("HISTORY-RULE" in c["messages"][-1]["content"] for c in calls) == 2
            # Independent failure injection, not an inference retry: preserve all
            # scripted attempts when guideline bytes disappear after deployment.
            original_run = ab.run_pilot
            def remove_after_run(*args, **kwargs):
                result = original_run(*args, **kwargs)
                guideline.unlink()
                return result
            arguments[arguments.index("--output-dir") + 1] = str(directory / "missing-guideline-run")
            before = len(transport.requests)
            with patch.object(ab, "LocalModel", side_effect=lambda *a: original_model(*a, transport=transport)), \
                    patch.object(ab, "bounded_request", return_value={"version": "scripted"}), \
                    patch.object(ab, "run_pilot", side_effect=remove_after_run), \
                    contextlib.redirect_stdout(io.StringIO()):
                assert ab.main(arguments) == 1
            failed = json.loads((directory / "missing-guideline-run/report.json").read_text())
            assert failed["acon_unchanged"] is False
            assert len([r for r in transport.requests[before:] if r[0] == "chat"]) == 64
            record_files = list((directory / "missing-guideline-run").glob("*.jsonl"))
            assert {p.stem for p in record_files} == set(protocol["arms"]) - {"raw"}
            assert all(len(p.read_text().splitlines()) == 10 for p in record_files)
        if smart:
            assert len(instances) == 2 and instances[1].native_core == artifact
            assert report["headroom_smart_unchanged"]
            assert report["acon"]["qa_observation_arm"]["prompt_family"] == "smolagents"
            assert report["runtime"]["headroom_smart_core_sha256"] == protocol["runtime"]["headroom_smart_core_sha256"]
        else:
            assert "headroom_smart" not in report and "headroom_smart_unchanged" not in report
    print("PASS frozen pilot integration with real CLI/official ACON; scripted answers and Headroom only")


def check_comparison_profile(acon_root):
    """Frozen real-CLI/official-ACON protocol check; scripted responses, no quality claim."""
    import contextlib, hashlib, io
    from test_acon_benchmark import ScriptedTransport, DIGEST, response
    from freeze_campaign import freeze
    repository = Path(ab.__file__).resolve().parents[2]
    with tempfile.TemporaryDirectory(dir=repository / "tmp") as directory:
        directory = Path(directory); tasks = directory / "tasks"; tasks.mkdir()
        groups = [{"id": "a", "text": '{"id":"a","time":2,"mag":5}'},
                  {"id": "z", "text": ',{"id":"z","time":1,"mag":5,"noise":"' + 'unused ' * 100 + '"}'}]
        task = {"id": "comparison-control", "family": "json", "tier": "A", "query": "Latest ID with mag >= 4.5?",
                "source": "[" + "".join(g["text"] for g in groups) + "]", "gold_answer": "a", "critical_atoms": [],
                "select_context": {"prefix": "[", "suffix": "]", "groups": groups}}
        (tasks / "task.json").write_text(json.dumps(task))
        config = {"record_path": [], "columns": {key: [key] for key in ("id", "time", "mag")},
                  "selection": {"id_column": "id", "rank_column": "time", "direction": "max", "tie": "id-ascending",
                                "filters": [{"column": "mag", "op": "gte", "value": 4.5}]}}
        profile = directory / "profile.json"; profile.write_text(json.dumps(config))
        profile_hash = hashlib.sha256(profile.read_bytes()).hexdigest()
        protocol = {"version": 1, "scope": "smoke", "arms": ["raw", "tokenfold-lossless", "acon-observation", "tokenfold-generative"],
                    "models": {"answering_and_compressor": "test:local@" + DIGEST}, "seeds": [17],
                    "runtime": {"acon_revision": ab.ACON_REVISION, "transport": "ollama-loopback-killable-subprocess",
                                "answering_roles": ["system", "user"], "assistant_prefill": False, "scorer_guideline": None,
                                "generative_guideline": gen.SOURCE_IDS_GUIDELINE, "generative_source_ids_structured": False,
                                "generative_comparison_profile_sha256": profile_hash, "headroom_revision": None},
                    "budgets": {"context_tokens": 32768, "output_tokens": 512, "max_calls": 6, "timeout_seconds": 1, "ratio": .5},
                    "quality": {"max_cfr": .01, "max_success_loss": .01, "confidence": .95, "min_raw_success_clusters": 299, "cfr_claim_count": 3},
                    "suites": [{"split": "test", "workload": "json", "path": tasks.relative_to(repository).as_posix()}]}
        protocol_path = directory / "protocol.json"; protocol_path.write_text(json.dumps(protocol))
        frozen = directory / "frozen.json"; frozen.write_text(json.dumps(freeze(protocol_path, repository)))
        arguments = ["--run-live", "--acon-root", str(acon_root), "--model", "test:local", "--model-digest", DIGEST,
                     "--generative-arm", "--generative-source-ids", "--generative-comparison-profile", str(profile),
                     "--tasks-dir", str(tasks), "--output-dir", str(directory / "stable"), "--context-tokens", "32768",
                     "--output-tokens", "512", "--max-calls", "6", "--timeout", "1", "--seed", "17", "--ratio", ".5",
                     "--campaign-protocol", str(protocol_path), "--campaign-freeze", str(frozen)]
        original_model, original_run = ab.LocalModel, ab.run_pilot
        for changed in (False, True):
            transport = ScriptedTransport()
            transport.responses = [response("a"), response("a"), response("# Refined Observation\na"), response("a"),
                                  response(json.dumps({"summary": "a has time 2 and mag 5", "source_ids": ["a"]})), response("a")]
            output = directory / ("changed" if changed else "stable")
            arguments[arguments.index("--output-dir") + 1] = str(output)
            def finish(*args, **kwargs):
                result = original_run(*args, **kwargs)
                if changed: profile.write_text(profile.read_text() + "\n")
                return result
            with patch.object(ab, "LocalModel", side_effect=lambda *args: original_model(*args, transport=transport)), \
                    patch.object(ab, "bounded_request", return_value={"version": "scripted"}), \
                    patch.object(ab, "run_pilot", side_effect=finish), contextlib.redirect_stdout(io.StringIO()):
                assert ab.main(arguments) == int(changed)
            report = json.loads((output / "report.json").read_text())
            rows = json.loads((output / "economics.json").read_text())
            assert report["runtime"]["generative_comparison_profile_sha256"] == profile_hash
            assert report["generative_comparison_profile_unchanged"] is not changed
            for comparison in report["comparisons"].values():
                bound = comparison["contrastive_confidence"]
                assert bound["confidence"] == 1 - .05 / 3
                assert bound["multiplicity"]["claim_count"] == 3
            assert sum(len(row["model_calls"]) for row in rows) == 6
            receipt = next(row["compression_receipt"] for row in rows if row["candidate"] == "tokenfold-generative")
            assert receipt["selection_contract"] == config["selection"] and receipt["disposition"] == "generated"
            assert len(list(output.glob("*.jsonl"))) == 3
    print("PASS frozen comparison profile + mutation retains six scripted calls; no model-quality claim")


if __name__ == "__main__":
    import sys
    if len(sys.argv) == 5 and sys.argv[1] == "--native-smart":
        check_native_smart(*sys.argv[2:])
    elif len(sys.argv) == 5 and sys.argv[1] == "--native-recovery":
        check_native_recovery(*sys.argv[2:])
    elif len(sys.argv) == 3 and sys.argv[1] == "--acon-root":
        check_comparison_profile(Path(sys.argv[2]))
        check_official_history(sys.argv[2])
        check_pilot_integration(Path(sys.argv[2]))
        check_pilot_integration(Path(sys.argv[2]), smart=True)
        check_pilot_integration(Path(sys.argv[2]), source_ids=True)
        check_pilot_integration(Path(sys.argv[2]), history_guidelines=True)
    else:
        unittest.main()
