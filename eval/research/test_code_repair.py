"""Offline edit-boundary checks, not repair quality results."""

import json
import unittest
import tempfile
from pathlib import Path

from code_repair import apply_edits, edit_receipt, freeze_capsule, isolated_test_command


class EditTests(unittest.TestCase):
    def test_isolated_command_is_pinned_and_does_not_start_runtime(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); (root / "evaluator.py").write_text("assert True")
            command = isolated_test_command("sha256:" + "a" * 64, root, "tokenfold-repair-control")
            self.assertEqual(command[command.index("--pull") + 1], "never")
            self.assertEqual(command[command.index("--network") + 1], "none")
            self.assertIn("--read-only", command)
            self.assertIn("no-new-privileges", command)
            self.assertEqual(command[-4:], ["python", "-I", "-B", "/capsule/evaluator.py"])
            self.assertEqual(command.count("--mount"), 1)
            for image, name in (("python:latest", "tokenfold-repair-control"),
                                ("sha256:" + "a" * 64, "--privileged")):
                with self.assertRaises(ValueError): isolated_test_command(image, root, name)
    def test_capsule_roles_and_actor_evaluator_fields(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); actor = root / "actor.json"; evaluator = root / "test.py"
            actor.write_text(json.dumps({"query": "Fix code", "source_files": [], "output_contract": {}}))
            evaluator.write_text("assert True")
            frozen = freeze_capsule(actor, [evaluator], [])
            self.assertEqual(len(frozen["roles"]["actor"]), 1)
            self.assertEqual(len(frozen["roles"]["evaluator"][0]["sha256"]), 64)
            with self.assertRaises(ValueError): freeze_capsule(actor, [actor], [])
            actor.write_text(json.dumps({"query": "Fix code", "source_files": [], "output_contract": {}, "gold_answer": "hidden"}))
            with self.assertRaisesRegex(ValueError, "exclude evaluator"): freeze_capsule(actor, [evaluator], [])
    def test_declared_size_and_edit_limits_fail_closed(self):
        self.assertEqual(edit_receipt("x" * (1024 * 1024 + 1), "{}")["reason"], "input_limit")
        self.assertEqual(edit_receipt("source", " " * 65537)["reason"], "input_limit")
        too_many = json.dumps({"edits": [{"find": "source", "replace": "source"}] * 17})
        self.assertEqual(edit_receipt("source", too_many)["reason"], "invalid_edit_list")
        # Each bounded edit can be legal while their final expansion exceeds the source cap.
        source = "start " + "x" * (1024 * 1024 - 6)
        expanded = json.dumps({"edits": [{"find": "start", "replace": "longer than start"}]})
        self.assertEqual(edit_receipt(source, expanded)["reason"], "output_limit")
        self.assertEqual(source[:6], "start ")
    def test_receipts_distinguish_failure_without_candidate_text(self):
        missing = json.dumps({"edits": [{"find": "PRIVATE_REJECTED_MARKER", "replace": "unexecuted"}]})
        receipt = edit_receipt("source", missing)
        self.assertFalse(receipt["applied"])
        self.assertEqual(receipt["reason"], "nonunique_or_missing_span")
        self.assertNotIn("PRIVATE_REJECTED_MARKER", json.dumps(receipt))
        self.assertEqual(len(receipt["response_sha256"]), 64)
        self.assertIsNone(receipt["repaired_source_sha256"])
        self.assertFalse(receipt["candidate_code_executed"])
        self.assertEqual(edit_receipt("source", "not JSON")["reason"], "invalid_json_or_encoding")
        valid = edit_receipt("source", json.dumps({"edits": [{"find": "source", "replace": "changed"}]}))
        self.assertTrue(valid["applied"])
        self.assertIsNone(valid["reason"])
        self.assertIsNotNone(valid["repaired_source_sha256"])
    def test_exact_ordered_edits_and_unicode(self):
        source = "old α parser"
        reply = json.dumps({"edits": [{"find": "old", "replace": "new"}, {"find": "α", "replace": "β"}]})
        self.assertEqual(apply_edits(source, reply), "new β parser")
        self.assertEqual(source, "old α parser")

    def test_invalid_schema_duplicates_and_ambiguous_spans(self):
        for reply in ('{"edits":[],"edits":[]}', '{"edits":[]}',
                      '{"edits":[{"find":"x","find":"y","replace":"z"}]}',
                      json.dumps({"edits": [{"find": "x", "replace": "y", "path": "outside"}]}),
                      json.dumps({"edits": [{"find": "x", "replace": "y"}]})):
            with self.assertRaises(ValueError): apply_edits("x x", reply)

    def test_code_is_only_text_not_executed(self):
        reply = json.dumps({"edits": [{"find": "old", "replace": "raise RuntimeError('not executed')"}]})
        self.assertEqual(apply_edits("old", reply), "raise RuntimeError('not executed')")


if __name__ == "__main__":
    unittest.main()
