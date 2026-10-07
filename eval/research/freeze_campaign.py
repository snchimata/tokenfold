#!/usr/bin/env python3
"""Freeze a predeclared campaign and paired-task files offline; never runs models."""

import argparse
import hashlib
import json
import math
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import run_paired as rp

WORKLOADS = {"json", "logs", "code", "retrieval", "multi-turn"}
SPLITS = {"train", "validation", "test"}
TRANSPORTS = {"ollama-loopback-killable-subprocess", "native-openai-loopback-killable-subprocess",
              "openrouter-https-no-redirect-no-fallback-zero-price"}
ANSWER_ROLES = ["system", "user"]


def digest(data):
    return hashlib.sha256(data).hexdigest()


def freeze(protocol_path: Path, root: Path) -> dict:
    root = root.resolve()
    protocol_bytes = protocol_path.read_bytes()
    protocol = json.loads(protocol_bytes)
    if protocol.get("version") != 1 or protocol.get("scope") not in {"smoke", "qualification"}:
        raise ValueError("version 1 and explicit smoke/qualification scope required")
    for field in ("arms", "models", "seeds", "budgets", "quality", "suites"):
        if not protocol.get(field):
            raise ValueError(f"nonempty {field} required")
    arms = protocol["arms"]
    if (not isinstance(arms, list) or any(not isinstance(a, str) or not a for a in arms)
            or len(set(arms)) != len(arms) or "raw" not in arms or len(arms) < 2):
        raise ValueError("unique named arms including raw and a comparator required")
    seeds = protocol["seeds"]
    if not isinstance(seeds, list) or any(type(s) is not int for s in seeds) or len(set(seeds)) != len(seeds):
        raise ValueError("unique integer seeds required")
    models = protocol["models"]
    if not isinstance(models, dict) or any(not isinstance(v, str) or not v for v in models.values()):
        raise ValueError("explicit model revision descriptors required")
    budgets = protocol["budgets"]
    for field in ("context_tokens", "output_tokens", "max_calls", "timeout_seconds"):
        value = budgets.get(field)
        if type(value) not in (int, float) or not math.isfinite(value) or value <= 0:
            raise ValueError(f"positive finite {field} required")
        if field != "timeout_seconds" and type(value) is not int:
            raise ValueError(f"integer {field} required")
    if budgets["context_tokens"] <= budgets["output_tokens"]:
        raise ValueError("context must reserve space beyond output")
    ratio = budgets.get("ratio")
    if type(ratio) not in (int, float) or not math.isfinite(ratio) or not 0 < ratio <= 1:
        raise ValueError("predeclared ratio must be in (0, 1]")
    for field in ("max_cfr", "max_success_loss", "confidence"):
        value = protocol["quality"].get(field)
        if type(value) not in (int, float) or not math.isfinite(value) or not 0 < value < 1:
            raise ValueError(f"predeclared {field} must be in (0, 1)")
    minimum = protocol["quality"].get("min_raw_success_clusters")
    if type(minimum) is not int or minimum <= 0:
        raise ValueError("positive min_raw_success_clusters required")
    claims = protocol["quality"].get("cfr_claim_count")
    if "cfr_claim_count" in protocol["quality"] or protocol["scope"] == "qualification":
        if type(claims) is not int or claims < len(arms) - 1:
            raise ValueError("cfr_claim_count must cover every comparator and any declared subgroup claims")
    if protocol["scope"] == "qualification":
        advantage = protocol.get("advantage")
        if not isinstance(advantage, dict) or set(advantage) != {"latency", "economics", "features"}:
            raise ValueError("qualification requires explicit latency/economics/features advantage thresholds")
        for name, metrics in (("latency", {"warm_end_to_end_p50_ms", "warm_end_to_end_p95_ms", "warm_end_to_end_p99_ms"}),
                              ("economics", {"total_campaign_cost_usd"})):
            rule = advantage[name]
            if (not isinstance(rule, dict) or rule.get("metric") not in metrics
                    or type(rule.get("min_relative_reduction")) not in (int, float)
                    or not math.isfinite(rule["min_relative_reduction"])
                    or not 0 < rule["min_relative_reduction"] < 1):
                raise ValueError("positive predeclared " + name + " advantage threshold required")
        if not isinstance(advantage["economics"].get("cost_basis"), str) or not advantage["economics"]["cost_basis"].strip():
            raise ValueError("explicit economics cost_basis assumptions required; unknown costs cannot prove advantage")
        behaviors = advantage["features"]
        if (not isinstance(behaviors, list) or not behaviors or any(not isinstance(item, str) or not item.strip() for item in behaviors)
                or len(set(behaviors)) != len(behaviors)):
            raise ValueError("unique predeclared feature behaviors required")

    files, ids, sources, clusters = [], {}, {}, {}
    source_clusters = {}
    coverage = set()
    test_clusters = set()
    runtime = protocol.get("runtime", {})
    if not isinstance(runtime, dict) or type(runtime.get("allow_inferred_answers", False)) is not bool:
        raise ValueError("allow_inferred_answers must be a boolean runtime declaration")
    if "native_template_preflight" in runtime and type(runtime["native_template_preflight"]) is not bool:
        raise ValueError("native_template_preflight must be a boolean runtime declaration")
    if "arm_order" in runtime:
        order = runtime["arm_order"]
        if (not isinstance(order, list) or any(not isinstance(name, str) for name in order)
                or len(order) != len(arms) or set(order) != set(arms)):
            raise ValueError("arm_order must be an exact live-arm permutation")
    transport = runtime.get("transport")
    if "transport" in runtime or protocol["scope"] == "qualification":
        if transport not in TRANSPORTS:
            raise ValueError("runtime transport must pin the predeclared answering transport")
    roles = runtime.get("answering_roles")
    if "answering_roles" in runtime or protocol["scope"] == "qualification":
        if roles != ANSWER_ROLES:
            raise ValueError("answering roles must stay system/user with no assistant prefill")
    prefill = runtime.get("assistant_prefill")
    if "assistant_prefill" in runtime or protocol["scope"] == "qualification":
        if prefill is not False:
            raise ValueError("assistant prefill must stay disabled")
    for suite in protocol["suites"]:
        split, workload = suite["split"], suite["workload"]
        if split not in SPLITS or workload not in WORKLOADS | {"mixed"}:
            raise ValueError("unsupported split or workload")
        directory = (root / suite["path"]).resolve()
        directory.relative_to(root)  # Refuse outside-workspace fixtures, including symlinks.
        for path in directory.glob("*.json"):
            path.resolve().relative_to(root)
        tasks = rp.load_tasks(directory, require_literal_answer=not runtime.get("allow_inferred_answers", False))
        # Reuse the existing loader, then hash the exact bytes it consumed. Refuse
        # a moving dataset rather than freezing a mixture of different reads.
        by_id = {task["id"]: task for task in tasks}
        for path in sorted(directory.glob("*.json")):
            path.resolve().relative_to(root)
            data = path.read_bytes()
            task = json.loads(data)
            if "development_split" in task and task["development_split"] != split:
                raise ValueError("task development_split differs from campaign suite split")
            if protocol["scope"] == "qualification":
                origin = task.get("origin")
                if not isinstance(origin, str) or not origin.strip():
                    raise ValueError("qualification task origin must be a nonempty string")
                if (origin.lower().startswith(("synthetic-development", "synthetic-protocol-control"))
                        or any(marker in origin.lower() for marker in ("not-qualification", "not-agent-qualification"))):
                    raise ValueError("explicitly unqualified task origin cannot enter qualification scope")
            task_workload = task.get("workload") if workload == "mixed" else workload
            if not isinstance(task_workload, str) or task_workload not in WORKLOADS:
                raise ValueError("mixed suite tasks require an explicit supported workload")
            if "workload" in task and task["workload"] != task_workload:
                raise ValueError("task workload differs from campaign suite workload")
            if by_id.get(task["id"]) != task:
                raise ValueError("task changed while freezing")
            task_id = task["id"]
            source_hash = digest(task["source"].encode("utf-8"))
            cluster = task.get("cluster_id")
            if protocol["scope"] == "qualification" and (not isinstance(cluster, str) or not cluster):
                raise ValueError("qualification requires explicit task cluster_id")
            cluster = cluster or task_id
            if task_id in ids:
                raise ValueError("duplicate task id across campaign suites")
            if source_hash in sources and sources[source_hash] != split:
                raise ValueError("source overlaps train/validation/test splits")
            if (protocol["scope"] == "qualification" and source_hash in source_clusters
                    and source_clusters[source_hash] != cluster):
                raise ValueError("identical source cannot count as different independent clusters")
            if cluster in clusters and clusters[cluster] != split:
                raise ValueError("task cluster overlaps train/validation/test splits")
            ids[task_id], sources[source_hash], clusters[cluster] = split, split, split
            source_clusters[source_hash] = cluster
            if split == "test":
                coverage.add(task_workload)
                test_clusters.add(cluster)
            files.append({"path": path.relative_to(root).as_posix(), "sha256": digest(data),
                          "task_id": task_id, "cluster_id": cluster, "split": split,
                          "workload": task_workload})
    if protocol["scope"] == "qualification":
        if coverage != WORKLOADS or set(ids.values()) != SPLITS:
            raise ValueError("qualification requires all five test workloads and three splits")
        # Necessary zero-event sample floor, not a correlated-task confidence bound
        # or proof that these tasks will actually be raw successes.
        ceiling = protocol["quality"]["max_cfr"]
        confidence = protocol["quality"]["confidence"]
        # Bonferroni allocation covers the declared family without assuming
        # independence between comparator claims. Clusters still must be independent.
        log_alpha = math.log1p(-confidence) - math.log(claims)
        floor = math.ceil(log_alpha / math.log1p(-ceiling))
        if minimum < floor or len(test_clusters) < minimum:
            raise ValueError("insufficient predeclared/test clusters for zero-event CFR ceiling")
    if protocol_path.read_bytes() != protocol_bytes:
        raise ValueError("protocol changed while freezing")
    for entry in files:
        path = root / entry["path"]
        path.resolve().relative_to(root)
        if digest(path.read_bytes()) != entry["sha256"]:
            raise ValueError("task changed while freezing")
    inventory = set()
    for suite in protocol["suites"]:
        directory = (root / suite["path"]).resolve()
        directory.relative_to(root)
        for path in directory.glob("*.json"):
            path.resolve().relative_to(root)
            inventory.add(path.relative_to(root).as_posix())
    if inventory != {entry["path"] for entry in files}:
        raise ValueError("task inventory changed while freezing")
    result = {"version": 1, "kind": "campaign-freeze-not-quality-evidence",
            "protocol_sha256": digest(protocol_bytes), "protocol": protocol,
            "files": files, "test_workloads": sorted(coverage),
            "test_cluster_count": len(test_clusters),
            "limitations": ["Hashes do not prove tasks are fresh, representative or independent.",
                            "This artifact does not implement arms, run models or qualify defaults.",
                            "Model descriptors are declarations, not verified weight identity."]}
    if protocol["scope"] == "qualification":
        result["cfr_sample_rule"] = {
            "method": "bonferroni-one-sided-zero-event-binomial",
            "family_confidence": confidence, "claim_count": claims,
            "per_claim_alpha": (1 - confidence) / claims,
            "min_raw_success_clusters_per_claim": floor,
            "limitation": "Total test clusters do not prove sufficient raw-success clusters "
                          "for each claim or subgroup, non-inferiority power, or independence."}
    return result


def verify_run(protocol_path: Path, frozen_path: Path, root: Path, tasks_dir: Path,
               arms: list[str], model_revision: str, settings: dict, *, suite_split="test",
               summarizer_revision=None) -> dict:
    """Authorize an exact declared suite; validation/training require explicit opt-in."""
    if suite_split not in SPLITS:
        raise ValueError("unsupported live suite split")
    frozen_bytes = frozen_path.read_bytes()
    frozen = json.loads(frozen_bytes)
    if frozen != freeze(protocol_path, root):
        raise ValueError("campaign protocol or dataset differs from frozen artifact")
    protocol = frozen["protocol"]
    if protocol["scope"] != "smoke":
        raise ValueError("observation/replay pilot cannot execute qualification scope")
    if set(protocol["arms"]) != set(arms):
        raise ValueError("live arms differ from predeclared arms")
    models = {"answering_and_compressor": model_revision}
    if summarizer_revision is not None:
        models["summarizer"] = summarizer_revision
    if protocol["models"] != models:
        raise ValueError("live model differs from predeclared revision")
    if protocol.get("runtime") != settings.get("runtime"):
        raise ValueError("live comparator/guideline settings differ from predeclared runtime")
    if settings["seed"] not in protocol["seeds"]:
        raise ValueError("seed was not predeclared")
    for field in ("context_tokens", "output_tokens", "max_calls", "ratio"):
        if settings[field] != protocol["budgets"][field]:
            raise ValueError("live budgets differ from predeclared budgets")
    if settings["timeout"] != protocol["budgets"]["timeout_seconds"]:
        raise ValueError("live deadline differs from predeclared deadline")
    directory = tasks_dir.resolve().relative_to(root.resolve()).as_posix()
    if not any(suite["split"] == suite_split and suite["path"] == directory
               for suite in protocol["suites"]):
        raise ValueError("live directory is not a predeclared " + suite_split + " suite")
    result = {"sha256": digest(frozen_bytes), "protocol_sha256": frozen["protocol_sha256"],
              "scope": protocol["scope"]}
    result.update({"test_directory": directory} if suite_split == "test" else
                  {"suite_directory": directory, "suite_split": suite_split})
    result["confidence"] = protocol["quality"]["confidence"]
    if "cfr_claim_count" in protocol["quality"]:
        result["cfr_claim_count"] = protocol["quality"]["cfr_claim_count"]
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--protocol", required=True, type=Path)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args(argv)
    frozen = freeze(args.protocol, args.root)
    with args.output.open("x", encoding="utf-8") as output:
        output.write(json.dumps(frozen, indent=2) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
