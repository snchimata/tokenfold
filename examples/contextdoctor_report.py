"""Compare two recorded synthetic controls, offline and without optional packages.

From a source checkout: python examples/contextdoctor_report.py
These fixture charges are invented test data, not measured provider savings.
"""

import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "eval"))
import contextdoctor as cd


def main():
    rows = []
    for control, verdict in (("identity", "no_effect"), ("cheaper_worse", "regressed")):
        fixture = ROOT / "eval" / "tasks" / "contextdoctor" / control
        experiment = cd.load_experiment(fixture / "experiment.json", fixture / "episodes.jsonl")
        report = cd.build_report(experiment)
        readiness = cd.readiness(experiment, report)
        assert report["verdict"] == verdict
        assert readiness["phases"]["P4"] == "blocked"
        assert not readiness["expansion_allowed"]
        rows.append({
            "control": control,
            "evidence_class": report["evidence_class"],
            "verified_successes": report["counts"]["verified_successes"],
            "regressions": report["paired"]["outcomes"]["baseline_only_success"],
            "cost_basis": "provider_billed (synthetic fixture)",
            "cost_scope": report["cost_scope"]["label"],
            "cost_per_verified_success": {
                arm: report["economics"]["provider_billed"][arm]["cost_per_verified_success"]
                for arm in cd.ARMS
            },
            "verdict": report["verdict"],
            "p4_status": readiness["phases"]["P4"],
            "expansion_allowed": readiness["expansion_allowed"],
        })
    print(json.dumps(rows, indent=2))


if __name__ == "__main__":
    main()
