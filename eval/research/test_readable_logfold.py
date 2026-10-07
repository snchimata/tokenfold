"""Presentation checks do not prove downstream model readability."""

import json
import unittest
import subprocess
from pathlib import Path
from unittest.mock import patch

import readable_logfold as rl
from readable_logfold import render


class PresentationTests(unittest.TestCase):
    def test_order_literals_and_capture_strings_recover_exactly(self):
        templates = ["RULE: never retry.\r\n", "req=\0 addr=\0 n=\0\r\n", "last=\0"]
        frame = "__tf_logfold1__\n" + json.dumps(templates) + "\n0\n1 001 0x0f 9007199254740993\n1 002 0xAf 00\n2 03"
        payload, recovered = render(frame)
        self.assertEqual(recovered, "RULE: never retry.\r\nreq=001 addr=0x0f n=9007199254740993\r\nreq=002 addr=0xAf n=00\r\nlast=03")
        data = json.loads(payload)
        self.assertEqual(data["rows"][1], [0, "001", "0x0f", "9007199254740993"])
        self.assertNotIn("\0", payload)
        self.assertEqual(data["template_parts"], [templates[1].split("\0")])
        self.assertEqual(data["rows"][0], templates[0])
        self.assertEqual(data["rows"][-1], "last=03")
        expanded = []
        for row in data["rows"]:
            if isinstance(row, str):
                expanded.append(row)
            else:
                parts = data["template_parts"][row[0]]
                expanded.append(parts[0] + "".join(v + part for v, part in zip(row[1:], parts[1:])))
        self.assertEqual("".join(expanded), recovered)

    def test_unused_and_singleton_templates_are_literal_not_indexed(self):
        payload, recovered = render('__tf_logfold1__\n["unused","one=\\u0000\\n","last"]\n1 007\n2')
        data = json.loads(payload)
        self.assertEqual(data["template_parts"], [])
        self.assertEqual(data["rows"], ["one=007\n", "last"])
        self.assertEqual(recovered, "one=007\nlast")

    def test_malformed_and_expansion_limits_fail_closed(self):
        for frame in ("plain text", "__tf_logfold1__\n{}\n0", "__tf_logfold1__\n[1]\n0",
                      '__tf_logfold1__\n["x"]\n1', '__tf_logfold1__\n["x"]\n0 extra',
                      '__tf_logfold1__\n["\\u0000"]\n0', '__tf_logfold1__\n["\\u0000"]\n0 -1',
                      '__tf_logfold1__\n["\\u0000"]\n0 0xZZ'):
            with self.assertRaises(ValueError):
                render(frame)
        frame = "__tf_logfold1__\n" + json.dumps(["x" * 600000]) + "\n0\n0"
        with self.assertRaisesRegex(ValueError, "expanded"):
            render(frame)

    def test_arm_checks_native_recovery_and_does_not_hide_fallbacks(self):
        source = "request=001\nrequest=002\nrequest=003\n"
        frame = '__tf_logfold1__\n["request=\\u0000\\n"]\n0 001\n0 002\n0 003'
        payload, _ = render(frame)
        def tokens(text):
            return 100 if text == source else 50 if text == payload else 20
        arm = rl.ReadableLogfoldArm(Path("binary"), lambda source, target: frame, tokens)
        task = {"source": source, "query": "Which request?", "gold_answer": "DO_NOT_EXPORT"}
        with patch.object(rl, "native_guard") as guard, patch.object(rl.subprocess, "run",
                return_value=subprocess.CompletedProcess([], 0, stdout=source.encode())):
            self.assertEqual(arm(task, 80, 0), payload)
            self.assertEqual(arm.last_receipt["disposition"], "rendered")
            self.assertTrue(arm.last_receipt["byte_exact_recovery"])
            self.assertNotIn("DO_NOT_EXPORT", str(guard.call_args_list))
            self.assertEqual(arm(task, 40, 0), source)
            self.assertEqual(arm.last_receipt["reason"], "over_budget")
        with patch.object(rl, "native_guard"), patch.object(rl.subprocess, "run",
                return_value=subprocess.CompletedProcess([], 0, stdout=b"wrong")):
            with self.assertRaises(ValueError):
                arm(task, 80, 0)
            self.assertFalse(arm.last_receipt["valid_attempt"])


if __name__ == "__main__":
    unittest.main()
