"""Real native allocation checks; not downstream quality qualification."""

import json
import unittest
from pathlib import Path

import acon_benchmark as ab
from bm25_select import Bm25SelectArm, heading_scores, HEADING_GUIDELINE


class HeadingTests(unittest.TestCase):
    def test_only_literal_framed_titles_are_boosted(self):
        texts = ["[document 0] Sam Wanamaker\nActor and director.",
                 "[document 1] Sam Wanamaker Award\nSam Wanamaker director award director.",
                 "[document 2] Norman Z. McLeod\nFilm director.",
                 "[LineId 3] Sam Wanamaker\nUnframed data."]
        query = "Are Norman Z. McLeod and Sam Wanamaker both directors?"
        original = ab.rp.rb.sel_bm25(texts, query)
        scores = heading_scores(texts, query)
        self.assertGreater(scores[0], max(original))
        self.assertGreater(scores[2], max(original))
        self.assertEqual(scores[1], original[1])
        self.assertEqual(scores[3], original[3])
        self.assertEqual(heading_scores(texts, ""), [0.0] * 4)


@unittest.skipUnless(ab.rp.rb._TOKENFOLD_BIN, "built Tokenfold CLI required")
class NativeBm25Tests(unittest.TestCase):
    def test_heading_variant_keeps_both_named_subjects_with_native_budget(self):
        groups = [{"id": "a", "text": "[document 0] Sam Wanamaker\nActor and director.\n"},
                  {"id": "b", "text": "[document 1] Norman Z. McLeod\nFilm director.\n"}]
        groups += [{"id": str(i), "text": f"[document {i}] Film award {i}\n" + " director award film" * 15 + "\n"}
                   for i in range(2, 10)]
        task = {"source": "".join(g["text"] for g in groups),
                "query": "Are Norman Z. McLeod and Sam Wanamaker both directors?",
                "gold_answer": "NEVER_EXPORT", "select_context": {"groups": groups}}
        arm = Bm25SelectArm(Path(ab.rp.rb._TOKENFOLD_BIN), heading_aware=True)
        payload = arm(task, 100, 42)
        self.assertIn(groups[0]["text"], payload)
        self.assertIn(groups[1]["text"], payload)
        self.assertLessEqual(ab.rp.rb.count_tokens(payload), 100)
        self.assertEqual(arm.last_receipt["kind"], HEADING_GUIDELINE)
        self.assertEqual(arm.last_receipt["model_calls"], 0)
        self.assertNotIn("NEVER_EXPORT", payload)

    def task(self):
        groups = [{"id": "rule", "text": "NEVER RETRY.\r\n", "required": True}]
        groups += [{"id": str(i), "text": f"[LineId {i}] service message sequence={i}\n"} for i in range(1, 65)]
        return {"source": "PREFIX\n" + "".join(g["text"] for g in groups) + "END",
                "query": "For LineId 33, return its message.", "gold_answer": "DO_NOT_EXPORT",
                "select_context": {"prefix": "PREFIX\n", "groups": groups, "suffix": "END"}}

    def test_literal_groups_protection_and_zero_inference(self):
        task = self.task()
        original = json.dumps(task)
        arm = Bm25SelectArm(Path(ab.rp.rb._TOKENFOLD_BIN))
        payload = arm(task, 150, 42)
        self.assertEqual(json.dumps(task), original)
        self.assertLessEqual(ab.rp.rb.count_tokens(payload), 150)
        self.assertIn("[LineId 33]", payload)
        self.assertTrue(payload.startswith("PREFIX\nNEVER RETRY.\r\n"))
        self.assertTrue(payload.endswith("END"))
        self.assertNotIn("DO_NOT_EXPORT", payload)
        self.assertEqual(arm.last_receipt["disposition"], "selected")
        self.assertEqual(arm.last_receipt["model_calls"], 0)
        self.assertFalse(arm.last_receipt["native_receipt"]["used_scorer"])
        self.assertEqual(arm(task, 1, 42), task["source"])
        self.assertEqual(arm.last_receipt["disposition"], "fell_back")

    def test_bad_ids_and_secret_query_fail_closed(self):
        arm = Bm25SelectArm(Path(ab.rp.rb._TOKENFOLD_BIN))
        task = self.task()
        task["select_context"]["groups"][1]["id"] = "rule"
        with self.assertRaises(ValueError):
            arm(task, 150, 42)
        self.assertFalse(arm.last_receipt["valid_attempt"])
        task = self.task()
        task["query"] = "Authorization: Bearer abcDEF123.token-value"
        with self.assertRaises(ValueError):
            arm(task, 150, 42)
        self.assertFalse(arm.last_receipt["valid_attempt"])


if __name__ == "__main__":
    unittest.main()
