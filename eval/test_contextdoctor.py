#!/usr/bin/env python3
"""Offline contract and arithmetic checks; synthetic evidence, never quality proof."""
from __future__ import annotations

import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

try:
    import contextdoctor as cd
except ModuleNotFoundError:
    cd = None


def digest(data):
    if not isinstance(data, bytes):
        data = data.encode()
    return "sha256:" + hashlib.sha256(data).hexdigest()


H = digest("synthetic pin")
ARMS = ("baseline", "candidate")
CATEGORIES = ("model", "transform", "tool", "verification", "infrastructure")


def manifest(count=2, trials=1):
    return {
        "schema_version": "1.0", "kind": "contextdoctor.experiment",
        "experiment_id": "synthetic", "content_hash": H,
        "arms": {arm: {"name": arm, "transformation": "identity" if arm == "baseline" else "external",
                       "config_hash": H} for arm in ARMS},
        "tasks": [{"id": f"t{i}", "source_hash": H, "prompt_hash": H,
                   "snapshot_hash": H, "reference_hash": H,
                   "provenance": {"class": "synthetic_mechanics", "family": "arithmetic",
                                  "selection": "hand-authored", "permission": "public fixture",
                                  "exposure": "public"}} for i in range(count)],
        "pins": {"model": "synthetic-model", "revision": "fixed", "runtime": "test",
                 "device": "cpu", "dtype": "none", "tokenizer_hash": H,
                 "settings_hash": H, "tools_hash": H, "cache_policy": "isolated"},
        "verifier": {"digest": H, "kind": "recorded", "reference_health": "passed", "spec_hash": H},
        "schedule": [{"task_id": f"t{i}", "trial_id": f"r{j}", "order": list(ARMS)}
                     for i in range(count) for j in range(trials)],
        "limits": {"max_operations": 1000, "max_seconds": "120", "operation_seconds": "10",
                   "max_output_bytes": 8192, "max_tokens": 64, "retries": 0, "max_cost": "0"},
        "cost_policy": {"currency": "USD", "rounding": "ROUND_HALF_EVEN",
                        "categories": {c: {"coverage": "included" if c == "model" else "excluded",
                                           "basis": "provider_billed", "allocation": "per_episode"}
                                       for c in CATEGORIES}, "rates": []},
        "analysis": {"mode": "diagnostic", "seed": 7, "resamples": 100,
                     "confidence": "0.95", "min_tasks": 2, "min_trials": 1,
                     "noninferiority_margin": "0", "regression_ceiling": "0",
                     "method": "task_cluster_percentile", "multiplicity": "single_contrast",
                     "sample_size_rationale": "synthetic mechanics only", "review_hash": None},
        "bindings": {"operations": "operations.jsonl", "artifacts": {}},
        "execution": None, "rollout": None,
    }


def encode(value):
    return (json.dumps(value, ensure_ascii=False, sort_keys=True, indent=2) + "\n").encode()


def save_case(root, outcomes=None, amounts=None, trials=1):
    outcomes = outcomes or [(True, True), (True, False)]
    m = manifest(len(outcomes), trials)
    m["content_hash"] = cd.canonical_digest(m)
    episodes, events = [], []
    for i, outcome in enumerate(outcomes):
        for trial in range(trials):
            for ai, arm in enumerate(ARMS):
                eid = f"t{i}-r{trial}-{arm}"
                receipt = {"schema_version": "1.0", "kind": "contextdoctor.verification",
                           "episode_id": eid, "manifest_hash": m["content_hash"],
                           "output_hash": H, "verifier_hash": H, "independence": "external",
                           "reference_health": "passed", "status": "passed" if outcome[ai] else "failed",
                           "checks": [{"id": "answer", "result": "passed" if outcome[ai] else "failed"}],
                           "evidence_hash": H}
                raw = encode(receipt)
                rh = digest(raw)
                path = f"{eid}.verification.json"
                (root / path).write_bytes(raw)
                m["bindings"]["artifacts"][rh] = path
                episodes.append({"schema_version": "1.0", "kind": "contextdoctor.episode",
                                 "id": eid, "experiment_id": m["experiment_id"],
                                 "manifest_hash": m["content_hash"], "task_id": f"t{i}",
                                 "trial_id": f"r{trial}", "arm": arm, "pins_hash": cd.canonical_digest(m["pins"]),
                                 "source_hash": H, "prompt_hash": H, "snapshot_hash": H,
                                 "transform_hash": cd.canonical_digest(m["arms"][arm]),
                                 "status": "completed", "request_hash": H, "output_hash": H,
                                 "verification_hash": rh, "treatment": {"enabled": arm == "candidate",
                                     "disposition": "applied" if arm == "candidate" else "no_op",
                                     "input_hash": H, "output_hash": digest("changed synthetic context") if arm == "candidate" else H, "receipt_hash": None},
                                 "operation_ids": [eid], "wall_seconds": "1", "safety_violation": False})
                cost = {"basis": "provider_billed", "amount": str(amounts[ai] if amounts else "1"),
                        "currency": "USD", "receipt_hash": digest(eid), "reconciled": True,
                        "missing_reason": None}
                bill = encode({"operation_id": eid, "amount": cost["amount"], "currency": "USD", "charge_id": eid})
                cost["receipt_hash"] = digest(bill)
                (root / f"{eid}.charge.json").write_bytes(bill)
                m["bindings"]["artifacts"][cost["receipt_hash"]] = f"{eid}.charge.json"
                for event in ("dispatch", "terminal"):
                    events.append({"schema_version": "1.0", "kind": "contextdoctor.operation",
                                   "operation_id": eid, "episode_id": eid, "experiment_id": m["experiment_id"],
                                   "manifest_hash": m["content_hash"], "event": event,
                                   "role": "agent", "category": "model", "status": "dispatched" if event == "dispatch" else "completed",
                                   "duration_seconds": None if event == "dispatch" else "1", "usage": None,
                                   "cost": cost if event == "terminal" else {"basis": "unavailable", "amount": None,
                                       "currency": "USD", "receipt_hash": None, "reconciled": False,
                                       "missing_reason": "not_reconciled"}, "request_id": eid})
    write_case(root, m, episodes, events)
    return m, episodes, events


def write_case(root, m, episodes, events):
    (root / "experiment.json").write_bytes(encode(m))
    for filename, rows in (("episodes.jsonl", episodes), ("operations.jsonl", events)):
        (root / filename).write_text("".join(json.dumps(r, sort_keys=True) + "\n" for r in rows), encoding="utf-8")


class Contracts(unittest.TestCase):
    def setUp(self):
        self.assertIsNotNone(cd, "ContextDoctor module is not implemented")
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)

    def load(self):
        return cd.load_experiment(self.root / "experiment.json", self.root / "episodes.jsonl")

    def test_valid_contract_binds_manifest_without_local_paths(self):
        m, _, _ = save_case(self.root)
        self.assertEqual(self.load()["manifest"]["experiment_id"], "synthetic")
        moved = copy.deepcopy(m)
        moved["bindings"]["operations"] = "elsewhere.jsonl"
        self.assertEqual(cd.canonical_digest(moved), m["content_hash"])
        moved["limits"]["max_tokens"] += 1
        self.assertNotEqual(cd.canonical_digest(moved), m["content_hash"])

    def test_manifest_tampering_and_strict_fields_fail_closed(self):
        for mutate in (lambda m: m.update(schema_version="9"), lambda m: m.update(extra=True),
                       lambda m: m["limits"].update(max_operations=True),
                       lambda m: m["limits"].update(max_seconds="NaN"),
                       lambda m: m["limits"].update(max_cost="-1"),
                       lambda m: m["tasks"][0].update(source_hash="sha256:x")):
            m, ep, ev = save_case(self.root)
            mutate(m)
            write_case(self.root, m, ep, ev)
            with self.assertRaises(ValueError):
                self.load()

    def test_duplicate_keys_and_nonfinite_json_are_rejected(self):
        save_case(self.root)
        for payload in ('{"schema_version":"1.0","schema_version":"1.0"}', '{"x":NaN}', '{"x":1.5}'):
            (self.root / "experiment.json").write_text(payload)
            with self.assertRaises(ValueError):
                self.load()

    def test_traversal_and_cross_platform_absolute_paths_are_rejected(self):
        for path in ("../operations.jsonl", "/tmp/x", "C:\\x", "\\x", "sub\\..\\x"):
            m, ep, ev = save_case(self.root)
            m["bindings"]["operations"] = path
            write_case(self.root, m, ep, ev)
            with self.assertRaises(ValueError):
                self.load()

    def test_tampered_verification_artifact_is_rejected(self):
        _, ep, _ = save_case(self.root)
        ref = ep[0]["verification_hash"]
        m = json.loads((self.root / "experiment.json").read_text())
        (self.root / m["bindings"]["artifacts"][ref]).write_text("{}")
        with self.assertRaises(ValueError):
            self.load()

    def test_duplicate_episode_ids_and_negative_cost_are_rejected(self):
        m, ep, ev = save_case(self.root)
        write_case(self.root, m, ep + [ep[0]], ev)
        with self.assertRaises(ValueError):
            self.load()
        ev[1]["cost"]["amount"] = "-1"
        write_case(self.root, m, ep, ev)
        with self.assertRaises(ValueError):
            self.load()

    def test_symlink_artifact_escape_is_rejected(self):
        m, ep, ev = save_case(self.root)
        with tempfile.TemporaryDirectory() as outside:
            target = Path(outside) / "outside.jsonl"
            target.write_text("")
            try:
                (self.root / "escaped.jsonl").symlink_to(target)
            except (OSError, NotImplementedError):
                self.skipTest("symlink creation is unavailable on this platform")
            m["bindings"]["operations"] = "escaped.jsonl"
            write_case(self.root, m, ep, ev)
            with self.assertRaises(ValueError):
                self.load()


class PurePairing(unittest.TestCase):
    @unittest.skipUnless(os.name == "nt", "Windows junction containment")
    def test_windows_junction_artifact_escape_is_rejected(self):
        with tempfile.TemporaryDirectory() as root, tempfile.TemporaryDirectory() as outside:
            junction = Path(root) / "junction"
            result = subprocess.run(["cmd.exe", "/c", "mklink", "/J", str(junction), outside],
                                    capture_output=True, check=False)
            self.assertEqual(result.returncode, 0, result.stderr)
            try:
                (Path(outside) / "private.txt").write_text("outside")
                with self.assertRaisesRegex(ValueError, "artifact escapes root"):
                    cd.safe_path(Path(root), "junction/private.txt")
            finally:
                junction.rmdir()

    def test_pure_pairing_does_not_load_execution_or_tokenizers(self):
        code = '''import sys
class Forbidden:
    def find_spec(self, fullname, path=None, target=None):
        if fullname.split('.')[0] in ('run_baselines','run_paired','tiktoken','torch','transformers','subprocess','socket') or fullname == 'urllib.request':
            raise AssertionError(fullname)
sys.meta_path.insert(0, Forbidden())
import paired_records as p
rows=[]
for i,(a,b) in enumerate([(True,True)]*8+[(True,False)]*2+[(False,True),(False,False)]):
    for arm,ok in [('raw',a),('candidate',b)]:
        rows.append(dict(schema_version='1.0',run_id='x',task_id=str(i),environment_snapshot=p.snapshot_hash('x'),arm=arm,model='m',policy_revision=None,seed=0,attempt=1,outcome='success' if ok else 'failure',evidence=['synthetic']))
r=p.aggregate(*p.pair_records(rows))
assert r['paired_count']==12 and r['conditional_failure_rate']==0.2 and r['success_delta']==-0.0833,r
'''
        result = subprocess.run([sys.executable, "-c", code], cwd=Path(__file__).parent, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)


class Reporting(Contracts):
    def report(self):
        return cd.build_report(self.load())

    def test_cheaper_but_worse_counts_failures_and_all_successes(self):
        save_case(self.root, [(True, True)] * 4 + [(True, False)] * 4 + [(False, False)] * 2,
                  amounts=("1", "0.8"))
        report = self.report()
        self.assertEqual(report["accounting"]["baseline"]["provider_billed"]["known_subtotal"], "10")
        self.assertEqual(report["economics"]["provider_billed"]["baseline"]["cost_per_verified_success"], "1.25")
        self.assertEqual(report["economics"]["provider_billed"]["candidate"]["cost_per_verified_success"], "2")
        self.assertEqual(report["economics"]["provider_billed"]["relative_reduction"], "-0.6")
        self.assertEqual(report["paired"]["outcomes"]["baseline_only_success"], 4)
        self.assertEqual(report["verdict"], "regressed")

    def test_zero_success_is_null_with_a_reason(self):
        save_case(self.root, [(False, False)] * 2)
        row = self.report()["economics"]["provider_billed"]["baseline"]
        self.assertIsNone(row["cost_per_verified_success"])
        self.assertEqual(row["reason"], "zero_verified_successes")

    def test_incomplete_operations_and_missing_cells_keep_known_spend(self):
        m, ep, ev = save_case(self.root)
        ep.pop()
        ev.pop()
        write_case(self.root, m, ep, ev)
        r = self.report()
        self.assertEqual(r["counts"]["unexecuted"], 1)
        self.assertEqual(r["accounting"]["baseline"]["provider_billed"]["known_subtotal"], "2")
        self.assertFalse(r["evidence_complete"])
        self.assertIn("unfinished_operations", r["claim_blockers"])
        self.assertIsNone(r["economics"]["provider_billed"]["candidate"]["cost_per_verified_success"])

    def test_identity_mismatch_invalidates_but_does_not_erase_charges(self):
        m, ep, ev = save_case(self.root)
        ep[0]["pins_hash"] = digest("wrong settings")
        write_case(self.root, m, ep, ev)
        r = self.report()
        self.assertEqual(r["verdict"], "invalid")
        self.assertEqual(r["accounting"]["baseline"]["provider_billed"]["known_subtotal"], "2")
        self.assertEqual(r["counts"]["invalid"], 1)

    def test_agent_success_without_independent_receipt_stays_unknown(self):
        m, ep, ev = save_case(self.root)
        ep[0]["verification_hash"] = None
        write_case(self.root, m, ep, ev)
        r = self.report()
        self.assertEqual(r["counts"]["unknown"], 1)
        self.assertEqual(r["counts"]["verified_successes"]["baseline"], 1)
        self.assertFalse(r["evidence_complete"])

    def test_cumulative_usage_is_last_wins_and_price_is_not_billing(self):
        m, ep, ev = save_case(self.root, [(True, True)] * 2)
        m["cost_policy"]["categories"]["model"]["basis"] = "usage_priced"
        m["cost_policy"]["rates"] = [{"model": "synthetic-model", "metric": k, "rate": "1",
                                      "per": 1000, "currency": "USD", "evidence_hash": H}
                                     for k in ("input", "cache_read", "cache_write", "output")]
        rebind(self.root, m, ep, ev)
        for event in ev:
            if event["event"] == "terminal":
                event["usage"] = {"model": "synthetic-model", "source": "provider_observed",
                                  "normalization": "cumulative_last_v1", "snapshots": [
                                      {"input": 10, "cache_read": 0, "cache_write": 0, "output": 1},
                                      {"input": 10, "cache_read": 0, "cache_write": 0, "output": 2}]}
                event["cost"] = {"basis": "usage_priced", "amount": None, "currency": "USD",
                                 "receipt_hash": None, "reconciled": False, "missing_reason": None}
        write_case(self.root, m, ep, ev)
        r = self.report()
        self.assertEqual(r["accounting"]["baseline"]["usage_priced"]["known_subtotal"], "0.024")
        self.assertFalse(r["evidence_complete"])
        self.assertIn("missing_price_evidence", r["claim_blockers"])
        self.assertIsNone(r["economics"]["provider_billed"]["baseline"]["cost_per_verified_success"])
        self.assertEqual(r["secondary"]["baseline"]["provider_tokens"]["input"], 20)

    def test_undefined_bootstrap_draws_are_not_silently_filtered(self):
        save_case(self.root, [(True, True), (False, False)])
        r = self.report()
        stat = r["uncertainty"]["relative_reduction"]["provider_billed"]
        self.assertGreater(stat["undefined_resamples"], 0)
        self.assertIsNone(stat["interval"])
        self.assertEqual(r["uncertainty"]["task_clusters"], 2)

    def test_exposure_does_not_replace_assigned_arm_outcomes(self):
        m, ep, ev = save_case(self.root, [(True, True)] * 2)
        for e in ep:
            if e["arm"] == "candidate":
                e["treatment"]["disposition"] = "fallback"
                e["treatment"]["output_hash"] = H
        write_case(self.root, m, ep, ev)
        r = self.report()
        self.assertEqual(r["counts"]["verified_successes"]["candidate"], 2)
        self.assertEqual(r["exposure"]["candidate"]["fallback"], 2)
        self.assertEqual(r["verdict"], "no_effect")

    def test_cli_report_is_reproducible_and_never_overwrites(self):
        save_case(self.root)
        argv = [sys.executable, str(Path(__file__).with_name("contextdoctor.py")), "report",
                "--manifest", str(self.root / "experiment.json"), "--records", str(self.root / "episodes.jsonl")]
        for name in ("out1", "out2"):
            result = subprocess.run(argv + ["--out", str(self.root / name)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
        for name in ("report.json", "report.md"):
            self.assertEqual((self.root / "out1" / name).read_bytes(), (self.root / "out2" / name).read_bytes())
        result = subprocess.run(argv + ["--out", str(self.root / "out1")], capture_output=True)
        self.assertEqual(result.returncode, 1)
        result = subprocess.run(argv + ["--out", str(self.root / "gate"), "--require-benefit"], capture_output=True)
        self.assertEqual(result.returncode, 3)

    def test_excluded_known_tool_costs_stay_in_ledger_not_model_only_ratio(self):
        m, ep, ev = save_case(self.root, [(True, True)] * 2)
        for e in ep:
            oid = e["id"] + "-tool"
            e["operation_ids"].append(oid)
            for event in ("dispatch", "terminal"):
                op = copy.deepcopy(next(o for o in ev if o["episode_id"] == e["id"] and o["event"] == event))
                op.update(operation_id=oid, category="tool", role="tool")
                if event == "terminal":
                    op["cost"] = {"basis": "modeled", "amount": None, "currency": "USD", "receipt_hash": None,
                                  "reconciled": False, "missing_reason": "missing_usage"}
                ev.append(op)
        write_case(self.root, m, ep, ev)
        r = self.report()
        self.assertEqual(r["economics"]["provider_billed"]["baseline"]["cost_per_verified_success"], "1")
        self.assertIn("tool", r["accounting"]["baseline"]["modeled"]["known_by_category"])

    def test_unbound_billing_evidence_is_not_complete_spend(self):
        m, ep, ev = save_case(self.root, [(True, True)] * 2)
        ev[1]["cost"]["receipt_hash"] = digest("no invoice bytes")
        write_case(self.root, m, ep, ev)
        r = self.report()
        self.assertEqual(r["accounting"]["baseline"]["provider_billed"]["known_subtotal"], "2")
        self.assertFalse(r["evidence_complete"])
        self.assertIn("unbound_billing_evidence", r["claim_blockers"])

    def test_unknown_exposure_blocks_complete_evidence(self):
        m, ep, ev = save_case(self.root, [(True, True)] * 2)
        ep[1]["treatment"]["disposition"] = "unknown"
        write_case(self.root, m, ep, ev)
        self.assertFalse(self.report()["evidence_complete"])

    def test_tiny_positive_baseline_is_not_rounded_before_ratio(self):
        save_case(self.root, [(True, True)] * 2, amounts=("0.00000000000000000002", "0.00000000000000000001"))
        self.assertEqual(self.report()["economics"]["provider_billed"]["relative_reduction"], "0.5")

    def test_unreconciled_known_charge_stays_visible_but_blocks_complete_ratio(self):
        m, ep, ev = save_case(self.root, [(True, True)] * 2)
        ev[1]["cost"].update(reconciled=False, missing_reason="provider_charge_unreconciled")
        write_case(self.root, m, ep, ev)
        r = self.report()
        self.assertFalse(r["evidence_complete"])
        self.assertEqual(r["accounting"]["baseline"]["provider_billed"]["known_subtotal"], "2")

    def test_degenerate_cost_interval_does_not_assert_zero_risk(self):
        save_case(self.root, [(True, True)] * 3, amounts=("1", "0.5"))
        r = self.report()
        stat = r["uncertainty"]["relative_reduction"]["provider_billed"]
        self.assertIsNone(stat["interval"])
        self.assertEqual(stat["reason"], "degenerate_resamples")
        self.assertFalse(r["economics_gate"])

    def test_diagnostic_readiness_never_claims_pilot_or_qualification(self):
        save_case(self.root)
        r = self.report()
        ready = cd.readiness(self.load(), r)
        self.assertEqual(ready["phases"]["P3"], "pending_pilot_evidence")
        self.assertEqual(ready["phases"]["P4"], "blocked")
        self.assertFalse(ready["qualified"])

    def test_repeated_trials_remain_task_clusters(self):
        save_case(self.root, [(True, True), (True, False)], trials=3)
        r = self.report()
        self.assertEqual(r["counts"]["planned"], 12)
        self.assertEqual(r["uncertainty"]["task_clusters"], 2)
        self.assertEqual(r["counts"]["verified_successes"]["baseline"], 6)
        self.assertEqual(r["economics"]["provider_billed"]["baseline"]["cost_per_verified_success"], "1")

    def test_changed_identity_and_unknown_included_categories_never_clear_gate(self):
        m, ep, ev = save_case(self.root, [(True, True)] * 2, amounts=("1", "0.5"))
        m["analysis"]["mode"] = "claim"
        m["cost_policy"]["categories"]["infrastructure"]["coverage"] = "unknown"
        rebind(self.root, m, ep, ev)
        r = self.report()
        self.assertEqual(r["verdict"], "inconclusive")
        self.assertIn("unknown_cost_coverage", r["claim_blockers"])
        self.assertIn("not_held_out", r["claim_blockers"])
        self.assertFalse(r["quality_gate"])

    def test_supplied_output_and_verifier_evidence_artifacts_are_integrity_checked(self):
        m, ep, ev = save_case(self.root, [(True, True)] * 2)
        m["bindings"]["artifacts"][H] = "output.txt"
        (self.root / "output.txt").write_bytes(b"NOT THE HASHED OUTPUT")
        write_case(self.root, m, ep, ev)
        with self.assertRaises(ValueError):
            self.load()

    def test_supplied_price_evidence_is_integrity_checked(self):
        m, ep, ev = save_case(self.root)
        m["cost_policy"]["rates"] = [{"model": "synthetic-model", "metric": "input", "rate": "0", "per": 1,
                                      "currency": "USD", "evidence_hash": H}]
        m["bindings"]["artifacts"][H] = "prices.json"
        (self.root / "prices.json").write_bytes(b"NOT THE HASHED PRICE SOURCE")
        rebind(self.root, m, ep, ev)
        with self.assertRaises(ValueError):
            self.load()

    def test_high_precision_confidence_does_not_round_to_one_and_crash(self):
        m, ep, ev = save_case(self.root, [(True, True)] * 2)
        a = m["analysis"]
        a.update(confidence="0.999999999999999999999999999999", method="task_hoeffding_bonferroni")
        review = {"schema_version": "1.0", "kind": "contextdoctor.method_review",
                  "analysis_hash": cd.canonical_digest({k: v for k, v in a.items() if k != "review_hash"}),
                  "method": a["method"], "independent": True, "approved": True,
                  "reviewer": "synthetic-test-reviewer", "evidence_hash": H}
        raw = encode(review)
        h = digest(raw)
        (self.root / "review.json").write_bytes(raw)
        a["review_hash"] = h
        m["bindings"]["artifacts"][h] = "review.json"
        rebind(self.root, m, ep, ev)
        r = self.report()
        self.assertFalse(r["quality_gate"])
        self.assertIn("quality_uncertainty_crosses_boundary", r["claim_blockers"])


class GoldenFixtures(unittest.TestCase):
    def test_public_synthetic_controls_have_stable_reports(self):
        root = Path(__file__).parent / "tasks/contextdoctor"
        for name, verdict, baseline, candidate in (("identity", "no_effect", "1", "1"),
                                                    ("cheaper_worse", "regressed", "1.25", "2")):
            folder = root / name
            self.assertTrue((folder / "experiment.json").is_file(), "synthetic fixtures are missing")
            report = cd.build_report(cd.load_experiment(folder / "experiment.json", folder / "episodes.jsonl"))
            self.assertEqual(report["verdict"], verdict)
            self.assertEqual(report["economics"]["provider_billed"]["baseline"]["cost_per_verified_success"], baseline)
            self.assertEqual(report["economics"]["provider_billed"]["candidate"]["cost_per_verified_success"], candidate)
            self.assertEqual(json.loads((folder / "expected-report.json").read_text()), report)
            self.assertEqual((folder / "expected-report.md").read_text(encoding="utf-8"), cd.render_markdown(report))


class Legacy(Contracts):
    def test_legacy_scientific_numeric_cost_is_retained_exactly(self):
        m, _, _ = save_case(self.root)
        record = {"schema_version": "1.0", "run_id": "old", "task_id": "t0",
                  "environment_snapshot": H, "arm": "raw", "model": m["pins"]["model"],
                  "outcome": "failure", "evidence": ["synthetic"], "seed": 0, "attempt": 1,
                  "measurement": {"reported_cost": 1e-8}}
        path = self.root / "legacy.jsonl"
        path.write_bytes((json.dumps(record) + "\n").encode())
        imported = cd.import_legacy(m, path)
        cost = imported["unreconciled_cost_observations"][0]
        self.assertEqual(cost["amount"], "0.00000001")
        self.assertEqual(cost["basis"], "unclassified")
        self.assertIsNone(cost["currency"])
        with self.assertRaises(ValueError):
            cd.decimal("1e-8", "new schema still requires fixed decimals")

    def test_legacy_missing_optional_policy_remains_diagnostic(self):
        m, _, _ = save_case(self.root)
        record = {"schema_version": "1.0", "run_id": m["experiment_id"], "task_id": "t0",
                  "environment_snapshot": H, "arm": "raw", "model": m["pins"]["model"],
                  "outcome": "success", "evidence": ["synthetic"], "seed": 0, "attempt": 1}
        path = self.root / "legacy.jsonl"
        path.write_bytes((json.dumps(record) + "\n").encode())
        imported = cd.import_legacy(m, path)
        self.assertEqual(imported["records_retained"], 1)
        self.assertIn("legacy_identity_unmapped", imported["claim_blockers"])
        self.assertIsNone(imported["cost_per_verified_success"])

    def test_legacy_import_keeps_unknown_and_unpaired_records_diagnostic_only(self):
        m, _, _ = save_case(self.root)
        rows = [{"schema_version": "1.0", "run_id": "old", "task_id": "t0", "environment_snapshot": H,
                 "arm": a, "model": "synthetic-model", "policy_revision": None, "seed": 0, "attempt": 1,
                 "outcome": "unknown" if a == "candidate" else "success", "evidence": ["synthetic"],
                 "measurement": {"reported_cost": "3"}} for a in ("raw", "candidate")]
        raw = "".join(json.dumps(r) + "\n" for r in rows).encode()
        (self.root / "legacy.jsonl").write_bytes(raw)
        imported = cd.import_legacy(m, self.root / "legacy.jsonl")
        self.assertEqual(imported["records_retained"], 2)
        self.assertEqual(imported["source_hash"], digest(raw))
        self.assertIn("legacy_identity_unmapped", imported["claim_blockers"])
        self.assertIsNone(imported["cost_per_verified_success"])
        self.assertEqual((self.root / "legacy.jsonl").read_bytes(), raw)

    def test_cross_experiment_legacy_records_are_never_paired(self):
        m, _, _ = save_case(self.root)
        rows = [{"schema_version": "1.0", "run_id": run, "task_id": "t0", "environment_snapshot": H,
                 "arm": arm, "model": "synthetic-model", "policy_revision": None, "seed": 0, "attempt": 1,
                 "outcome": "success", "evidence": ["synthetic"]} for run, arm in (("one", "raw"), ("two", "candidate"))]
        (self.root / "legacy.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
        r = cd.import_legacy(m, self.root / "legacy.jsonl")
        self.assertEqual(sum(g["paired_count"] for g in r["groups"]), 0)


def rebind(root, m, ep, ev):
    """Keep test evidence internally consistent when deliberately changing the scope."""
    m["content_hash"] = cd.canonical_digest(m)
    for e in ep:
        e["manifest_hash"] = m["content_hash"]
        e["pins_hash"] = cd.canonical_digest(m["pins"])
        e["transform_hash"] = cd.canonical_digest(m["arms"][e["arm"]])
        if e["verification_hash"]:
            old_hash = e["verification_hash"]
            v = json.loads((root / m["bindings"]["artifacts"][old_hash]).read_text())
            v["manifest_hash"] = m["content_hash"]
            raw = encode(v)
            h = digest(raw)
            name = f"{e['id']}.verification.json"
            (root / name).write_bytes(raw)
            m["bindings"]["artifacts"].pop(old_hash)
            m["bindings"]["artifacts"][h] = name
            e["verification_hash"] = h
    for e in ev:
        e["manifest_hash"] = m["content_hash"]
    write_case(root, m, ep, ev)


if __name__ == "__main__":
    unittest.main()
