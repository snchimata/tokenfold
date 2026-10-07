"""Reuse the shipped experimental Select runtime for model-ranked observation groups."""

import hashlib
import json
import subprocess
import sys
from pathlib import Path

from openrouter_model import ModelError, STOP_REASONS


def source_context(task: dict) -> dict:
    context = task.get("select_context", {"groups": [{"id": "whole-source", "text": task["source"]}]})
    if not isinstance(context, dict) or set(context) - {"prefix", "suffix", "groups"}:
        raise ValueError("invalid caller grouping")
    groups = context.get("groups")
    if not isinstance(groups, list) or not groups or any(not isinstance(g, dict) for g in groups):
        raise ValueError("invalid caller groups")
    if (len(groups) > 512 or any(not isinstance(g.get("text"), str) for g in groups)
            or not isinstance(context.get("prefix", ""), str) or not isinstance(context.get("suffix", ""), str)
            or context.get("prefix", "") + "".join(g["text"] for g in groups) + context.get("suffix", "") != task["source"]):
        raise ValueError("grouping must reconstruct exact source, without gold-derived protection")
    return context


class SelectArm:
    def __init__(self, model, binary: Path, directory: Path, compact: bool = False):
        self.model, self.binary, self.directory = model, binary.resolve(), directory.resolve()
        self.directory.mkdir(parents=True, exist_ok=False)
        self.last_receipt = None
        self.compact = compact

    def __call__(self, task: dict, target: int, seed: int) -> str:
        context = source_context(task)
        self.last_receipt = None
        if self.model.stop_reason is not None:
            raise ModelError(self.model.stop_reason)
        if len(self.model.calls) >= self.model.max_calls:
            raise ModelError("call_budget_exhausted")
        metrics = self.directory / "last-call.json"
        receipt = self.directory / "last-receipt.json"
        for path in (metrics, receipt):
            path.unlink(missing_ok=True)
        script = Path(__file__).with_name("openrouter_scorer.py").resolve()
        runtime_timeout = min(25, self.model.timeout)
        config = {
            "executable": str(Path(sys.executable).resolve()),
            "executable_sha256": hashlib.sha256(Path(sys.executable).read_bytes()).hexdigest(),
            "arguments": ["-I", str(script), "--model", self.model.name, "--revision", self.model.digest,
                          "--env-file", self.model.env_file, "--metrics-file", str(metrics),
                          "--timeout", str(runtime_timeout), "--output-tokens", str(self.model.output_tokens),
                          "--context-tokens", str(self.model.context_tokens), "--seed", str(seed)],
            "model_revision": self.model.digest,
            "approved_by": "explicit-hosted-public-data-benchmark",
            "scratch_root": str(self.directory),
        }
        config_path = self.directory / "approval.json"
        if self.compact:
            config["arguments"].append("--compact")
        config_path.write_text(json.dumps(config, indent=2), encoding="utf-8")
        pending = {"status": "unknown", "usage": None, "cost": None, "wall_ms": None,
                   "reason": "runtime_interrupted", "attempted": None}
        self.model.calls.append(pending)  # Reserve the runtime's one model call.
        index = len(self.model.calls) - 1
        try:
            result = subprocess.run([
                str(self.binary), "--experimental", "select", "--query", task["query"],
                "--target-tokens", str(target), "--scorer-config", str(config_path),
                "--inference-timeout-ms", str(min(30000, round(self.model.timeout * 1000))),
                "--receipt-file", str(receipt),
            ], input=json.dumps(context), encoding="utf-8", capture_output=True,
                timeout=min(30, self.model.timeout) + 10)
            if result.returncode:
                raise ModelError("select_cli_refused")
            self.last_receipt = json.loads(receipt.read_text(encoding="utf-8"))
            # A model outage/fallback is not valid evidence about model-ranked pruning.
            # Keep the fallback receipt, but don't score it as a model quality win.
            if not self.last_receipt.get("used_scorer"):
                raise ModelError("select_scorer_not_used")
            return result.stdout
        finally:
            if metrics.exists():
                calls = json.loads(metrics.read_text(encoding="utf-8"))
                if isinstance(calls, list) and len(calls) == 1:
                    self.model.calls[index] = calls[0]
                    if (calls[0].get("reason") in STOP_REASONS or
                            type(calls[0].get("cost")) in (int, float) and calls[0]["cost"] > 0):
                        self.model.max_calls = len(self.model.calls)
                        self.model.stop_reason = calls[0].get("reason") if calls[0].get("reason") in STOP_REASONS else "unexpected_charge"
