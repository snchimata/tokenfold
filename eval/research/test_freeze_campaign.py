"""Offline campaign-freeze regressions; no model-quality claims."""

import copy
import json
import tempfile
import unittest
from unittest.mock import patch
from pathlib import Path

import freeze_campaign as fc


class CampaignTests(unittest.TestCase):
    def test_explicit_arm_order_is_frozen_and_is_an_exact_permutation(self):
        self.protocol["runtime"] = {"arm_order": ["tokenfold-lossless", "raw"]}
        first = self.freeze()
        self.protocol["runtime"]["arm_order"].reverse()
        self.assertNotEqual(first["protocol_sha256"], self.freeze()["protocol_sha256"])
        for order in (["raw"], ["raw", "raw"], "raw", [True, "raw"]):
            self.protocol["runtime"]["arm_order"] = order
            with self.assertRaisesRegex(ValueError, "arm_order"): self.freeze()
    def test_qualification_cannot_omit_multidimensional_advantage_contract(self):
        self.protocol["scope"] = "qualification"
        self.protocol["quality"]["cfr_claim_count"] = 1
        saved = copy.deepcopy(self.protocol["advantage"])
        del self.protocol["advantage"]
        with self.assertRaisesRegex(ValueError, "advantage thresholds"): self.freeze()
        self.protocol["advantage"] = copy.deepcopy(saved)
        for value in (0, True, float("nan"), 1):
            self.protocol["advantage"]["latency"]["min_relative_reduction"] = value
            with self.assertRaisesRegex(ValueError, "latency advantage"): self.freeze()
        self.protocol["advantage"] = copy.deepcopy(saved)
        self.protocol["advantage"]["economics"]["cost_basis"] = ""
        with self.assertRaisesRegex(ValueError, "cost_basis"): self.freeze()
        self.protocol["advantage"] = copy.deepcopy(saved)
        self.protocol["advantage"]["features"] = []
        with self.assertRaisesRegex(ValueError, "feature behaviors"): self.freeze()
    def test_identical_source_cannot_inflate_qualification_clusters(self):
        self.write_task("test", {**self.task, "cluster_id": "one"})
        second = {**self.task, "id": "task-2", "query": "Return its value.", "cluster_id": "two"}
        self.write_task("test", second)
        self.assertEqual(self.freeze()["test_cluster_count"], 2)  # Legacy smoke declarations.
        self.protocol["scope"] = "qualification"
        self.protocol["quality"]["cfr_claim_count"] = 1
        self.protocol["runtime"] = {"transport": "native-openai-loopback-killable-subprocess",
                                    "answering_roles": ["system", "user"], "assistant_prefill": False}
        with self.assertRaisesRegex(ValueError, "identical source"):
            self.freeze()
        self.write_task("test", {**second, "cluster_id": "one"})
        with self.assertRaisesRegex(ValueError, "all five"):
            self.freeze()  # Shared cluster is permitted; other qualification requirements remain.
    def test_template_preflight_flag_is_typed_and_changes_frozen_runtime(self):
        for value in (None, 1, "true", []):
            self.protocol["runtime"]={"native_template_preflight":value}
            with self.assertRaisesRegex(ValueError,"native_template_preflight"):
                self.freeze()
        self.protocol["runtime"]={"native_template_preflight":True}
        first=self.freeze()
        self.protocol["runtime"]={"native_template_preflight":False}
        self.assertNotEqual(first["protocol_sha256"],self.freeze()["protocol_sha256"])
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.protocol = {
            "version": 1, "scope": "smoke", "arms": ["raw", "tokenfold-lossless"],
            "models": {"answering_and_compressor": "test@revision"}, "seeds": [0],
            "budgets": {"context_tokens": 8192, "output_tokens": 512,
                        "max_calls": 12, "timeout_seconds": 30, "ratio": 0.5},
            "quality": {"max_cfr": 0.01, "max_success_loss": 0.01,
                        "confidence": 0.95, "min_raw_success_clusters": 299},
            "advantage": {"latency": {"metric": "warm_end_to_end_p95_ms", "min_relative_reduction": .1},
                          "economics": {"metric": "total_campaign_cost_usd", "min_relative_reduction": .1,
                                        "cost_basis": "offline contract-test assumption only; no measured advantage"},
                          "features": ["authorized recovery"]},
            "suites": [{"split": "test", "workload": "json", "path": "test"}],
        }
        self.task = {"id": "task-1", "origin": "offline-contract-test-metadata-not-a-representativeness-claim", "family": "fixture", "tier": "A", "source": "answer: 7",
                     "query": "What is the answer?", "gold_answer": "7", "critical_atoms": ["7"]}
        self.write_task("test", self.task)
        self.path = self.root / "protocol.json"

    def write_task(self, split, task):
        directory = self.root / split
        directory.mkdir(exist_ok=True)
        (directory / (task["id"] + ".json")).write_text(json.dumps(task), encoding="utf-8")

    def freeze(self):
        self.path.write_text(json.dumps(self.protocol), encoding="utf-8")
        return fc.freeze(self.path, self.root)

    def test_stable_fingerprints_and_exclusive_publication(self):
        first = self.freeze()
        self.assertEqual(first, self.freeze())
        self.assertEqual(first["test_workloads"], ["json"])
        self.assertEqual(first["test_cluster_count"], 1)
        self.assertNotIn("gold_answer", first["files"][0])
        self.assertEqual(len(first["files"][0]["sha256"]), 64)
        self.task["query"] = "Return the answer."
        self.write_task("test", self.task)
        self.assertNotEqual(first["files"], self.freeze()["files"])
        output = self.root / "frozen.json"
        arguments = ["--protocol", str(self.path), "--root", str(self.root), "--output", str(output)]
        self.assertEqual(fc.main(arguments), 0)
        saved = output.read_bytes()
        with self.assertRaises(FileExistsError):
            fc.main(arguments)
        self.assertEqual(output.read_bytes(), saved)

    def test_task_addition_during_final_byte_check_refuses_publication(self):
        self.path.write_text(json.dumps(self.protocol), encoding="utf-8")
        original = Path.read_bytes
        task_path = self.root / "test" / "task-1.json"
        reads = 0
        def changing(path):
            nonlocal reads
            data = original(path)
            if path == task_path:
                reads += 1
                if reads == 2:
                    self.write_task("test", {**self.task, "id": "added-task"})
            return data
        output = self.root / "frozen.json"
        with patch.object(Path, "read_bytes", changing):
            with self.assertRaisesRegex(ValueError, "inventory changed"):
                fc.main(["--protocol", str(self.path), "--root", str(self.root), "--output", str(output)])
        self.assertFalse(output.exists())

    def test_resolved_outside_task_is_refused_before_loader_reads_it(self):
        self.path.write_text(json.dumps(self.protocol), encoding="utf-8")
        original = Path.resolve
        task_path = self.root / "test" / "task-1.json"
        def resolve(path, *args, **kwargs):
            return self.root.parent / "outside.json" if path == task_path else original(path, *args, **kwargs)
        with patch.object(Path, "resolve", resolve), patch.object(fc.rp, "load_tasks") as loader:
            with self.assertRaises(ValueError):
                fc.freeze(self.path, self.root)
            loader.assert_not_called()

    def test_overlap_refused(self):
        self.protocol["suites"].append({"split": "train", "workload": "json", "path": "train"})
        for field, value in (("id", "task-2"), ("source", "another answer: 7")):
            task = copy.deepcopy(self.task)
            task[field] = value
            self.write_task("train", task)
            with self.assertRaises(ValueError):
                self.freeze()
            for path in (self.root / "train").glob("*.json"):
                path.unlink()
        self.task["cluster_id"] = "same-cluster"
        self.write_task("test", self.task)
        task = {**self.task, "id": "task-2", "source": "other answer: 7"}
        self.write_task("train", task)
        with self.assertRaises(ValueError):
            self.freeze()

    def test_importer_assigned_split_cannot_be_relabelled_by_suite(self):
        for value in ("train", "validation", "unknown", None, True, ["test"]):
            self.write_task("test", {**self.task, "development_split": value})
            with self.assertRaisesRegex(ValueError, "development_split"):
                self.freeze()
        self.write_task("test", {**self.task, "development_split": "test"})
        self.assertEqual(self.freeze()["files"][0]["split"], "test")
        self.write_task("test", self.task)  # Legacy fixtures without assigned splits remain supported.
        self.assertEqual(self.freeze()["files"][0]["split"], "test")

    def test_task_workload_cannot_be_relabelled_by_suite(self):
        for value in ("logs", "code", "retrieval", "multi-turn", "unknown", None, True, ["json"]):
            self.write_task("test", {**self.task, "workload": value})
            with self.assertRaisesRegex(ValueError, "workload"):
                self.freeze()
        self.write_task("test", {**self.task, "workload": "json"})
        self.assertEqual(self.freeze()["files"][0]["workload"], "json")
        self.write_task("test", self.task)  # Keep legacy suite-labelled fixtures supported.
        self.assertEqual(self.freeze()["files"][0]["workload"], "json")
        self.protocol["suites"][0]["workload"] = "mixed"
        for value in (None, True, ["json"], "unknown"):
            self.write_task("test", {**self.task, "workload": value})
            with self.assertRaisesRegex(ValueError, "workload"):
                self.freeze()
        self.write_task("test", self.task)
        with self.assertRaisesRegex(ValueError, "workload"):
            self.freeze()
        for workload in sorted(fc.WORKLOADS):
            self.write_task("test", {**self.task, "workload": workload})
            self.assertEqual(self.freeze()["test_workloads"], [workload])

    def test_invalid_settings_and_inadequate_qualification(self):
        original = copy.deepcopy(self.protocol)
        for section, field, value in (("budgets", "timeout_seconds", float("nan")),
                                      ("budgets", "max_calls", True),
                                      ("budgets", "context_tokens", 512),
                                      ("quality", "confidence", 1),
                                      ("quality", "min_raw_success_clusters", 0)):
            self.protocol = copy.deepcopy(original)
            self.protocol[section][field] = value
            with self.assertRaises(ValueError):
                self.freeze()
        self.protocol = copy.deepcopy(original)
        self.protocol["scope"] = "qualification"
        self.protocol["quality"]["cfr_claim_count"] = 1
        self.protocol["runtime"] = {"transport": "ollama-loopback-killable-subprocess",
                                    "answering_roles": ["system", "user"],
                                    "assistant_prefill": False}
        with self.assertRaisesRegex(ValueError, "cluster_id"):
            self.freeze()
        self.task["cluster_id"] = "one-cluster"
        self.write_task("test", self.task)
        with self.assertRaisesRegex(ValueError, "five test workloads"):
            self.freeze()
        self.protocol["scope"] = "smoke"
        self.protocol["suites"][0]["path"] = ".."
        with self.assertRaises(ValueError):
            self.freeze()

    def test_qualification_sample_floor_counts_clusters_not_seeds(self):
        self.protocol["scope"] = "qualification"
        self.protocol["quality"]["cfr_claim_count"] = 1
        self.protocol["runtime"] = {"transport": "ollama-loopback-killable-subprocess",
                                    "answering_roles": ["system", "user"],
                                    "assistant_prefill": False}
        self.protocol["seeds"] = list(range(10))
        self.protocol["suites"] = []
        for split in ("train", "validation", "test"):
            for workload in sorted(fc.WORKLOADS):
                folder = split + "-" + workload
                self.protocol["suites"].append({"split": split, "workload": workload, "path": folder})
                count = 60 if split == "test" else 1
                for i in range(count):
                    identity = folder + "-" + str(i)
                    self.write_task(folder, {**self.task, "id": identity, "cluster_id": identity,
                                             "source": identity + ": answer 7"})
        frozen = self.freeze()
        self.assertEqual(frozen["test_cluster_count"], 300)
        self.assertEqual(frozen["cfr_sample_rule"]["min_raw_success_clusters_per_claim"], 299)
        self.protocol["quality"]["cfr_claim_count"] = 2
        with self.assertRaisesRegex(ValueError, "insufficient"):
            self.freeze()
        self.protocol["quality"]["min_raw_success_clusters"] = 368
        with self.assertRaisesRegex(ValueError, "insufficient"):
            self.freeze()  # Declaring N is not a substitute for 368 test clusters.
        for i in range(68):
            identity = "extra-independent-cluster-" + str(i)
            self.write_task("test-code", {**self.task, "id": identity, "cluster_id": identity,
                                         "source": identity + ": answer 7"})
        adjusted = self.freeze()
        self.assertEqual(adjusted["test_cluster_count"], 368)
        self.assertEqual(adjusted["cfr_sample_rule"]["min_raw_success_clusters_per_claim"], 368)
        self.assertAlmostEqual(adjusted["cfr_sample_rule"]["per_claim_alpha"], .025)
        self.protocol["quality"].update(cfr_claim_count=1, min_raw_success_clusters=299)
        path = self.root / "test-code" / "test-code-0.json"
        original = path.read_bytes()
        task = json.loads(original)
        path.write_text(json.dumps({**task, "origin": "synthetic-protocol-control"}), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "unqualified task origin"):
            self.freeze()  # Adequate labels/sample count cannot qualify a declared synthetic control.
        path.write_bytes(original)
        path.write_text(json.dumps({**task, "workload": "logs"}), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "task workload"):
            self.freeze()
        path.write_bytes(original)
        self.protocol["quality"]["min_raw_success_clusters"] = 298
        with self.assertRaisesRegex(ValueError, "insufficient"):
            self.freeze()
        self.protocol["quality"]["min_raw_success_clusters"] = 299
        for suite in self.protocol["suites"]:
            if suite["split"] == "test":
                for path in (self.root / suite["path"]).glob("*.json"):
                    task = json.loads(path.read_text())
                    task["cluster_id"] = "one-correlated-test-cluster"
                    path.write_text(json.dumps(task), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "insufficient"):
            self.freeze()

    def test_claim_count_is_explicit_and_covers_comparators(self):
        self.protocol["scope"] = "qualification"
        for value in (None, True, 0, -1, 1.5, "2"):
            self.protocol["quality"]["cfr_claim_count"] = value
            with self.assertRaisesRegex(ValueError, "cfr_claim_count"):
                self.freeze()
        self.protocol["scope"] = "smoke"
        self.protocol["arms"].append("acon-observation")
        self.protocol["quality"]["cfr_claim_count"] = 1
        with self.assertRaisesRegex(ValueError, "cfr_claim_count"):
            self.freeze()
        self.protocol["quality"]["cfr_claim_count"] = 2
        self.assertNotIn("cfr_sample_rule", self.freeze())
        del self.protocol["quality"]["cfr_claim_count"]
        self.freeze()  # Historical smoke declarations remain supported.

    def test_explicitly_unqualified_origins_cannot_be_relabelled_as_qualification(self):
        origins = ("synthetic-development-not-agent-qualification",
                   "public-mtrag-full-rag-development-not-agent-qualification",
                   "public-usgs-api-multistep-selection-development-not-qualification",
                   "PUBLIC-LOGHUB-WINDOW-DEVELOPMENT-NOT-AGENT-QUALIFICATION")
        self.protocol["quality"]["cfr_claim_count"] = 1
        self.protocol["runtime"] = {"transport": "ollama-loopback-killable-subprocess",
                                    "answering_roles": ["system", "user"],
                                    "assistant_prefill": False}
        for origin in origins:
            self.write_task("test", {**self.task, "origin": origin})
            self.protocol["scope"] = "smoke"
            self.freeze()
            self.protocol["scope"] = "qualification"
            with self.assertRaisesRegex(ValueError, "unqualified task origin"):
                self.freeze()
        for origin in (None, True, [], ""):
            self.write_task("test", {**self.task, "origin": origin})
            with self.assertRaisesRegex(ValueError, "origin must"):
                self.freeze()
        no_origin = {key: value for key, value in self.task.items() if key != "origin"}
        self.write_task("test", no_origin)
        with self.assertRaisesRegex(ValueError, "origin must"):
            self.freeze()
        self.protocol["scope"] = "smoke"
        self.freeze()  # Backward-compatible smoke tasks may omit provenance declarations.

    def test_live_run_must_match_frozen_test_settings(self):
        frozen = self.freeze()
        saved = self.root / "frozen.json"
        saved.write_text(json.dumps(frozen), encoding="utf-8")
        settings = {"seed": 0, "context_tokens": 8192, "output_tokens": 512,
                    "max_calls": 12, "timeout": 30, "ratio": 0.5, "runtime": {"guideline": "fixture-v1"}}
        self.protocol["runtime"] = settings["runtime"]
        frozen = self.freeze()
        saved.write_text(json.dumps(frozen), encoding="utf-8")

        def verify(**changes):
            return fc.verify_run(self.path, saved, self.root, self.root / "test",
                                 self.protocol["arms"], "test@revision", {**settings, **changes})

        self.assertEqual(verify()["scope"], "smoke")
        self.assertEqual(list(verify()), ["sha256", "protocol_sha256", "scope", "test_directory", "confidence"])
        for change in ({"seed": 1}, {"ratio": 0.25}, {"max_calls": 24}, {"timeout": 60},
                       {"runtime": {"guideline": "different"}}):
            with self.assertRaises(ValueError):
                verify(**change)
        with self.assertRaisesRegex(ValueError, "model"):
            fc.verify_run(self.path, saved, self.root, self.root / "test",
                          self.protocol["arms"], "other@revision", settings)
        self.task["query"] = "mutated query"
        self.write_task("test", self.task)
        with self.assertRaisesRegex(ValueError, "differs"):
            verify()

    def test_validation_requires_explicit_phase_and_keeps_test_default(self):
        self.protocol["suites"][0]["split"] = "validation"
        saved = self.root / "frozen.json"
        saved.write_text(json.dumps(self.freeze()), encoding="utf-8")
        settings = {"seed": 0, "context_tokens": 8192, "output_tokens": 512,
                    "max_calls": 12, "timeout": 30, "ratio": 0.5}
        args = (self.path, saved, self.root, self.root / "test", self.protocol["arms"], "test@revision", settings)
        with self.assertRaisesRegex(ValueError, "test suite"):
            fc.verify_run(*args)
        result = fc.verify_run(*args, suite_split="validation")
        self.assertEqual(result["suite_split"], "validation")
        self.assertNotIn("test_directory", result)
        for split in ("train", "arbitrary"):
            with self.assertRaises(ValueError):
                fc.verify_run(*args, suite_split=split)

    def test_answering_transport_and_roles_are_pinned(self):
        pinned = {"transport": "native-openai-loopback-killable-subprocess",
                  "answering_roles": ["system", "user"], "assistant_prefill": False}
        self.protocol["runtime"] = dict(pinned)
        self.freeze()
        for field, value in (("transport", "unknown-transport"),
                             ("answering_roles", ["system", "user", "assistant"]),
                             ("assistant_prefill", True)):
            self.protocol["runtime"] = {**pinned, field: value}
            with self.assertRaises(ValueError):
                self.freeze()
        # A different valid transport is a different declaration, not the same
        # frozen protocol: exact verify_run comparison rejects the drift.
        self.protocol["runtime"] = {**pinned, "transport": "ollama-loopback-killable-subprocess"}
        drifted = self.freeze()
        saved = self.root / "frozen.json"
        saved.write_text(json.dumps(drifted), encoding="utf-8")
        settings = {"seed": 0, "context_tokens": 8192, "output_tokens": 512,
                    "max_calls": 12, "timeout": 30, "ratio": 0.5,
                    "runtime": dict(pinned)}
        with self.assertRaisesRegex(ValueError, "comparator/guideline"):
            fc.verify_run(self.path, saved, self.root, self.root / "test",
                          self.protocol["arms"], "test@revision", settings)
        self.protocol["runtime"] = dict(pinned)
        self.protocol["scope"] = "qualification"
        self.protocol["quality"]["cfr_claim_count"] = 1
        with self.assertRaisesRegex(ValueError, "transport"):
            del self.protocol["runtime"]["transport"]
            try:
                self.freeze()
            finally:
                self.protocol["runtime"] = dict(pinned)
        with self.assertRaisesRegex(ValueError, "system/user"):
            self.protocol["runtime"] = {**pinned, "answering_roles": ["system", "user", "assistant"]}
            try:
                self.freeze()
            finally:
                self.protocol["runtime"] = dict(pinned)
        # Pinned transport/roles also satisfy qualification scope: the checks
        # above already ran under that scope, so no 299-cluster floor needed.

    def test_independent_summarizer_revision_is_required_and_exact(self):
        self.protocol["models"]["summarizer"] = "Qwen/Qwen3.5-0.8B@" + "b" * 64
        saved = self.root / "frozen.json"
        saved.write_text(json.dumps(self.freeze()), encoding="utf-8")
        settings = {"seed": 0, "context_tokens": 8192, "output_tokens": 512,
                    "max_calls": 12, "timeout": 30, "ratio": .5}
        args = (self.path, saved, self.root, self.root / "test", self.protocol["arms"], "test@revision", settings)
        for revision in (None, "different@revision"):
            with self.assertRaisesRegex(ValueError, "model"):
                fc.verify_run(*args, summarizer_revision=revision)
        self.assertEqual(fc.verify_run(*args, summarizer_revision=self.protocol["models"]["summarizer"])["scope"], "smoke")


if __name__ == "__main__":
    unittest.main()
