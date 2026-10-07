"""Offline checks for aligned source-only Loghub development adaptation."""

import csv
import io
import json
import tempfile
import unittest
from pathlib import Path

import acon_benchmark as ab
import import_loghub as lh
from select_observation import source_context


class LoghubTests(unittest.TestCase):
    def fixture(self):
        raw = b"first message\nsecond message\nthird message\n"
        buffer = io.StringIO()
        writer = csv.DictWriter(buffer, fieldnames=["LineId", "Content", "EventId", "EventTemplate"])
        writer.writeheader()
        for i, line in enumerate(raw.decode().splitlines(), 1):
            writer.writerow({"LineId": i, "Content": line, "EventId": "GOLD_LABEL", "EventTemplate": "GOLD_TEMPLATE"})
        return raw, buffer.getvalue().encode()

    def test_projection_alignment_grouping_and_conservative_clusters(self):
        raw, structured = self.fixture()
        tasks = lh.convert("family", raw, structured, 2)
        self.assertEqual(len(tasks), 4)
        self.assertEqual(len({t["cluster_id"] for t in tasks}), 1)
        self.assertNotEqual(tasks[0]["cluster_id"], lh.convert("other", raw, structured, 2)[0]["cluster_id"])
        for task in tasks:
            source_context(task)
            self.assertEqual(task["critical_atoms"], [])
            self.assertNotIn("GOLD_LABEL", task["source"])
            self.assertNotIn("GOLD_TEMPLATE", task["source"])
            self.assertFalse(any(g.get("required", False) for g in task["select_context"]["groups"]))
            if task["workload"] == "json":
                self.assertIsInstance(json.loads(task["source"]), list)
        self.assertEqual(tasks[0]["gold_answer"], "second message")
        for bad in (raw[:-15], raw.replace(b"first", b"changed")):
            with self.assertRaises(ValueError):
                lh.convert("family", bad, structured)
        with self.assertRaises(ValueError):
            lh.convert("family", raw, structured, 513)

    def test_snapshot_fingerprints_license_and_no_overwrite(self):
        raw, structured = self.fixture()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            snapshot, output = root / "snapshot", root / "output"
            snapshot.mkdir()
            entries = []
            revision = "a" * 40
            for rel, data in {"LICENSE": b"test license notice", **{
                rel: data for f in ("one", "two", "three") for rel, data in
                ((f"{f}/{f}_2k.log", raw), (f"{f}/{f}_2k.log_structured.csv", structured))}}.items():
                path = snapshot / rel
                path.parent.mkdir(exist_ok=True)
                path.write_bytes(data)
                entries.append({"path": rel, "sha256": lh.digest(data),
                                "url": f"https://raw.githubusercontent.com/logpai/loghub/{revision}/{rel}"})
            manifest = root / "snapshot.json"
            manifest.write_text(json.dumps({"repository": lh.REPOSITORY, "revision": revision, "files": entries}))
            args = ["--snapshot", str(snapshot), "--manifest", str(manifest), "--output-dir", str(output), "--window", "2"]
            self.assertEqual(lh.main(args), 0)
            self.assertEqual((output / "LICENSE.loghub").read_bytes(), b"test license notice")
            clusters = {}
            for split in ("train", "validation", "test"):
                for task in ab.rp.load_tasks(output / split):
                    self.assertNotIn(task["cluster_id"], {c for c, s in clusters.items() if s != split})
                    clusters[task["cluster_id"]] = split
            self.assertEqual(len(clusters), 3)
            with self.assertRaises(FileExistsError):
                lh.main(args)
            (snapshot / "LICENSE").write_bytes(b"changed")
            with self.assertRaises(ValueError):
                lh.main(args)


if __name__ == "__main__":
    unittest.main()
