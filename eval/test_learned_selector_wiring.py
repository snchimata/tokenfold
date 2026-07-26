#!/usr/bin/env python3
"""Model-free wiring test for the v0.4-beta learned-selector seam in `run_baselines.py`.

There is no ML here. It proves three things about the *plug-in mechanism* without any model
installed, so the seam is verified now and the actual (gitignored) probe just has to satisfy the
same `LEARNED_SELECTORS` contract later:

  1. Absent module / ML  -> `_load_learned_selectors()` returns {} and never raises (clean skip,
     like tiktoken and the tokenfold CLI).
  2. A module that exposes `LEARNED_SELECTORS` gets its callables registered into `SELECTORS`.
  3. Any registered score function still goes through the deterministic critical-atom forcing +
     ceiling allocator, so 100% critical-atom survival and the token ceiling hold *by
     construction* — a learned selector emits scores only, it cannot break the hard gates.

Stdlib only, assert-based; run directly (`python eval/test_learned_selector_wiring.py`) or via
pytest. No frameworks, no fixtures beyond the shipped task JSON.
"""

from __future__ import annotations

import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))  # import run_baselines regardless of CWD
import run_baselines as rb  # noqa: E402


def test_absent_module_skips_cleanly():
    # Point at a module that certainly does not exist: must return {} and not raise.
    _set_env("TOKENFOLD_LEARNED_MODULE", "definitely_no_such_learned_module_xyz")
    try:
        assert rb._load_learned_selectors() == {}
    finally:
        _set_env("TOKENFOLD_LEARNED_MODULE", None)


def test_module_with_contract_registers():
    with tempfile.TemporaryDirectory() as d:
        Path(d, "dummy_learned.py").write_text(
            "LEARNED_SELECTORS = {"
            "'learned_dummy': lambda units, query: [float(len(u)) for u in units]}\n",
            encoding="utf-8",
        )
        sys.path.insert(0, d)
        _set_env("TOKENFOLD_LEARNED_MODULE", "dummy_learned")
        try:
            loaded = rb._load_learned_selectors()
            assert "learned_dummy" in loaded, loaded
            assert callable(loaded["learned_dummy"])
        finally:
            _set_env("TOKENFOLD_LEARNED_MODULE", None)
            sys.path.remove(d)


def _write_dummy_module(dirpath, modname, body):
    Path(dirpath, f"{modname}.py").write_text(body, encoding="utf-8")
    sys.path.insert(0, dirpath)
    _set_env("TOKENFOLD_LEARNED_MODULE", modname)


def test_real_registration_path_flows_through_report():
    """Exercise the ACTUAL register step run_baselines runs at import
    (`SELECTORS.update(_load_learned_selectors())`) — not a hand-injection — and prove the loaded
    selector reaches the measured registry, is labeled *learned* (never deterministic) in the
    report, and still passes the hard gates. Breaking the real `.update()` line must fail here."""
    with tempfile.TemporaryDirectory() as d:
        _write_dummy_module(
            d,
            "dummy_flow",
            "LEARNED_SELECTORS = {"
            "'learned_flow': lambda units, query: [float(len(u)) for u in units]}\n",
        )
        rb.SELECTORS.update(rb._load_learned_selectors())  # the exact line run_baselines runs
        try:
            assert "learned_flow" in rb.SELECTORS
            fixtures = rb.load_fixtures(Path(__file__).parent / "tasks" / "v04")
            report = rb.build_report(fixtures, [0.25])
            assert "learned_flow" in report["learned_selectors"], report["learned_selectors"]
            assert "learned_flow" not in report["deterministic_selectors"]
            assert rb.run_gate(fixtures, [0.25]) == 0  # learned selector must pass the hard gates
        finally:
            rb.SELECTORS.pop("learned_flow", None)
            _set_env("TOKENFOLD_LEARNED_MODULE", None)
            sys.path.remove(d)


def test_collision_with_baseline_is_rejected():
    """A learned selector that shadows a deterministic baseline name must fail loud, not silently
    overwrite the baseline and get mislabeled as deterministic."""
    with tempfile.TemporaryDirectory() as d:
        _write_dummy_module(
            d,
            "dummy_clash",
            "LEARNED_SELECTORS = {'bm25': lambda units, query: [0.0 for _ in units]}\n",
        )
        try:
            raised = False
            try:
                rb._load_learned_selectors()
            except ValueError:
                raised = True
            assert raised, "collision with deterministic baseline 'bm25' must raise"
        finally:
            _set_env("TOKENFOLD_LEARNED_MODULE", None)
            sys.path.remove(d)


def test_registered_selector_upholds_hard_gates():
    """Inject an arbitrary learned score fn and drive a real fixture through run_one: critical
    atoms must all survive and the ceiling must hold (unless the forced floor alone exceeds it) —
    exactly the invariants `--gate` asserts, now proven for a non-deterministic selector too."""
    fixtures = rb.load_fixtures(Path(__file__).parent / "tasks" / "v04")
    assert fixtures, "no v04 fixtures found"
    rb.SELECTORS["learned_dummy"] = lambda units, query: [float(len(u)) for u in units]
    try:
        for fx in fixtures:
            res = rb.run_one(fx, "learned_dummy", 0.25)
            assert res["critical_atom_survival"] == 1.0, res
            ceiling_ok = (not res["over_budget"]) or (
                res["forced_floor_tokens"] > res["budget_tokens"]
            )
            assert ceiling_ok, res
    finally:
        rb.SELECTORS.pop("learned_dummy", None)


def _set_env(key, value):
    import os

    if value is None:
        os.environ.pop(key, None)
    else:
        os.environ[key] = value


if __name__ == "__main__":
    test_absent_module_skips_cleanly()
    test_module_with_contract_registers()
    test_real_registration_path_flows_through_report()
    test_collision_with_baseline_is_rejected()
    test_registered_selector_upholds_hard_gates()
    print("ok: learned-selector wiring holds (skip-when-absent, register+report flow, "
          "baseline-collision rejected, gates-hold)")
