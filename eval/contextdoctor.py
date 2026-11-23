#!/usr/bin/env python3
"""Experimental offline context evaluation. Hashes establish integrity, not authorship."""
from __future__ import annotations

import argparse
from collections import Counter, defaultdict
from decimal import Decimal, InvalidOperation, localcontext, ROUND_HALF_EVEN
import hashlib
import json
from pathlib import Path, PureWindowsPath
import random
import re
import sys

SCHEMA_VERSION = "1.0"
ARMS = ("baseline", "candidate")
CATEGORIES = ("model", "transform", "tool", "verification", "infrastructure")
BASES = ("provider_billed", "usage_priced", "modeled", "unavailable")
METRICS = ("input", "cache_read", "cache_write", "output")
DISPOSITIONS = ("applied", "no_op", "fallback", "refused", "failed", "unknown")
STATUSES = ("completed", "failed", "timeout", "cancelled", "invalid", "unknown", "unexecuted")
EXACT_VERIFIER = "contextdoctor.exact_utf8_match.v1"
MAX_ARTIFACT_BYTES = 32 * 1024 * 1024


class ContractError(ValueError):
    """An invalid or unsupported evidence contract."""


def require(condition, message):
    if not condition:
        raise ContractError(message)


def fields(value, expected, where):
    require(isinstance(value, dict), f"{where}: expected object")
    require(set(value) == set(expected.split()), f"{where}: incorrect fields (missing/extra)")


def choice(value, choices, where):
    require(isinstance(value, str) and value in choices, f"{where}: unsupported value")


def identifier(value, where):
    require(isinstance(value, str) and re.fullmatch(r"[A-Za-z0-9_.-]{1,100}", value), f"{where}: invalid ID")


def string(value, where):
    require(isinstance(value, str) and bool(value.strip()) and len(value) <= 4096, f"{where}: expected nonempty string")


def hash_value(value, where, nullable=False):
    if nullable and value is None:
        return
    require(isinstance(value, str) and re.fullmatch(r"sha256:[0-9a-f]{64}", value), f"{where}: invalid SHA-256")


def integer(value, where, minimum=0, maximum=10**12):
    require(type(value) is int and minimum <= value <= maximum, f"{where}: invalid integer")


def decimal(value, where, nullable=False):
    if nullable and value is None:
        return None
    require(isinstance(value, str) and re.fullmatch(r"(?:0|[1-9][0-9]{0,29})(?:\.[0-9]{1,30})?", value),
            f"{where}: expected nonnegative finite decimal string")
    try:
        number = Decimal(value)
    except InvalidOperation as exc:
        raise ContractError(f"{where}: invalid decimal") from exc
    require(number.is_finite() and number >= 0, f"{where}: invalid decimal")
    return number


def hash_bytes(data):
    return "sha256:" + hashlib.sha256(data).hexdigest()


def canonical_digest(value):
    """UTF-8, sorted keys, compact JSON, unescaped Unicode; no floating-point values."""
    if isinstance(value, dict) and value.get("kind") == "contextdoctor.experiment":
        value = {k: v for k, v in value.items() if k not in ("content_hash", "bindings")}
    return hash_bytes(json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":"), allow_nan=False).encode("utf-8"))


def _pairs(items):
    value = {}
    for key, item in items:
        require(key not in value, "duplicate JSON key")
        value[key] = item
    return value


def _not_integer(value):
    raise ContractError("JSON nonfinite/floating numbers are forbidden; use decimal strings")


def parse_json(raw):
    try:
        return json.loads(raw, object_pairs_hook=_pairs, parse_float=_not_integer, parse_constant=_not_integer)
    except (json.JSONDecodeError, UnicodeError, RecursionError) as exc:
        raise ContractError("invalid JSON") from exc


def read_bytes(path):
    require(path.is_file(), "artifact is not a regular file")
    require(path.stat().st_size <= MAX_ARTIFACT_BYTES, "artifact exceeds size limit")
    with path.open("rb") as handle:
        raw = handle.read(MAX_ARTIFACT_BYTES + 1)
    require(len(raw) <= MAX_ARTIFACT_BYTES, "artifact exceeds size limit")
    return raw


def safe_path(root, relative):
    require(isinstance(relative, str) and relative and "\x00" not in relative, "unsafe artifact path")
    windows = PureWindowsPath(relative)
    require(not windows.anchor and not windows.drive and ":" not in relative, "absolute artifact path")
    parts = relative.replace("\\", "/").split("/")
    require(all(p not in ("", ".", "..") for p in parts), "unsafe artifact path")
    path = (root / Path(*parts)).resolve()
    require(path.is_relative_to(root.resolve()), "artifact escapes root")
    return path


def header(value, kind):
    require(value.get("schema_version") == SCHEMA_VERSION, "unsupported schema version")
    require(value.get("kind") == f"contextdoctor.{kind}", "incorrect artifact kind")


def _unique(rows, key, where):
    require(isinstance(rows, list), f"{where}: expected list")
    seen = set()
    for row in rows:
        require(isinstance(row, dict) and key in row, f"{where}: invalid row")
        identifier(row[key], where)
        require(row[key] not in seen, f"{where}: duplicate ID")
        seen.add(row[key])
    return seen


def validate_manifest(m):
    fields(m, "schema_version kind experiment_id content_hash arms tasks pins verifier schedule limits cost_policy analysis bindings execution rollout", "manifest")
    header(m, "experiment")
    identifier(m["experiment_id"], "experiment_id")
    hash_value(m["content_hash"], "content_hash")
    require(canonical_digest(m) == m["content_hash"], "manifest content hash mismatch")
    fields(m["arms"], "baseline candidate", "arms")
    for arm in ARMS:
        a = m["arms"][arm]
        fields(a, "name transformation config_hash", "arm")
        identifier(a["name"], "arm name")
        choice(a["transformation"], ("identity", "tokenfold", "external"), "transformation")
        hash_value(a["config_hash"], "config_hash")
    ids = _unique(m["tasks"], "id", "tasks")
    require(bool(ids), "no tasks")
    classes = set()
    for t in m["tasks"]:
        fields(t, "id source_hash prompt_hash snapshot_hash reference_hash provenance", "task")
        for key in ("source_hash", "prompt_hash", "snapshot_hash", "reference_hash"):
            hash_value(t[key], key)
        p = t["provenance"]
        fields(p, "class family selection permission exposure", "provenance")
        choice(p["class"], ("synthetic_mechanics", "development", "held_out"), "evidence class")
        classes.add(p["class"])
        for key in ("family", "selection", "permission", "exposure"):
            string(p[key], key)
    require(len(classes) == 1, "do not mix evidence classes")
    fields(m["pins"], "model revision runtime device dtype tokenizer_hash settings_hash tools_hash cache_policy", "pins")
    for key, value in m["pins"].items():
        (hash_value if key.endswith("_hash") else string)(value, key)
    v = m["verifier"]
    fields(v, "digest kind reference_health spec_hash", "verifier")
    hash_value(v["digest"], "verifier digest")
    hash_value(v["spec_hash"], "verifier spec")
    choice(v["kind"], ("recorded", "exact_match"), "verifier kind")
    choice(v["reference_health"], ("passed", "invalid"), "reference health")
    require(isinstance(m["schedule"], list) and bool(m["schedule"]), "empty schedule")
    seen, trials = set(), defaultdict(set)
    for cell in m["schedule"]:
        fields(cell, "task_id trial_id order", "schedule")
        identifier(cell["task_id"], "scheduled task")
        require(cell["task_id"] in ids, "unknown scheduled task")
        identifier(cell["trial_id"], "trial_id")
        key = (cell["task_id"], cell["trial_id"])
        require(key not in seen, "duplicate scheduled trial")
        seen.add(key)
        require(cell["order"] in (["baseline", "candidate"], ["candidate", "baseline"]), "invalid arm order")
        trials[cell["task_id"]].add(cell["trial_id"])
    require(set(trials) == ids and len({tuple(sorted(t)) for t in trials.values()}) == 1, "unbalanced task/trial grid")
    limits = m["limits"]
    fields(limits, "max_operations max_seconds operation_seconds max_output_bytes max_tokens retries max_cost", "limits")
    for k in ("max_operations", "max_output_bytes", "max_tokens"):
        integer(limits[k], k, 1)
    integer(limits["retries"], "retries", 0, 0)
    for k in ("max_seconds", "operation_seconds", "max_cost"):
        decimal(limits[k], k)
    require(decimal(limits["operation_seconds"], "operation seconds") > 0 and
            decimal(limits["max_seconds"], "max seconds") > 0, "zero time limit")
    cp = m["cost_policy"]
    fields(cp, "currency rounding categories rates", "cost_policy")
    require(isinstance(cp["currency"], str) and re.fullmatch("[A-Z]{3}", cp["currency"]), "invalid currency")
    require(cp["rounding"] == "ROUND_HALF_EVEN", "unsupported rounding")
    fields(cp["categories"], " ".join(CATEGORIES), "cost categories")
    for c in cp["categories"].values():
        fields(c, "coverage basis allocation", "category")
        choice(c["coverage"], ("included", "excluded", "unknown"), "coverage")
        choice(c["basis"], BASES, "basis")
        choice(c["allocation"], ("per_episode", "equal_arms"), "allocation")
    require(isinstance(cp["rates"], list), "rates must be a list")
    prices = set()
    for rate in cp["rates"]:
        fields(rate, "model metric rate per currency evidence_hash", "rate")
        require(rate["model"] == m["pins"]["model"], "rate model mismatch")
        choice(rate["metric"], METRICS, "rate metric")
        require(rate["metric"] not in prices, "duplicate price")
        prices.add(rate["metric"])
        decimal(rate["rate"], "rate")
        integer(rate["per"], "rate unit", 1)
        require(rate["currency"] == cp["currency"], "incompatible rate currency")
        hash_value(rate["evidence_hash"], "price evidence")
    a = m["analysis"]
    fields(a, "mode seed resamples confidence min_tasks min_trials noninferiority_margin regression_ceiling method multiplicity sample_size_rationale review_hash", "analysis")
    choice(a["mode"], ("diagnostic", "claim"), "analysis mode")
    integer(a["seed"], "seed")
    integer(a["resamples"], "resamples", 100, 10000)
    integer(a["min_tasks"], "min_tasks", 2)
    integer(a["min_trials"], "min_trials", 1)
    for k in ("confidence", "noninferiority_margin", "regression_ceiling"):
        require(decimal(a[k], k) <= 1, f"{k}: exceeds one")
    require(Decimal("0.5") < Decimal(a["confidence"]) < 1, "invalid confidence")
    choice(a["method"], ("task_cluster_percentile", "task_hoeffding_bonferroni"), "analysis method")
    require(a["multiplicity"] == "single_contrast", "unsupported multiplicity policy")
    string(a["sample_size_rationale"], "sample size rationale")
    hash_value(a["review_hash"], "review_hash", True)
    fields(m["bindings"], "operations artifacts", "bindings")
    require(isinstance(m["bindings"]["artifacts"], dict), "artifact bindings must be an object")
    for h, path in m["bindings"]["artifacts"].items():
        hash_value(h, "artifact hash")
        string(path, "artifact path")
    if m["execution"] is not None:
        e = m["execution"]
        fields(e, "adapter image agent_argv tokenfold_argv model provider settings catalog_hash", "execution")
        choice(e["adapter"], ("container", "openrouter_free"), "adapter")
        hash_value(e["image"], "image", True)
        hash_value(e["catalog_hash"], "catalog_hash", True)
        for key in ("agent_argv", "tokenfold_argv"):
            require(isinstance(e[key], list) and all(isinstance(x, str) and "\x00" not in x for x in e[key]), "invalid argv")
        require(e["model"] == m["pins"]["model"], "execution model mismatch")
        string(e["provider"], "provider")
        fields(e["settings"], "temperature max_tokens", "settings")
        require(decimal(e["settings"]["temperature"], "temperature") <= 2, "invalid temperature")
        integer(e["settings"]["max_tokens"], "max_tokens", 1, limits["max_tokens"])
        require(canonical_digest(e["settings"]) == m["pins"]["settings_hash"], "settings pin mismatch")
    if m["rollout"] is not None:
        r = m["rollout"]
        fields(r, "pilot_hash maintenance_minutes benefit_minutes repeat_uses decision", "rollout")
        hash_value(r["pilot_hash"], "pilot hash", True)
        decimal(r["maintenance_minutes"], "maintenance", True)
        decimal(r["benefit_minutes"], "benefit", True)
        integer(r["repeat_uses"], "repeat uses")
        choice(r["decision"], ("keep_small", "stop", "expand"), "decision")


def _jsonl(raw):
    try:
        return [parse_json(line) for line in raw.decode("utf-8").splitlines() if line.strip()]
    except UnicodeError as exc:
        raise ContractError("invalid UTF-8 records") from exc


def validate_cost(c, currency):
    fields(c, "basis amount currency receipt_hash reconciled missing_reason", "cost")
    choice(c["basis"], BASES, "cost basis")
    require(c["currency"] == currency, "incompatible cost currency")
    decimal(c["amount"], "cost amount", True)
    hash_value(c["receipt_hash"], "cost receipt", True)
    require(type(c["reconciled"]) is bool, "reconciled must be boolean")
    if c["missing_reason"] is not None:
        identifier(c["missing_reason"], "missing cost reason")
    if c["basis"] == "unavailable":
        require(c["amount"] is None and c["missing_reason"] is not None, "unavailable cost needs reason, not amount")
    if c["amount"] is not None and c["basis"] == "provider_billed":
        require(c["receipt_hash"] is not None, "billed cost requires a receipt")
        require(c["reconciled"] or c["missing_reason"] is not None, "unreconciled charge needs a reason")


def normalized_usage(u, model):
    if u is None:
        return None
    fields(u, "model source normalization snapshots", "usage")
    require(u["model"] == model, "usage model mismatch")
    choice(u["source"], ("provider_observed", "local_estimate"), "usage source")
    choice(u["normalization"], ("disjoint_v1", "cumulative_last_v1"), "usage normalization")
    require(isinstance(u["snapshots"], list) and bool(u["snapshots"]), "empty usage snapshots")
    previous = {key: 0 for key in METRICS}
    for counters in u["snapshots"]:
        fields(counters, " ".join(METRICS), "usage counters")
        for k in METRICS:
            integer(counters[k], k)
            require(counters[k] >= previous[k], "decreasing cumulative usage")
        previous = counters
    require(u["normalization"] != "disjoint_v1" or len(u["snapshots"]) == 1, "disjoint usage must have one snapshot")
    return previous


def load_manifest(path):
    raw = read_bytes(path)
    m = parse_json(raw)
    validate_manifest(m)
    root = path.parent.resolve()
    safe_path(root, m["bindings"]["operations"])
    for relative in m["bindings"]["artifacts"].values():
        safe_path(root, relative)
    return m, raw


def load_experiment(manifest_path, records_path):
    m, manifest_raw = load_manifest(manifest_path)
    root = manifest_path.parent.resolve()
    episode_raw = read_bytes(records_path)
    operation_raw = read_bytes(safe_path(root, m["bindings"]["operations"]))
    episodes, events = _jsonl(episode_raw), _jsonl(operation_raw)
    _unique(episodes, "id", "episodes")
    artifact_cache = {}
    verified_artifacts = set()

    def artifact(h, cache=False):
        if h in artifact_cache:
            return artifact_cache[h]
        require(h in m["bindings"]["artifacts"], "unbound artifact hash")
        raw = read_bytes(safe_path(root, m["bindings"]["artifacts"][h]))
        require(hash_bytes(raw) == h, "artifact digest mismatch")
        verified_artifacts.add(h)
        if cache:
            artifact_cache[h] = raw
        return raw

    def check_optional(h):
        if h in m["bindings"]["artifacts"] and h not in verified_artifacts:
            artifact(h)

    for task in m["tasks"]:
        for k in ("source_hash", "prompt_hash", "snapshot_hash", "reference_hash"):
            check_optional(task[k])
    for rate in m["cost_policy"]["rates"]:
        check_optional(rate["evidence_hash"])

    for e in episodes:
        fields(e, "schema_version kind id experiment_id manifest_hash task_id trial_id arm pins_hash source_hash prompt_hash snapshot_hash transform_hash status request_hash output_hash verification_hash treatment operation_ids wall_seconds safety_violation", "episode")
        header(e, "episode")
        for k in ("experiment_id", "task_id", "trial_id"):
            identifier(e[k], k)
        choice(e["arm"], ARMS, "episode arm")
        choice(e["status"], STATUSES, "episode status")
        for k in ("manifest_hash", "pins_hash", "source_hash", "prompt_hash", "snapshot_hash", "transform_hash"):
            hash_value(e[k], k)
        for k in ("request_hash", "output_hash", "verification_hash"):
            hash_value(e[k], k, True)
            check_optional(e[k])
        decimal(e["wall_seconds"], "wall seconds", True)
        require(type(e["safety_violation"]) is bool, "invalid safety flag")
        t = e["treatment"]
        fields(t, "enabled disposition input_hash output_hash receipt_hash", "treatment")
        require(type(t["enabled"]) is bool, "invalid enabled flag")
        choice(t["disposition"], DISPOSITIONS, "disposition")
        for k in ("input_hash", "output_hash", "receipt_hash"):
            hash_value(t[k], k, True)
            check_optional(t[k])
        if t["receipt_hash"] is not None:
            artifact(t["receipt_hash"])
        require(isinstance(e["operation_ids"], list) and all(isinstance(o, str) for o in e["operation_ids"]) and
                len(set(e["operation_ids"])) == len(e["operation_ids"]), "duplicate/invalid operation references")
        for op in e["operation_ids"]:
            identifier(op, "operation reference")
        if e["verification_hash"] is not None:
            v = parse_json(artifact(e["verification_hash"], cache=True))
            fields(v, "schema_version kind episode_id manifest_hash output_hash verifier_hash independence reference_health status checks evidence_hash", "verification")
            header(v, "verification")
            identifier(v["episode_id"], "verified episode")
            for k in ("manifest_hash", "output_hash", "verifier_hash", "evidence_hash"):
                hash_value(v[k], k)
                check_optional(v[k])
            choice(v["independence"], ("external", "agent"), "independence")
            choice(v["reference_health"], ("passed", "invalid"), "verifier health")
            choice(v["status"], ("passed", "failed", "invalid", "unknown"), "verification status")
            _unique(v["checks"], "id", "checks")
            for check in v["checks"]:
                fields(check, "id result", "check")
                choice(check["result"], ("passed", "failed"), "check result")
    operations = {}
    invoices = set()
    for event in events:
        fields(event, "schema_version kind operation_id episode_id experiment_id manifest_hash event role category status duration_seconds usage cost request_id", "operation")
        header(event, "operation")
        identifier(event["operation_id"], "operation_id")
        if event["episode_id"] is not None:
            identifier(event["episode_id"], "operation episode")
        identifier(event["experiment_id"], "operation experiment")
        hash_value(event["manifest_hash"], "operation manifest")
        choice(event["event"], ("dispatch", "terminal"), "event")
        choice(event["role"], ("agent", "transform", "tool", "verification", "setup"), "operation role")
        choice(event["category"], CATEGORIES, "operation category")
        choice(event["status"], ("dispatched", "completed", "failed", "timeout", "cancelled", "unknown"), "operation status")
        require((event["event"] == "dispatch") == (event["status"] == "dispatched"), "invalid event state")
        decimal(event["duration_seconds"], "duration", True)
        if event["request_id"] is not None:
            string(event["request_id"], "request ID")
        normalized_usage(event["usage"], m["pins"]["model"])
        validate_cost(event["cost"], m["cost_policy"]["currency"])
        oid = event["operation_id"]
        if event["event"] == "dispatch":
            require(oid not in operations, "duplicate operation dispatch")
        else:
            require(oid in operations and operations[oid]["event"] == "dispatch", "terminal without unique dispatch")
            for k in ("episode_id", "experiment_id", "manifest_hash", "role", "category"):
                require(event[k] == operations[oid][k], "operation identity changed")
            cost = event["cost"]
            if cost["basis"] == "provider_billed" and cost["receipt_hash"] is not None:
                require(cost["receipt_hash"] not in invoices, "duplicate billing receipt")
                invoices.add(cost["receipt_hash"])
        operations[oid] = event
    return {"manifest": m, "episodes": episodes, "operations": list(operations.values()),
            "artifacts": artifact_cache, "verified_artifacts": verified_artifacts, "artifact_reader": artifact,
            "input_hashes": {"manifest": hash_bytes(manifest_raw), "episodes": hash_bytes(episode_raw),
                             "operations": hash_bytes(operation_raw)}}


def number(value):
    """Render derived decimals to 12 places, half-even; never round ledger inputs."""
    if value is None:
        return None
    if not isinstance(value, Decimal):
        value = Decimal(str(value))
    value = value.quantize(Decimal("0.000000000001"), rounding=ROUND_HALF_EVEN)
    return format(value, "f").rstrip("0").rstrip(".") if value else "0"


def money(value):
    rendered = format(value, "f")
    return rendered.rstrip("0").rstrip(".") if "." in rendered else rendered


def operation_amount(op, m):
    c = op["cost"]
    if c["basis"] == "provider_billed":
        return decimal(c["amount"], "billed amount", True), c["missing_reason"]
    if c["basis"] == "unavailable":
        return None, c["missing_reason"]
    usage = op["usage"]
    if usage is None:
        return None, "missing_usage"
    require((c["basis"] == "usage_priced") == (usage["source"] == "provider_observed"), "usage/cost basis mismatch")
    counters = normalized_usage(usage, m["pins"]["model"])
    rates = {r["metric"]: r for r in m["cost_policy"]["rates"]}
    if any(k not in rates for k in METRICS):
        return None, "missing_price_category"
    amount = sum((Decimal(counters[k]) * Decimal(rates[k]["rate"]) / rates[k]["per"] for k in METRICS), Decimal(0))
    require(c["amount"] is None or Decimal(c["amount"]) == amount, "priced amount disagrees with frozen usage/rates")
    return amount, c["missing_reason"]


def episode_outcome(e, m, tasks, artifacts):
    reasons = []
    expected = {"experiment_id": m["experiment_id"], "manifest_hash": m["content_hash"],
                "pins_hash": canonical_digest(m["pins"]), "transform_hash": canonical_digest(m["arms"][e["arm"]])}
    if e["task_id"] in tasks:
        expected.update({k: tasks[e["task_id"]][k] for k in ("source_hash", "prompt_hash", "snapshot_hash")})
    else:
        reasons.append("unknown_task")
    if any(e[k] != v for k, v in expected.items()):
        reasons.append("episode_identity_mismatch")
    t = e["treatment"]
    if e["status"] != "unexecuted" and (t["input_hash"] != e["source_hash"] or
            t["output_hash"] is None or (t["disposition"] in ("no_op", "fallback") and t["input_hash"] != t["output_hash"])):
        reasons.append("treatment_hash_mismatch")
    if e["arm"] == "candidate" and not t["enabled"]:
        reasons.append("candidate_not_enabled")
    if reasons or e["status"] == "invalid":
        return "invalid", reasons or ["invalid_execution"]
    if e["status"] == "unexecuted":
        return "unexecuted", []
    if e["verification_hash"] is None:
        return "unknown", ["missing_verification"]
    v = parse_json(artifacts[e["verification_hash"]])
    if (v["episode_id"] != e["id"] or v["manifest_hash"] != m["content_hash"] or
            v["output_hash"] != e["output_hash"] or v["verifier_hash"] != m["verifier"]["digest"]):
        return "invalid", ["verification_binding_mismatch"]
    if (v["independence"] != "external" or v["reference_health"] != "passed" or
            m["verifier"]["reference_health"] != "passed" or v["status"] in ("invalid", "unknown") or not v["checks"]):
        return "unknown", ["unhealthy_or_nonindependent_verification"]
    passed = all(c["result"] == "passed" for c in v["checks"])
    if (v["status"] == "passed") != passed:
        return "invalid", ["inconsistent_verifier_result"]
    if e["status"] != "completed" and passed:
        return "invalid", ["success_without_completed_episode"]
    return ("success" if passed else "failure"), []


def _interval(values, undefined, resamples, confidence, reason=None):
    if reason is None and undefined:
        reason = "undefined_resamples"
    if reason is None and values and min(values) == max(values):
        reason = "degenerate_resamples"
    interval = None
    if reason is None and len(values) >= 2:
        values.sort()
        tail = (1 - Decimal(confidence)) / 2
        interval = [number(values[int(tail * len(values))]),
                    number(values[min(len(values) - 1, int((1 - tail) * len(values)))])]
    return {"interval": interval, "undefined_resamples": undefined, "resamples": resamples,
            "reason": reason if interval is None else None}


def build_report(experiment):
    # Input precision is bounded by the contracts; keep intermediates deterministic.
    with localcontext() as context:
        context.prec = 100
        return _build_report(experiment)


def _build_report(experiment):
    m, episodes, operations = (experiment[k] for k in ("manifest", "episodes", "operations"))
    tasks = {t["id"]: t for t in m["tasks"]}
    planned = {(s["task_id"], s["trial_id"], a) for s in m["schedule"] for a in ARMS}
    states, cells, diagnostics = {}, {}, []
    blockers = set()
    exposure = {arm: {d: 0 for d in DISPOSITIONS} for arm in ARMS}
    for e in episodes:
        outcome, reasons = episode_outcome(e, m, tasks, experiment["artifacts"])
        key = (e["task_id"], e["trial_id"], e["arm"])
        if key not in planned:
            outcome, reasons = "invalid", reasons + ["unscheduled_episode"]
        if key in cells:
            states[cells[key]["id"]] = "invalid"
            outcome, reasons = "invalid", reasons + ["duplicate_assigned_cell"]
            cells[key]["duplicate"] = True
        else:
            cells[key] = {"id": e["id"]}
        states[e["id"]] = outcome
        exposure[e["arm"]][e["treatment"]["disposition"]] += 1
        for reason in reasons:
            diagnostics.append({"episode_id": e["id"], "reason": reason})
            blockers.add(reason)
    missing = sorted(planned - set(cells))
    outcomes = Counter(states.values())
    unexecuted = len(missing) + outcomes["unexecuted"]
    if unexecuted:
        blockers.add("unexecuted_cells")
    if outcomes["invalid"]:
        blockers.add("invalid_episodes")
    if outcomes["unknown"]:
        blockers.add("unknown_outcomes")
    if any(exposure[a]["unknown"] for a in ARMS):
        blockers.add("unknown_exposure")
    episode_by_id = {e["id"]: e for e in episodes}
    op_by_id = {o["operation_id"]: o for o in operations}
    op_refs = defaultdict(set)
    for e in episodes:
        for oid in e["operation_ids"]:
            op_refs[oid].add(e["id"])
            if oid not in op_by_id or op_by_id[oid]["episode_id"] != e["id"]:
                blockers.add("missing_or_mismatched_operation_reference")
    cp = m["cost_policy"]
    categories = cp["categories"]
    included = [k for k in CATEGORIES if categories[k]["coverage"] == "included"]
    excluded = [k for k in CATEGORIES if categories[k]["coverage"] == "excluded"]
    if not included or any(c["coverage"] == "unknown" for c in categories.values()):
        blockers.add("unknown_cost_coverage")
    totals = {arm: {basis: Decimal(0) for basis in BASES} for arm in (*ARMS, "unallocated")}
    scoped_totals = {arm: {basis: Decimal(0) for basis in BASES} for arm in ARMS}
    category_totals = {arm: {basis: {c: Decimal(0) for c in CATEGORIES} for basis in BASES}
                       for arm in (*ARMS, "unallocated")}
    op_counts = {arm: Counter() for arm in (*ARMS, "unallocated")}
    missing_costs = {arm: Counter() for arm in (*ARMS, "unallocated")}
    op_categories = defaultdict(Counter)
    task_costs = {t: {a: {b: Decimal(0) for b in BASES} for a in ARMS} for t in tasks}
    secondary = {a: {"operations": 0, "unfinished_operations": 0, "provider_tokens": {k: 0 for k in METRICS},
                     "local_estimated_tokens": {k: 0 for k in METRICS}, "missing_usage_operations": 0,
                     "roles": {k: 0 for k in ("agent", "transform", "tool", "verification", "setup")},
                     "wall_seconds_known": "0", "wall_seconds_missing": 0} for a in ARMS}
    all_missing = []
    invalid_ledger = False
    for op in operations:
        eid = op["episode_id"]
        episode = episode_by_id.get(eid)
        arms = [(episode["arm"], Decimal(1))] if episode else [("unallocated", Decimal(1))]
        if eid is None and op["role"] == "setup" and categories[op["category"]]["allocation"] == "equal_arms":
            arms = [(a, Decimal("0.5")) for a in ARMS]
        elif episode is None or op_refs[op["operation_id"]] != {eid}:
            blockers.add("unallocated_or_unreferenced_operation")
            invalid_ledger = True
        if op["experiment_id"] != m["experiment_id"] or op["manifest_hash"] != m["content_hash"]:
            blockers.add("operation_identity_mismatch")
            invalid_ledger = True
        category, basis = op["category"], op["cost"]["basis"]
        amount, reason = operation_amount(op, m)
        if basis in ("usage_priced", "modeled") and any(r["evidence_hash"] not in m["bindings"]["artifacts"] for r in cp["rates"]):
            blockers.add("missing_price_evidence")
        if basis == "provider_billed" and op["cost"]["receipt_hash"] is not None:
            receipt = op["cost"]["receipt_hash"]
            if receipt not in m["bindings"]["artifacts"]:
                blockers.add("unbound_billing_evidence")
            else:
                experiment["artifact_reader"](receipt)
        if op["event"] != "terminal":
            blockers.add("unfinished_operations")
            reason = "unfinished_operation"
        if op["status"] in ("failed", "timeout", "cancelled", "unknown"):
            blockers.add("operational_failure")
        if categories[category]["coverage"] == "included" and (amount is None or basis != categories[category]["basis"] or
                reason is not None or (basis == "provider_billed" and not op["cost"]["reconciled"])):
            blockers.add("incomplete_cost_evidence")
        for arm, share in arms:
            op_counts[arm][basis] += 1
            if amount is not None:
                totals[arm][basis] += amount * share
                category_totals[arm][basis][category] += amount * share
                if arm in ARMS and category in included:
                    scoped_totals[arm][basis] += amount * share
            if reason is not None or amount is None:
                missing_costs[arm][basis] += 1
                all_missing.append({"operation_id": op["operation_id"], "arm": arm, "reason": reason or "missing_amount"})
            if arm not in ARMS:
                continue
            secondary[arm]["operations"] += 1
            secondary[arm]["roles"][op["role"]] += 1
            secondary[arm]["unfinished_operations"] += int(op["event"] != "terminal")
            counters = normalized_usage(op["usage"], m["pins"]["model"])
            if counters is not None:
                label = "provider_tokens" if op["usage"]["source"] == "provider_observed" else "local_estimated_tokens"
                for k in METRICS:
                    secondary[arm][label][k] += counters[k]
            elif op["role"] == "agent":
                secondary[arm]["missing_usage_operations"] += 1
            if episode:
                op_categories[eid][category] += 1
            if amount is not None and category in included:
                if episode and episode["task_id"] in tasks:
                    task_costs[episode["task_id"]][arm][basis] += amount * share
                elif eid is None:
                    for tid in tasks:
                        task_costs[tid][arm][basis] += amount * share / len(tasks)
    for e in episodes:
        if e["status"] != "unexecuted" and any(op_categories[e["id"]][k] == 0 and categories[k]["allocation"] == "per_episode" for k in included):
            blockers.add("missing_category_operations")
        if e["wall_seconds"] is None:
            secondary[e["arm"]]["wall_seconds_missing"] += 1
        else:
            secondary[e["arm"]]["wall_seconds_known"] = money(Decimal(secondary[e["arm"]]["wall_seconds_known"]) + Decimal(e["wall_seconds"]))
    for category in included:
        if categories[category]["allocation"] == "equal_arms" and not any(o["category"] == category and o["episode_id"] is None for o in operations):
            blockers.add("missing_category_operations")
    successes = {a: sum(states[e["id"]] == "success" for e in episodes if e["arm"] == a) for a in ARMS}
    table = {k: 0 for k in ("both_success", "baseline_only_success", "candidate_only_success", "both_failure")}
    pair_rows, regressions, gains = [], [], []
    for s in sorted(m["schedule"], key=lambda s: (s["task_id"], s["trial_id"])):
        keys = [(s["task_id"], s["trial_id"], a) for a in ARMS]
        if not all(k in cells and not cells[k].get("duplicate") and states[cells[k]["id"]] in ("success", "failure") for k in keys):
            continue
        b, c = (states[cells[k]["id"]] == "success" for k in keys)
        table["both_success" if b and c else "baseline_only_success" if b else "candidate_only_success" if c else "both_failure"] += 1
        row = {"task_id": s["task_id"], "trial_id": s["trial_id"], "baseline_success": b, "candidate_success": c,
               "episodes": {a: cells[k]["id"] for a, k in zip(ARMS, keys)}}
        pair_rows.append(row)
        if b and not c:
            regressions.append(row)
        if c and not b:
            gains.append(row)
    paired_count = len(pair_rows)
    bs = table["both_success"] + table["baseline_only_success"]
    cs = table["both_success"] + table["candidate_only_success"]
    paired = {"count": paired_count, "outcomes": table,
              "conditional_failure_rate": number(Decimal(table["baseline_only_success"]) / bs) if bs else None,
              "conditional_failure_rate_reason": None if bs else "zero_baseline_successes",
              "success_delta": number(Decimal(cs - bs) / paired_count) if paired_count else None,
              "baseline_success_rate": number(Decimal(bs) / paired_count) if paired_count else None,
              "candidate_success_rate": number(Decimal(cs) / paired_count) if paired_count else None}
    accounting, economics = {}, {}
    evidence_complete = not blockers
    for arm in (*ARMS, "unallocated"):
        accounting[arm] = {}
        for basis in BASES:
            scope = [k for k in included if categories[k]["basis"] == basis]
            accounting[arm][basis] = {"known_subtotal": money(totals[arm][basis]), "known_operations": op_counts[arm][basis],
                                      "included_subtotal": money(scoped_totals[arm][basis]) if arm in ARMS else None,
                                      "known_by_category": {k: money(v) for k, v in category_totals[arm][basis].items()},
                                      "missing_operations": missing_costs[arm][basis], "scope_categories": scope,
                                      "subtotal_is_lower_bound": bool(missing_costs[arm][basis]) or not evidence_complete}
    for basis in BASES[:-1]:
        row = {}
        same_basis = bool(included) and all(categories[k]["basis"] == basis for k in included)
        for arm in ARMS:
            why = None if evidence_complete and same_basis else "incomplete_or_incompatible_cost_scope"
            if successes[arm] == 0:
                why = "zero_verified_successes"
            row[arm] = {"cost_per_verified_success": number(scoped_totals[arm][basis] / successes[arm]) if why is None else None,
                        "reason": why}
        b, c = (row[a]["cost_per_verified_success"] for a in ARMS)
        row["relative_reduction"] = number(1 - (scoped_totals["candidate"][basis] / successes["candidate"]) /
                                          (scoped_totals["baseline"][basis] / successes["baseline"])) if b is not None and c is not None and scoped_totals["baseline"][basis] > 0 else None
        row["reason"] = None if row["relative_reduction"] is not None else "missing_or_zero_baseline_ratio"
        economics[basis] = row
    a = m["analysis"]
    uncertainty = {"method": "task_cluster_percentile", "descriptive_only": True,
                   "seed": a["seed"], "task_clusters": len(tasks), "confidence": a["confidence"],
                   "success_delta": None, "conditional_failure_rate": None, "relative_reduction": {}}
    task_rows = defaultdict(list)
    for row in pair_rows:
        task_rows[row["task_id"]].append(row)
    samples = {k: [] for k in ("success_delta", "conditional_failure_rate", *BASES[:-1])}
    undefined = Counter()
    rng = random.Random(a["seed"])
    task_ids = sorted(tasks)
    interval_reason = "insufficient_task_clusters" if len(tasks) < 2 else "incomplete_pairs" if paired_count != len(m["schedule"]) else None
    if interval_reason is None:
        for _ in range(a["resamples"]):
            selected = [task_ids[rng.randrange(len(task_ids))] for _ in task_ids]
            rows = [r for tid in selected for r in task_rows[tid]]
            b = sum(r["baseline_success"] for r in rows)
            c = sum(r["candidate_success"] for r in rows)
            loss = sum(r["baseline_success"] and not r["candidate_success"] for r in rows)
            samples["success_delta"].append(Decimal(c - b) / len(rows))
            if b:
                samples["conditional_failure_rate"].append(Decimal(loss) / b)
            else:
                undefined["conditional_failure_rate"] += 1
            for basis in BASES[:-1]:
                bt = sum((task_costs[t]["baseline"][basis] for t in selected), Decimal(0))
                ct = sum((task_costs[t]["candidate"][basis] for t in selected), Decimal(0))
                if b and c and bt > 0 and economics[basis]["relative_reduction"] is not None:
                    samples[basis].append(1 - (ct / c) / (bt / b))
                else:
                    undefined[basis] += 1
    for stat in ("success_delta", "conditional_failure_rate"):
        uncertainty[stat] = _interval(samples[stat], undefined[stat], a["resamples"], a["confidence"], interval_reason)
    for basis in BASES[:-1]:
        uncertainty["relative_reduction"][basis] = _interval(samples[basis], undefined[basis], a["resamples"], a["confidence"], interval_reason)
    evidence_class = m["tasks"][0]["provenance"]["class"]
    if a["mode"] != "claim":
        blockers.add("diagnostic_only")
    if evidence_class != "held_out":
        blockers.add("not_held_out")
    if len(tasks) < a["min_tasks"] or min(len([s for s in m["schedule"] if s["task_id"] == tid]) for tid in tasks) < a["min_trials"]:
        blockers.add("inadequate_sample")
    # Descriptive bootstrap is not an independently reviewed inference policy.
    if a["method"] != "task_hoeffding_bonferroni" or a["review_hash"] is None:
        blockers.add("missing_reviewed_inference_policy")
    quality_bounds = None
    reviewed = False
    if a["review_hash"] is not None:
        review = parse_json(experiment["artifact_reader"](a["review_hash"]))
        fields(review, "schema_version kind analysis_hash method independent approved reviewer evidence_hash", "method review")
        header(review, "method_review")
        hash_value(review["evidence_hash"], "review evidence")
        if review["evidence_hash"] in m["bindings"]["artifacts"]:
            experiment["artifact_reader"](review["evidence_hash"])
        string(review["reviewer"], "reviewer")
        require(type(review["independent"]) is bool and type(review["approved"]) is bool, "invalid review flags")
        reviewed = (review["independent"] and review["approved"] and review["method"] == a["method"] and
                    review["analysis_hash"] == canonical_digest({k: v for k, v in a.items() if k != "review_hash"}))
        if not reviewed:
            blockers.add("invalid_method_review")
    if reviewed and paired_count == len(m["schedule"]):
        n = len(tasks)
        alpha = 1 - Decimal(a["confidence"])
        epsilon = ((Decimal(4) / alpha).ln() / (2 * n)).sqrt()
        bmean = sum(Decimal(sum(r["baseline_success"] for r in task_rows[t])) / len(task_rows[t]) for t in task_ids) / n
        lossmean = sum(Decimal(sum(r["baseline_success"] and not r["candidate_success"] for r in task_rows[t])) / len(task_rows[t]) for t in task_ids) / n
        delta_lower = Decimal(cs - bs) / paired_count - 2 * epsilon
        cfr_upper = min(Decimal(1), (lossmean + epsilon) / (bmean - epsilon)) if bmean > epsilon else Decimal(1)
        quality_bounds = {"method": "task_hoeffding_bonferroni", "success_delta_lower": number(max(-1, delta_lower)),
                          "conditional_failure_rate_upper": number(cfr_upper)}
        if delta_lower < -Decimal(a["noninferiority_margin"]) or cfr_upper > Decimal(a["regression_ceiling"]):
            blockers.add("quality_uncertainty_crosses_boundary")
    else:
        blockers.add("quality_bounds_unavailable")
    economic_pass = any(s["interval"] is not None and Decimal(s["interval"][0]) > 0
                        for s in uncertainty["relative_reduction"].values())
    if not economic_pass:
        blockers.add("no_positive_cost_improvement_interval")
    candidate_exposed = any(e["arm"] == "candidate" and e["treatment"]["disposition"] == "applied" and
                            e["treatment"]["input_hash"] != e["treatment"]["output_hash"] for e in episodes)
    if exposure["candidate"]["unknown"]:
        blockers.add("unknown_exposure")
    if not candidate_exposed:
        blockers.add("no_applied_treatment")
    safety = any(e["safety_violation"] for e in episodes)
    if safety:
        blockers.add("safety_violation")
    if regressions:
        blockers.add("observed_regressions")
    quality_pass = quality_bounds is not None and not any(k in blockers for k in
        ("quality_uncertainty_crosses_boundary", "invalid_method_review", "missing_reviewed_inference_policy", "observed_regressions", "safety_violation"))
    verdict = ("invalid" if outcomes["invalid"] or invalid_ledger else "regressed" if regressions or safety else
               "no_effect" if evidence_complete and not candidate_exposed else
               "beneficial_for_declared_scope" if not blockers else "inconclusive")
    return {"schema_version": SCHEMA_VERSION, "kind": "contextdoctor.report", "experiment_id": m["experiment_id"],
            "manifest_hash": m["content_hash"], "input_hashes": {**experiment["input_hashes"], "artifacts": sorted(experiment["verified_artifacts"])}, "evidence_class": evidence_class,
            "counts": {"tasks": len(tasks), "planned": len(planned), "recorded": len(episodes),
                       "executed": sum(e["status"] != "unexecuted" for e in episodes), "verified": outcomes["success"] + outcomes["failure"],
                       "verified_successes": successes, "invalid": outcomes["invalid"], "unknown": outcomes["unknown"], "unexecuted": unexecuted},
            "paired": paired, "regressions": regressions, "gains": gains, "exposure": exposure, "secondary": secondary,
            "accounting": accounting, "economics": economics, "uncertainty": uncertainty, "quality_bounds": quality_bounds,
            "cost_scope": {"currency": cp["currency"], "categories": categories, "included": included, "excluded": excluded,
                           "label": "+".join(included) + "-only cost per verified success" if excluded else "declared complete task scope",
                           "rounding": "ROUND_HALF_EVEN; derived metrics to 12 decimal places; ledger subtotals unrounded"},
            "missing_costs": all_missing, "unexecuted_cells": [dict(zip(("task_id", "trial_id", "arm"), row)) for row in missing],
            "diagnostics": sorted(diagnostics, key=lambda d: (d["episode_id"], d["reason"])), "quality_gate": quality_pass,
            "economics_gate": economic_pass, "evidence_complete": evidence_complete, "claim_blockers": sorted(blockers), "verdict": verdict,
            "limitations": ["Recorded receipts attest verification; hashes do not establish independent authorship.",
                            "Synthetic/development evidence and descriptive intervals are not representative qualification.",
                            "Excluded costs are not zero. No currency conversion or live repricing."]}


def render_markdown(report):
    lines = ["# ContextDoctor experimental report", "", f"Verdict: **{report['verdict']}**", "",
             f"Evidence: `{report['evidence_class']}`; complete: `{str(report['evidence_complete']).lower()}`.",
             f"Planned: {report['counts']['planned']}; verified: {report['counts']['verified']}; unexecuted: {report['counts']['unexecuted']}.",
             f"Baseline-only successes lost: {len(report['regressions'])}; candidate-only gains: {len(report['gains'])}.", "",
             f"Cost scope: {report['cost_scope']['label']} ({report['cost_scope']['currency']}).", "",
             "| Basis | Baseline known subtotal | Candidate known subtotal | Baseline cost/success | Candidate cost/success | Relative reduction |",
             "| --- | --- | --- | --- | --- | --- |"]
    for basis, economics in report["economics"].items():
        values = [basis, report["accounting"]["baseline"][basis]["known_subtotal"], report["accounting"]["candidate"][basis]["known_subtotal"],
                  economics["baseline"]["cost_per_verified_success"], economics["candidate"]["cost_per_verified_success"], economics["relative_reduction"]]
        lines.append("| " + " | ".join("unavailable" if v is None else str(v) for v in values) + " |")
    lines += ["", "Known subtotals are lower bounds when evidence is incomplete, not complete spend.", "",
              "## Claim blockers", ""] + [f"- `{b}`" for b in report["claim_blockers"]]
    lines += ["", "## Complete machine-readable evidence", "", "The JSON below is the exact report, including regressions, uncertainty, coverage and missing evidence.",
              "", "```json", json.dumps(report, ensure_ascii=False, indent=2, allow_nan=False), "```", ""]
    return "\n".join(lines)


def write_json(path, value):
    with path.open("w", encoding="utf-8", newline="\n") as handle:
        handle.write(json.dumps(value, ensure_ascii=False, indent=2, allow_nan=False) + "\n")


def build_plan(m):
    return {"schema_version": SCHEMA_VERSION, "kind": "contextdoctor.plan", "manifest_hash": m["content_hash"],
            "schedule_hash": canonical_digest(m["schedule"]), "execution_hash": canonical_digest(m["execution"]),
            "cells": [{"task_id": s["task_id"], "trial_id": s["trial_id"], "arm": a}
                      for s in m["schedule"] for a in s["order"]],
            "limits": m["limits"], "execution_permitted": False,
            "requirements": ["Separate local scope approval with expiry, implementation and executable/image hashes.",
                             "Required isolation, or constrained data-only free-provider adapter; no shell/model-output execution.",
                             "No paid models, retries, redirects, provider/model fallback or automatic resume."]}


def import_legacy(m, path):
    from paired_records import validate_record, pair_records, aggregate
    raw = read_bytes(path)
    try:
        records = [json.loads(line, object_pairs_hook=_pairs, parse_float=str, parse_constant=_not_integer)
                   for line in raw.decode("utf-8").splitlines() if line.strip()]
    except (json.JSONDecodeError, UnicodeError) as exc:
        raise ContractError("invalid legacy JSONL") from exc
    require(bool(records), "no legacy records")
    groups, gaps, costs = defaultdict(list), set(), []
    tasks = {t["id"]: t for t in m["tasks"]}
    scheduled = {(s["task_id"], s["trial_id"]) for s in m["schedule"]}
    for i, record in enumerate(records):
        validate_record(record, f"legacy record {i}")
        identifier(record["run_id"], "legacy run ID")
        groups[(record["run_id"], record["model"])].append(record)
        arm = "baseline" if record["arm"] == "raw" else "candidate"
        trial = f"s{record['seed']}-a{record['attempt']}"
        if (record["run_id"] != m["experiment_id"] or record["model"] != m["pins"]["model"] or
                record.get("policy_revision") != m["arms"][arm]["config_hash"] or
                record["task_id"] not in tasks or (record["task_id"], trial) not in scheduled or
                record["environment_snapshot"] != tasks.get(record["task_id"], {}).get("snapshot_hash")):
            gaps.add("legacy_identity_unmapped")
        measurement = record.get("measurement") or {}
        for key in ("reported_cost", "cost"):
            value = measurement.get(key)
            if value is not None and not isinstance(value, (dict, list, bool)):
                try:
                    amount = Decimal(str(value))
                except InvalidOperation as exc:
                    raise ContractError("invalid legacy cost") from exc
                require(amount.is_finite() and -30 <= amount.as_tuple().exponent <= 30,
                        "unsupported legacy cost precision")
                costs.append({"record_index": i, "field": "measurement." + key, "amount": money(decimal(format(amount, "f"), key)),
                              "currency": None, "basis": "unclassified"})
    results = []
    for (run, model), rows in sorted(groups.items()):
        result = aggregate(*pair_records(rows))
        result.update(run_id=run, model=model)
        results.append(result)
    return {"schema_version": SCHEMA_VERSION, "kind": "contextdoctor.legacy_import", "manifest_hash": m["content_hash"],
            "source_hash": hash_bytes(raw), "records_retained": len(records), "groups": results,
            "unreconciled_cost_observations": costs, "cost_per_verified_success": None, "diagnostic_only": True,
            "claim_blockers": sorted(gaps | {"legacy_verification_and_ledger_missing"}),
            "limitations": ["Original bytes retained separately, including failed/invalid/unpaired records and all measurements.",
                            "Legacy labels are not independent verifier receipts or complete billing evidence.",
                            "Legacy trials map only to predeclared s<seed>-a<attempt> IDs; no trial/model/experiment pins are invented."]}


def readiness(experiment, report):
    m = experiment["manifest"]
    rollout = m["rollout"]
    pilot = False
    if rollout and rollout["pilot_hash"] is not None:
        p = parse_json(experiment["artifact_reader"](rollout["pilot_hash"]))
        fields(p, "schema_version kind report_hash approval_hash adapter completed evidence_class", "pilot")
        header(p, "pilot")
        hash_value(p["report_hash"], "pilot report")
        hash_value(p["approval_hash"], "pilot approval")
        require(type(p["completed"]) is bool, "invalid pilot completion")
        pr = parse_json(experiment["artifact_reader"](p["report_hash"]))
        approval = parse_json(experiment["artifact_reader"](p["approval_hash"]))
        pilot = (p["completed"] and p["evidence_class"] == "development" and p["adapter"] in ("container", "openrouter_free") and
                 pr.get("kind") == "contextdoctor.report" and pr.get("evidence_complete") is True and
                 pr.get("evidence_class") == "development" and approval.get("kind") == "contextdoctor.approval" and
                 approval.get("manifest_hash") == pr.get("manifest_hash"))
    expansion = bool(rollout and pilot and rollout["maintenance_minutes"] is not None and rollout["benefit_minutes"] is not None and
                     Decimal(rollout["benefit_minutes"]) > Decimal(rollout["maintenance_minutes"]) and rollout["repeat_uses"] >= 2 and
                     rollout["decision"] == "expand")
    return {"schema_version": SCHEMA_VERSION, "kind": "contextdoctor.readiness", "manifest_hash": m["content_hash"],
            "phases": {"P0": "contracts_valid", "P1": "offline_report_available",
                       "P2": "configured_not_runtime_verified" if m["execution"] else "pending_execution_configuration",
                       "P3": "supplied_pilot_evidence_complete" if pilot else "pending_pilot_evidence",
                       "P4": "declared_scope_gate_passed" if report["verdict"] == "beneficial_for_declared_scope" and pilot else "blocked",
                       "P5": "expansion_supported_by_supplied_evidence" if expansion else "keep_small_pending_use_evidence"},
            "claim_blockers": report["claim_blockers"], "qualified": False, "expansion_allowed": expansion,
            "decision": rollout["decision"] if rollout else "keep_small",
            "limitations": ["Readiness checks supplied evidence, not runtime availability, independent authorship or Select/release qualification."]}


def build_parser():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("validate", "report", "plan", "readiness", "import-legacy", "run"):
        p = sub.add_parser(name)
        p.add_argument("--manifest", required=True, type=Path)
        if name in ("validate", "report", "readiness", "import-legacy"):
            p.add_argument("--records", required=True, type=Path)
        if name in ("report", "plan", "import-legacy", "run"):
            p.add_argument("--out", required=True, type=Path)
        if name == "report":
            p.add_argument("--require-benefit", action="store_true")
        if name == "run":
            p.add_argument("--approval", required=True, type=Path)
    return parser


def main(argv=None):
    args = build_parser().parse_args(argv)
    try:
        if args.command in ("plan", "run", "import-legacy"):
            m, _ = load_manifest(args.manifest)
            if args.command == "run":
                from contextdoctor_run import run
                return run(args.manifest, args.approval, args.out)
            result = build_plan(m) if args.command == "plan" else import_legacy(m, args.records)
            args.out.mkdir(parents=True, exist_ok=False)
            write_json(args.out / ("plan.json" if args.command == "plan" else "import.json"), result)
            if args.command == "import-legacy":
                (args.out / "legacy.jsonl").write_bytes(read_bytes(args.records))
            print(f"{args.command}: {args.out}")
            return 0
        experiment = load_experiment(args.manifest, args.records)
        report = build_report(experiment)
        if args.command == "validate":
            print(f"valid contracts; verdict={report['verdict']}; evidence_complete={report['evidence_complete']}")
            return 0
        if args.command == "readiness":
            print(json.dumps(readiness(experiment, report), ensure_ascii=False, indent=2, allow_nan=False))
            return 0
        args.out.mkdir(parents=True, exist_ok=False)
        write_json(args.out / "report.json", report)
        with (args.out / "report.md").open("w", encoding="utf-8", newline="\n") as handle:
            handle.write(render_markdown(report))
        print(f"{report['verdict']}: {args.out / 'report.json'}")
        return 3 if args.require_benefit and report["verdict"] != "beneficial_for_declared_scope" else 0
    except (ContractError, KeyError, TypeError, RecursionError) as exc:
        print(f"invalid contract: {exc}", file=sys.stderr)
        return 2
    except OSError as exc:
        print(f"I/O failure: {type(exc).__name__}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
