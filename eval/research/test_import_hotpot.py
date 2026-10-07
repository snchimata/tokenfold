"""Small offline regression for public corpus grouping and leakage prevention."""
import copy
import unittest
import json
import tempfile
from pathlib import Path

from import_hotpot import convert, answer_scores, main
from select_observation import source_context
import acon_benchmark as ab
from freeze_campaign import freeze


class ImportTests(unittest.TestCase):
    def test_component_cap_excludes_exposed_sources_without_label_selection(self):
        records = [{"id": str(i), "question": "Which?", "answer": "LABEL",
                    "context": [["Shared" if i < 3 else str(i), ["Source text."]]]}
                   for i in range(30)]
        tasks = convert(records)
        exposed = next(t["cluster_id"] for t in tasks if t["upstream_id"] == "0")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            snapshot, exclusions = root / "snapshot.json", root / "excluded.json"
            snapshot.write_text(json.dumps(records))
            exclusions.write_text(json.dumps([exposed]))
            args = ["--input", str(snapshot), "--output-dir", str(root / "out"),
                    "--limit-per-split", "2", "--one-per-component",
                    "--exclude-components", str(exclusions)]
            self.assertEqual(main(args), 0)
            self.assertEqual(main(["--input", str(snapshot), "--output-dir", str(root / "default"),
                                   "--limit-per-split", "2"]), 0)
            self.assertNotIn("one_per_component", json.loads((root / "default" / "manifest.json").read_bytes()))
            for split in ("train", "validation", "test"):
                default = [json.loads(p.read_bytes()) for p in sorted((root / "default" / split).glob("*.json"))]
                self.assertEqual(default, [t for t in tasks if t["development_split"] == split][:2])
                selected = [json.loads(p.read_bytes()) for p in sorted((root / "out" / split).glob("*.json"))]
                expected, seen = [], {exposed}
                for task in tasks:
                    if task["development_split"] == split and task["cluster_id"] not in seen:
                        expected.append(task)
                        seen.add(task["cluster_id"])
                self.assertEqual(selected, expected[:2])
            for record in records:
                record["answer"] = "DIFFERENT"
                record["supporting_facts"] = [["Ignored", 99]]
            snapshot.write_text(json.dumps(records))
            args[3] = str(root / "changed")
            self.assertEqual(main(args), 0)
            for split in ("train", "validation", "test"):
                before = [json.loads(p.read_bytes())["id"] for p in sorted((root / "out" / split).glob("*.json"))]
                after = [json.loads(p.read_bytes())["id"] for p in sorted((root / "changed" / split).glob("*.json"))]
                self.assertEqual(before, after)
            args[3] = str(root / "invalid")
            for value in ([exposed, exposed], ["unknown"], [True], {}):
                exclusions.write_text(json.dumps(value))
                with self.assertRaises(SystemExit):
                    main(args)
                self.assertFalse((root / "invalid").exists())
            exclusions.write_bytes(b" " * 65537)
            with self.assertRaises(SystemExit):
                main(args)
            self.assertFalse((root / "invalid").exists())
            exclusions.write_text(json.dumps([exposed]))
            with self.assertRaises(SystemExit):
                main([a for a in args if a != "--one-per-component"])
            self.assertFalse((root / "invalid").exists())

    def test_official_answer_normalization_and_special_answers(self):
        self.assertEqual(answer_scores(" The Foo-Bar! ", "foobar")["em"], 1)
        self.assertEqual(answer_scores("Yes.", "yes")["f1"], 1)
        self.assertEqual(answer_scores("yes indeed", "yes")["f1"], 0)
        self.assertEqual(answer_scores("foo foo", "foo bar")["f1"], 0.5)
        self.assertEqual(answer_scores("", "name")["f1"], 0)
        from test_acon_benchmark import ScriptedTransport, model, response
        task = {"id": "t", "source": "Foo-bar", "query": "Name?", "gold_answer": "foobar"}
        transport = ScriptedTransport()
        transport.responses = [response("The Foo-Bar!")] * 4
        _, reports, rows = ab.run_pilot([task], model(transport), lambda t, s: t["source"],
                                      lambda s, b: s, len, .5, 0, answer_evaluator="hotpot-answer-v1")
        self.assertEqual(reports["tokenfold-lossless"]["candidate_successes"], 1)
        self.assertEqual(rows[0]["answer_metrics"]["em"], 1)

    def test_source_only_components_and_nonliteral_answers(self):
        rows = [{"id": "a", "question": "Same nationality?", "answer": "yes",
                 "context": [["Person A", ["A is French."]], ["Person B", ["B is French."]]]},
                {"id": "b", "question": "Who is French?", "answer": "B",
                 "context": {"title": ["Person B"], "sentences": [["B is French."]]}},
                {"id": "c", "question": "Another?", "answer": "no",
                 "context": [["Separate", ["Different source."]]]}]
        tasks = {t["upstream_id"]: t for t in convert(rows)}
        self.assertEqual(tasks["a"]["cluster_id"], tasks["b"]["cluster_id"])
        self.assertEqual(tasks["a"]["development_split"], tasks["b"]["development_split"])
        self.assertNotEqual(tasks["a"]["cluster_id"], tasks["c"]["cluster_id"])
        for task in tasks.values():
            source_context(task)
            self.assertEqual(task["critical_atoms"], [])
        self.assertNotIn("yes", tasks["a"]["source"])
        changed = copy.deepcopy(rows)
        for row in changed:
            row["answer"] = "SECRET_GOLD"
            row["supporting_facts"] = [["Invented", 1]]
        for task in convert(changed):
            original = tasks[task["upstream_id"]]
            self.assertEqual(task["cluster_id"], original["cluster_id"])
            self.assertEqual(task["select_context"], original["select_context"])
            self.assertNotIn("SECRET_GOLD", task["source"])
        self.assertEqual(convert(rows), convert(list(reversed(rows))))
        with self.assertRaises(ValueError):
            convert(rows + [rows[0]])
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / "task.json").write_text(json.dumps(tasks["a"]))
            with self.assertRaises(ValueError):
                ab.rp.load_tasks(path)  # Existing literal-answer default stays fail-closed.
            self.assertEqual(ab.rp.load_tasks(path, require_literal_answer=False)[0], tasks["a"])
            protocol = json.loads(Path(__file__).with_name("campaign_protocol.example.json").read_text())
            protocol["suites"] = [{"split": tasks["a"]["development_split"], "workload": "retrieval", "path": "tasks"}]
            task_dir = path / "tasks"
            task_dir.mkdir()
            (task_dir / "task.json").write_text(json.dumps(tasks["a"]))
            protocol_path = path / "protocol.json"
            protocol_path.write_text(json.dumps(protocol))
            with self.assertRaises(ValueError):
                freeze(protocol_path, path)
            protocol["runtime"]["allow_inferred_answers"] = True
            protocol_path.write_text(json.dumps(protocol))
            self.assertEqual(freeze(protocol_path, path)["protocol"]["runtime"]["allow_inferred_answers"], True)
            protocol["runtime"]["allow_inferred_answers"] = "true"
            protocol_path.write_text(json.dumps(protocol))
            with self.assertRaises(ValueError):
                freeze(protocol_path, path)


if __name__ == "__main__":
    unittest.main()
