"""Model-free checks for the Select v3 notebook example."""
import ast
from contextlib import redirect_stderr, redirect_stdout
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch


NOTEBOOK = Path(__file__).with_name("quickstart.ipynb")


def sources():
    notebook = json.loads(NOTEBOOK.read_text(encoding="utf-8"))
    code = ["".join(c["source"]) for c in notebook["cells"] if c["cell_type"] == "code"]
    validate = next(s for s in code if s.startswith("def validate_select_fixture("))
    score = next(s for s in code if "RUN_SELECT_V3 = True" in s)
    return validate, score


class SelectV3Notebook(unittest.TestCase):
    def test_contextdoctor_examples_run_offline_in_this_notebook(self):
        notebook = json.loads(NOTEBOOK.read_text(encoding="utf-8"))
        cells = ["".join(c["source"]) for c in notebook["cells"]
                 if c["cell_type"] == "code" and "contextdoctor_report.py" in "".join(c["source"])]
        self.assertEqual(len(cells), 1, "the local notebook must include ContextDoctor")
        output = io.StringIO()
        with redirect_stdout(output):
            exec(cells[0], {"EXAMPLES": NOTEBOOK.parent})
        rows = {r["control"]: r for r in json.loads(output.getvalue())}
        self.assertEqual(rows["identity"]["verdict"], "no_effect")
        self.assertEqual(rows["cheaper_worse"]["verdict"], "regressed")
        self.assertTrue(all(r["p4_status"] == "blocked" and not r["expansion_allowed"] for r in rows.values()))

    def test_v3_is_enabled_and_uses_the_reference_pins(self):
        _, code = sources()
        self.assertIn('"fd8231d3cdc1efc46f7a6d7d83ca88b336a953591d63ea431407c8b13b990309"', code)
        self.assertIn('"03f20f46defab861c35b4684d8dc28e54e0f2171212f0decde0a214dc8dcc18b"', code)
        self.assertIn("tokenfold_select_v3.ettin import EttinDevelopmentScorer", code)
        self.assertIn('device="cuda:0"', code)
        self.assertNotIn("snapshot_download", code)
        self.assertNotIn("OnnxScorer", code)
        self.assertNotIn("select_v3_worker.py", code)
        ast.parse(code)

    def test_child_runs_once_and_relevance_failure_retains_all_groups(self):
        validate, code = sources()
        for outcome in ("pass", "wrong_top", "nonfinite", "timeout", "child_failure"):
            with self.subTest(outcome=outcome), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / "loader/src").mkdir(parents=True)
                namespace = {"json": json, "Path": Path, "tempfile": tempfile, "EXAMPLES": root / "examples"}
                exec(validate, namespace)

                def run(command, *, input, env, check, timeout, capture_output, text, encoding):
                    self.assertEqual(command[1:3], ["-I", "-c"])
                    self.assertEqual(command[-2:], [str(root), namespace["SELECT_V3_MANIFEST_SHA256"]])
                    self.assertEqual(timeout, 60)
                    self.assertTrue(check and capture_output and text)
                    self.assertEqual(encoding, "utf-8")
                    self.assertEqual(env["CUDA_VISIBLE_DEVICES"], "0")
                    self.assertEqual(env["HF_HUB_OFFLINE"], "1")
                    self.assertEqual(env["USERNAME"], "tokenfold-notebook")
                    self.assertEqual(env["TEMP"], env["HOME"])
                    self.assertEqual(env["TMP"], env["HOME"])
                    self.assertNotIn("OPENROUTER_API_KEY", env)
                    request = json.loads(input)
                    self.assertEqual(set(request), {"schema_version", "model_revision", "query", "groups"})
                    self.assertEqual(request["query"], "Which port does the payments service listen on?")
                    self.assertEqual(request["groups"], [
                        {"id": "payments", "text": "The payments service listens on port 8443.\n"},
                        {"id": "logging", "text": "The logger writes INFO events to standard output.\n"},
                        {"id": "health", "text": "The health endpoint returns HTTP 200 when ready.\n"},
                    ])
                    if outcome == "timeout":
                        raise subprocess.TimeoutExpired(command, timeout)
                    if outcome == "child_failure":
                        raise subprocess.CalledProcessError(1, command, stderr="synthetic child diagnostic")
                    values = [3.0, 1.0, 2.0] if outcome == "pass" else [1.0, 2.0, 3.0]
                    if outcome == "nonfinite":
                        values[0] = float("nan")
                    response = {"schema_version": 1, "model_revision": request["model_revision"],
                                "scores": [[g["id"], v] for g, v in zip(request["groups"], values)]}
                    return subprocess.CompletedProcess(command, 0, json.dumps({"response": response, "receipt": {}}), "")

                expected = {"wrong_top": AssertionError, "nonfinite": ValueError,
                            "timeout": subprocess.TimeoutExpired, "child_failure": subprocess.CalledProcessError}
                diagnostics = io.StringIO()
                with patch.dict(os.environ, {"TOKENFOLD_SELECT_V3_MODEL_DIR": str(root)}), \
                     patch("subprocess.run", side_effect=run) as child, redirect_stdout(io.StringIO()), redirect_stderr(diagnostics):
                    if outcome in expected:
                        with self.assertRaises(expected[outcome]):
                            exec(code, namespace)
                    else:
                        exec(code, namespace)
                child.assert_called_once()
                if outcome == "child_failure":
                    self.assertIn("synthetic child diagnostic", diagnostics.getvalue())
                if outcome in ("pass", "wrong_top"):
                    self.assertEqual(namespace["retained_groups"], namespace["request"]["groups"])
                    self.assertFalse(namespace["select_validation"]["qualification"])


if __name__ == "__main__":
    unittest.main()
