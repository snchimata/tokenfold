"""Offline judge contracts; scripted verdicts are not evidence of judge accuracy."""

import json
import unittest
from pathlib import Path
from unittest.mock import patch

import grounded_answer as ga
from test_acon_benchmark import ScriptedTransport, model, response


class JudgeTests(unittest.TestCase):
    def test_evidence_judge_blinds_ids_and_checks_native_frame_before_model_calls(self):
        task = {"source": "PrefixA = 007B = 008Suffix", "query": "A?", "gold_answer": "DO_NOT_EXPORT_GOLD",
                "select_context": {"prefix": "Prefix", "suffix": "Suffix", "groups": [
                    {"id": "DO_NOT_EXPORT_ID_A", "text": "A = 007"}, {"id": "b", "text": "B = 008"}]}}
        candidate = {"summary_unverified": "A = 007", "source_evidence": [
            {"id": "DO_NOT_EXPORT_ID_A", "text": "A = 007"}]}
        frame = lambda value: "Prefix" + json.dumps(value) + "Suffix"
        fake = ScriptedTransport()
        fake.responses = [response(json.dumps({"grounded": True, "complete": False, "relationships_preserved": True}))]
        judge = ga.EvidenceSummaryJudge(model(fake), Path("binary"), structured=True)
        with patch.object(ga, "native_guard"):
            result = judge(task, frame(candidate), 0)
        payload = json.loads(next(b for p, b, _ in fake.requests if p == "chat")["messages"][1]["content"])
        self.assertEqual(payload["supporting_context"], "PrefixA = 007Suffix")
        self.assertNotIn("DO_NOT_EXPORT", json.dumps(payload))
        self.assertEqual(result["outcome"], "failure")
        self.assertEqual(result["guideline"], "tokenfold-evidence-summary-faithfulness-research-v1")
        self.assertEqual(len(result["model_calls"]), 1)
        bad = ["raw source", frame({**candidate, "extra": 1}), frame({**candidate, "source_evidence": []})]
        for evidence in ([{"id": "b", "text": "invented"}], [candidate["source_evidence"][0]] * 2,
                         [{"id": "missing", "text": "A = 007"}],
                         [{"id": "b", "text": "B = 008"}, candidate["source_evidence"][0]]):
            bad.append(frame({**candidate, "source_evidence": evidence}))
        for value in bad:
            with patch.object(ga, "native_guard"):
                result = judge(task, value, 0)
            self.assertEqual(result["reason"], "judge_input_failed")
            self.assertEqual(result["model_calls"], [])
        self.assertEqual(len(judge.model.calls), 1)

    def test_summary_judge_is_source_only_and_separate_from_answer_metrics(self):
        fake = ScriptedTransport()
        metrics = dict.fromkeys(ga.SUMMARY_FIELDS, True)
        metrics["relationships_preserved"] = False
        fake.responses = [response(json.dumps(metrics))]
        task = {**self.task(), "supporting_facts": "DO_NOT_EXPORT_LABELS", "source_ids": ["DO_NOT_EXPORT_IDS"]}
        with patch.object(ga, "native_guard"):
            result = ga.SummaryJudge(model(fake), Path("binary"), structured=True)(task, "billing was retried", 0)
        request = next(body for path, body, _ in fake.requests if path == "chat")
        payload = json.loads(request["messages"][1]["content"])
        self.assertEqual(set(payload), {"original_context", "question", "summary"})
        self.assertNotIn("REFERENCE_ONLY", json.dumps(request))
        self.assertNotIn("DO_NOT_EXPORT", json.dumps(request))
        self.assertEqual(set(request["format"]["required"]), set(ga.SUMMARY_FIELDS))
        self.assertEqual(result["guideline"], ga.SUMMARY_GUIDELINE)
        self.assertEqual(result["outcome"], "failure")
        self.assertEqual(result["total_usage"], {"input_tokens": 10, "output_tokens": 2})
        self.assertIn("not-semantic-proof", result["verification"])
        # No reference label is required to judge a summary's source support.
        fake.responses = [response(json.dumps(dict.fromkeys(ga.SUMMARY_FIELDS, True)))]
        with patch.object(ga, "native_guard"):
            result = ga.SummaryJudge(model(fake), Path("binary"))(
                {k: v for k, v in task.items() if k != "gold_answer"}, "billing was NOT retried", 0)
        self.assertTrue(result["valid_attempt"])
        self.assertEqual(result["outcome"], "success")

    def task(self):
        return {"source": "billing failed and was NOT retried.", "query": "Was billing retried?",
                "gold_answer": "REFERENCE_ONLY_no", "arm": "DO_NOT_EXPORT_ARM",
                "payload": "DO_NOT_EXPORT_COMPRESSED_CONTEXT"}

    def test_blinded_original_context_and_reference_isolated_from_student(self):
        fake = ScriptedTransport()
        fake.responses = [response(json.dumps(dict.fromkeys(ga.FIELDS, True)))]
        runtime = model(fake)
        with patch.object(ga, "native_guard") as guard:
            result = ga.GroundedJudge(runtime, Path("binary"))(self.task(), "No, billing was not retried.", 42)
        self.assertEqual(result["outcome"], "success")
        self.assertTrue(result["valid_attempt"])
        request = next(body for path, body, _ in fake.requests if path == "chat")
        data = json.loads(request["messages"][1]["content"])
        self.assertEqual(set(data), {"original_context", "question", "reference_answer", "response"})
        self.assertEqual(data["original_context"], self.task()["source"])
        self.assertNotIn("DO_NOT_EXPORT", json.dumps(request))
        self.assertEqual(guard.call_count, 2)
        self.assertEqual(len(result["model_calls"]), 1)
        self.assertEqual(result["total_usage"], {"input_tokens": 10, "output_tokens": 2})
        self.assertIsNone(result["billed_cost"])
        self.assertIn("not-semantic-proof", result["verification"])

    def test_every_dimension_is_required_and_failed_calls_remain_metered(self):
        for field in ga.FIELDS:
            metrics = dict.fromkeys(ga.FIELDS, True)
            metrics[field] = False
            fake = ScriptedTransport()
            fake.responses = [response(json.dumps(metrics))]
            with patch.object(ga, "native_guard"):
                result = ga.GroundedJudge(model(fake), Path("binary"))(self.task(), "Answer", 0)
            self.assertEqual(result["outcome"], "failure")
        fake = ScriptedTransport()
        fake.responses = [response("PRIVATE_REJECTED_BODY", done_reason="length")]
        with patch.object(ga, "native_guard"):
            result = ga.GroundedJudge(model(fake), Path("binary"))(self.task(), "Answer", 0)
        self.assertEqual(result["reason"], "judge_generation_failed")
        self.assertEqual(len(result["model_calls"]), 1)
        self.assertEqual(result["total_usage"]["input_tokens"], 10)
        self.assertNotIn("PRIVATE", json.dumps(result))

    def test_malformed_schema_duplicate_fields_and_input_refusals(self):
        valid = dict.fromkeys(ga.FIELDS, True)
        replies = ["PRIVATE_REJECTED_BODY", json.dumps({**valid, "correct": 1}),
                   json.dumps({**valid, "correct": "true"}), json.dumps({**valid, "extra": True}),
                   '{"correct":false,' + json.dumps(valid)[1:], "[]", "null"]
        for reply in replies:
            fake = ScriptedTransport()
            fake.responses = [response(reply)]
            with patch.object(ga, "native_guard"):
                result = ga.GroundedJudge(model(fake), Path("binary"))(self.task(), "Answer", 0)
            self.assertEqual(result["outcome"], "invalid")
            self.assertEqual(result["reason"], "judge_schema_failed")
            self.assertNotIn("PRIVATE", json.dumps(result))
        runtime = model(ScriptedTransport())
        with patch.object(ga, "native_guard", side_effect=ValueError("PRIVATE")):
            result = ga.GroundedJudge(runtime, Path("binary"))(self.task(), "Answer", 0)
        self.assertEqual(result["model_calls"], [])
        self.assertIsNone(result["billed_cost"])
        self.assertEqual(result["reason"], "judge_input_failed")
        self.assertEqual(runtime.calls, [])

    def test_structured_schema_uses_same_checks_and_no_route_fallback(self):
        fake = ScriptedTransport()
        fake.responses = [response(json.dumps(dict.fromkeys(ga.FIELDS, True)))]
        with patch.object(ga, "native_guard"):
            result = ga.GroundedJudge(model(fake), Path("binary"), structured=True)(self.task(), "Answer", 0)
        request = next(body for path, body, _ in fake.requests if path == "chat")
        self.assertEqual(set(request["format"]["required"]), set(ga.FIELDS))
        self.assertFalse(request["format"]["additionalProperties"])
        self.assertEqual(result["outcome"], "success")

    def test_explicit_abstention_is_versioned_without_changing_default_or_rule(self):
        self.assertEqual(ga.GroundedJudge(model(ScriptedTransport()), Path("binary")).system, ga.SYSTEM)
        fake = ScriptedTransport()
        metrics = dict.fromkeys(ga.FIELDS, True)
        metrics["appropriate_abstention"] = False
        fake.responses = [response(json.dumps(metrics))]
        with patch.object(ga, "native_guard"):
            result = ga.GroundedJudge(model(fake), Path("binary"), explicit_abstention=True)(self.task(), "Answer", 0)
        request = next(body for path, body, _ in fake.requests if path == "chat")
        self.assertIn("NOT whether the response contains an abstention", request["messages"][0]["content"])
        self.assertEqual(result["guideline"], ga.EXPLICIT_GUIDELINE)
        self.assertNotEqual(result["rubric_sha256"], ga.RUBRIC_SHA256)
        self.assertEqual(result["outcome"], "failure")  # No false criterion ignored or threshold weakened.

    def test_paired_integration_is_explicit_and_separates_student_and_judge_costs(self):
        import acon_benchmark as ab
        import generate_observation as gen
        task = {**self.task(), "id": "task", "evaluation_requirement":
                "grounded-long-form-and-abstention-not-short-value-exact"}
        student, evaluator = ScriptedTransport(), ScriptedTransport()
        student.responses = [response("No, billing was not retried.")] * 3
        verdicts = [dict.fromkeys(ga.FIELDS, True), {**dict.fromkeys(ga.FIELDS, True), "complete": False}]
        evaluator.responses = [response(json.dumps(v)) for v in verdicts] + [response("PRIVATE_BAD_VERDICT")]
        student_model = model(student)
        judge = ga.GroundedJudge(model(evaluator), Path("binary"), explicit_abstention=True)
        with patch.object(gen, "native_guard"), patch.object(ga, "native_guard"):
            _, reports, rows = ab.run_pilot([task], student_model, lambda t, s: t["source"],
                                           lambda source, budget: source, len, .75, 42,
                                           answer_evaluator="grounded-answer-research-v1", answer_judge=judge)
        self.assertEqual([row["outcome"] for row in rows], ["success", "failure", "invalid"])
        self.assertEqual(len(student_model.calls), 3)
        self.assertEqual(len(judge.model.calls), 3)
        self.assertTrue(all(len(row["model_calls"]) == 1 for row in rows))
        self.assertTrue(all(len(row["judge_receipt"]["model_calls"]) == 1 for row in rows))
        self.assertEqual(rows[-1]["judge_receipt"]["reason"], "judge_schema_failed")
        self.assertEqual(reports["acon-observation"]["invalid_count"], 1)
        student_requests = [body for path, body, _ in student.requests if path == "chat"]
        self.assertNotIn("REFERENCE_ONLY", json.dumps(student_requests))
        self.assertIn("Address every requested part", student_requests[0]["messages"][0]["content"])
        self.assertTrue(all(row["evaluation_wall_ms"] is not None for row in rows))
        self.assertEqual(rows[-1]["total_usage"], {"input_tokens": 10, "output_tokens": 2})
        economics = ab.economics_summary(rows)
        self.assertEqual(economics["evaluation"]["calls"], 3)
        self.assertEqual(economics["evaluation"]["invalid_judgments"], 1)
        self.assertEqual(economics["evaluation"]["total_usage"], {"input_tokens": 30, "output_tokens": 6})
        self.assertIsNone(economics["evaluation"]["billed_cost"])
        self.assertEqual(economics["all_attempts"]["raw"]["total_usage"]["input_tokens"], 10)

    def test_paired_mode_refuses_missing_shared_or_implicit_judge_before_calls(self):
        import acon_benchmark as ab
        runtime = model(ScriptedTransport())
        for evaluator, judge in (("grounded-answer-research-v1", None),
                                 ("grounded-answer-research-v1", ga.GroundedJudge(runtime, Path("binary"))),
                                 ("exact", ga.GroundedJudge(model(ScriptedTransport()), Path("binary")))):
            with self.assertRaises(ValueError):
                ab.run_pilot([self.task()], runtime, None, None, len, .5, 42,
                             answer_evaluator=evaluator, answer_judge=judge)
        self.assertEqual(runtime.calls, [])


if __name__ == "__main__":
    unittest.main()
