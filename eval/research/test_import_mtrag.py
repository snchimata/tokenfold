"""Offline MTRAG framing, annotation isolation and split regression checks."""

import copy
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import import_mtrag as mtrag
from replay_context import validate_replay
from select_observation import source_context


def record(identity, base, history=True):
    return {"task_id": identity, "conversation_id": "conversation-" + identity,
            "Collection": "mt-rag-fiqa-beir-elser-512-100-20240501",
            "input": ([{"speaker": "user", "text": "Earlier " + identity},
                       {"speaker": "agent", "text": "Recorded response " + identity}] if history else [])
                     + [{"speaker": "user", "text": "Current question " + identity}],
            "contexts": [{"document_id": f"{base + i}-0-10", "title": f"Title {base + i}",
                          "text": f"Literal paragraph {base + i}\r\n", "reference": "GOLD_FLAG",
                          "score": 42, "feedback": {"relevant": True}} for i in range(5)],
            "targets": [{"speaker": "agent", "text": "CURRENT_TARGET_ONLY"}],
            "Answerability": ["ANSWERABLE"], "turn": "3"}


class ImportTests(unittest.TestCase):
    def test_all_passages_history_and_annotation_isolation(self):
        row = record("a", 100)
        task = mtrag.convert([row])[0]
        self.assertEqual(task["workload"], "multi-turn")
        self.assertEqual(task["query"], "Current question a")
        self.assertEqual(task["gold_answer"], "CURRENT_TARGET_ONLY")
        self.assertEqual(task["history_messages"][1]["role"], "assistant")
        self.assertEqual(task["critical_atoms"], [])
        source_context(task)
        validate_replay(task)
        for context in row["contexts"]:
            self.assertIn(context["text"], task["source"])
        self.assertNotIn("CURRENT_TARGET_ONLY", task["source"])
        self.assertNotIn("GOLD_FLAG", task["source"])
        self.assertEqual(sum(g.get("required", False) for g in task["select_context"]["groups"]), 1)
        import acon_benchmark as ab
        with self.assertRaisesRegex(ValueError, "requires an evaluator"):
            ab.run_pilot([task], None, None, None, len, .5, 42)
        changed = copy.deepcopy(row)
        changed["targets"][0]["text"] = "CHANGED_CURRENT_TARGET"
        changed["Answerability"] = ["UNANSWERABLE"]
        for context in changed["contexts"]:
            context.update(reference=False, feedback={}, score=-999)
        for message in changed["input"]:
            message["metadata"] = {"author_id": "ANNOTATION_ONLY"}
        second = mtrag.convert([changed])[0]
        for field in ("source", "query", "select_context", "history_messages", "observation",
                      "cluster_id", "development_split"):
            self.assertEqual(task[field], second[field])

    def test_source_components_are_order_stable_and_join_parent_passages_and_history(self):
        a, b, c, d = [record(name, base) for name, base in zip("abcd", (100, 200, 300, 400))]
        b["contexts"][0]["document_id"] = "100-10-20"  # Different passage, same parent.
        c["input"][0] = copy.deepcopy(b["input"][0])
        rows = [a, b, c, d]
        tasks = {t["upstream_id"]: t for t in mtrag.convert(rows)}
        self.assertEqual(tasks["a"]["cluster_id"], tasks["c"]["cluster_id"])
        self.assertEqual(tasks["a"]["development_split"], tasks["c"]["development_split"])
        self.assertNotEqual(tasks["a"]["cluster_id"], tasks["d"]["cluster_id"])
        self.assertEqual(mtrag.convert(rows), mtrag.convert(list(reversed(rows))))
        other = record("e", 500, history=False)
        other["contexts"][0]["text"] = a["contexts"][0]["text"]
        joined = mtrag.convert([a, other])
        self.assertEqual(joined[0]["cluster_id"], joined[1]["cluster_id"])
        single = mtrag.convert([other])[0]
        self.assertNotIn("history_messages", single)
        self.assertEqual(single["workload"], "retrieval")
        validate_replay(single)

    def test_contract_bounds_and_unknown_passage_formats_fail_closed(self):
        original = record("a", 100)
        mutations = [("contexts", original["contexts"][:4]), ("targets", []),
                     ("input", [{"speaker": "tool", "text": "opaque"}]),
                     ("input", [{"speaker": "user", "text": "x" * 4097}]),
                     ("input", original["input"] * 22)]
        for key, value in mutations:
            with self.subTest(key=key), self.assertRaises(ValueError):
                mtrag.convert([{**original, key: value}])
        with self.assertRaises(ValueError):
            mtrag.convert([original, original])
        for collection, identity, expected in (
                ("clapnq", "822086267_6698-7277-0-579", "822086267"),
                ("fiqa", "471123-0-798", "471123"),
                ("govt", "c8db6e06ff46669e-50302-52227", "c8db6e06ff46669e"),
                ("ibmcloud", "ibmcld_00474-7885-8455", "ibmcld_00474")):
            self.assertEqual(mtrag.document_root("mt-rag-" + collection + "-pinned", identity).split(":")[-1], expected)
        with self.assertRaises(ValueError):
            mtrag.document_root(original["Collection"], "unrecognized-id")

    def test_snapshot_hashes_and_no_overwrite(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp) / "snapshot"
            output = Path(temp) / "output"
            manifest = {"upstream": mtrag.REPOSITORY, "revision": mtrag.REVISION, "files": {}}
            for rel in (mtrag.DATA, "LICENSE", "README.md", "mtrag-human/README.md"):
                data = (json.dumps(record("a", 100)) + "\n").encode() if rel == mtrag.DATA else b"NOTICE"
                path = root / rel
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(data)
                manifest["files"][rel] = {"sha256": mtrag.digest(data),
                    "url": f"https://raw.githubusercontent.com/IBM/mt-rag-benchmark/{mtrag.REVISION}/{rel}"}
            (root / "manifest.json").write_text(json.dumps(manifest))
            args = ["--snapshot", str(root), "--output-dir", str(output)]
            with self.assertRaises(ValueError):
                mtrag.main(args)  # Not the pinned full-RAG snapshot.
            with patch.object(mtrag, "DATA_SHA256", manifest["files"][mtrag.DATA]["sha256"]):
                self.assertEqual(mtrag.main(args), 0)
                with self.assertRaises(FileExistsError):
                    mtrag.main(args)
                (root / mtrag.DATA).write_bytes(b"changed snapshot")
                with self.assertRaises(ValueError):
                    mtrag.main(args)
            self.assertEqual((output / "LICENSE.mtrag").read_bytes(), b"NOTICE")


if __name__ == "__main__":
    unittest.main()
