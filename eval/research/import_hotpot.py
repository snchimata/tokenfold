"""Offline HotpotQA snapshot importer; public development data, not agent qualification."""

import argparse
import hashlib
import json
import re
import string
from collections import Counter
from pathlib import Path


def digest(data):
    return hashlib.sha256(data).hexdigest()


def answer_scores(prediction, gold):
    """HotpotQA v1 answer-only normalization/EM/F1; no supporting-fact or joint score."""
    def normalize(text):
        text = text.lower().translate(str.maketrans("", "", string.punctuation))
        return " ".join(re.sub(r"\b(a|an|the)\b", " ", text).split())
    prediction, gold = normalize(prediction), normalize(gold)
    common = sum((Counter(prediction.split()) & Counter(gold.split())).values())
    if prediction != gold and ({prediction, gold} & {"yes", "no", "noanswer"}):
        common = 0
    precision = common / len(prediction.split()) if common else 0
    recall = common / len(gold.split()) if common else 0
    return {"em": float(prediction == gold), "f1": 2 * precision * recall / (precision + recall) if common else 0,
            "precision": precision, "recall": recall}


def convert(records):
    if not isinstance(records, list) or not records:
        raise ValueError("nonempty HotpotQA record array required")
    tasks, parent, owner = [], {}, {}

    def find(key):
        while parent[key] != key:
            parent[key] = parent[parent[key]]
            key = parent[key]
        return key

    for record in records:
        if not isinstance(record, dict):
            raise ValueError("HotpotQA records must be objects")
        identity = record.get("_id", record.get("id"))
        if not isinstance(identity, str) or not identity or identity in parent:
            raise ValueError("unique nonempty record IDs required")
        if any(not isinstance(record.get(k), str) or not record[k].strip() for k in ("question", "answer")):
            raise ValueError("question and answer must be nonempty text")
        context = record.get("context")
        if isinstance(context, dict):
            titles, sentences = context.get("title"), context.get("sentences")
            if not isinstance(titles, list) or not isinstance(sentences, list) or len(titles) != len(sentences):
                raise ValueError("context columns must align")
            context = list(zip(titles, sentences))
        if not isinstance(context, list) or not context:
            raise ValueError("nonempty context required")
        parent[identity] = identity
        groups = []
        for index, entry in enumerate(context):
            if (not isinstance(entry, (list, tuple)) or len(entry) != 2
                    or not isinstance(entry[0], str) or not entry[0].strip()
                    or not isinstance(entry[1], list) or not entry[1]
                    or any(not isinstance(s, str) for s in entry[1])):
                raise ValueError("invalid context paragraph")
            title, sentences = entry
            # Conservative leakage prevention: ALL context titles, not gold supporting facts.
            key = title.strip().casefold()
            if key in owner:
                left, right = sorted((find(identity), find(owner[key])))
                parent[right] = left
            else:
                owner[key] = identity
            groups.append({"id": digest(f"{identity}:{index}".encode())[:16],
                           "text": f"[document {index}] {title}\n" + "".join(sentences) + "\n"})
        tasks.append({"id": "hotpot-" + identity, "family": "retrieval", "workload": "retrieval", "tier": "A",
                      "source": "".join(g["text"] for g in groups), "query": record["question"],
                      "gold_answer": record["answer"], "critical_atoms": [],
                      "select_context": {"groups": groups},
                      "origin": "public-hotpot-validation-development-not-official-test",
                      "upstream_id": identity})
    members = {}
    for identity in parent:
        members.setdefault(find(identity), []).append(identity)
    clusters = {root: digest(json.dumps(sorted(ids)).encode()) for root, ids in members.items()}
    for task in tasks:
        cluster = clusters[find(task["upstream_id"])]
        task["cluster_id"] = "hotpot-context-component-" + cluster
        bucket = int(cluster[:8], 16) % 10
        task["development_split"] = "train" if bucket < 6 else "validation" if bucket < 8 else "test"
    return sorted(tasks, key=lambda t: digest(t["id"].encode()))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--limit-per-split", type=int, default=20)
    parser.add_argument("--one-per-component", action="store_true",
                        help="keep the first source-ordered task per component before applying the cap")
    parser.add_argument("--exclude-components", type=Path,
                        help="bounded JSON list of previously exposed component IDs; requires one-per-component")
    args = parser.parse_args(argv)
    if args.limit_per_split < 1:
        parser.error("limit-per-split must be positive")
    raw = args.input.read_bytes()
    if len(raw) > 64 * 1024 * 1024:
        parser.error("snapshot exceeds 64 MiB")
    tasks = convert(json.loads(raw))
    excluded = set()
    exclusion_sha = None
    if args.exclude_components is not None:
        if not args.one_per_component:
            parser.error("exclude-components requires one-per-component")
        with args.exclude_components.open("rb") as source:
            data = source.read(65537)
        if len(data) > 65536:
            parser.error("component exclusions exceed 64 KiB")
        try:
            values = json.loads(data)
        except (ValueError, UnicodeDecodeError):
            parser.error("invalid component exclusion JSON")
        known = {t["cluster_id"] for t in tasks}
        if (not isinstance(values, list) or any(not isinstance(v, str) or v not in known for v in values)
                or len(set(values)) != len(values)):
            parser.error("exclusions must be unique known component IDs")
        excluded, exclusion_sha = set(values), digest(data)
    args.output_dir.mkdir(parents=True, exist_ok=False)
    files = []
    for split in ("train", "validation", "test"):
        directory = args.output_dir / split
        directory.mkdir()
        selected, seen = [], set()
        for task in tasks:
            component = task["cluster_id"]
            if (task["development_split"] != split or component in excluded
                    or (args.one_per_component and component in seen)):
                continue
            selected.append(task)
            seen.add(component)
            if len(selected) == args.limit_per_split:
                break
        for index, task in enumerate(selected):
            data = (json.dumps(task, indent=2, ensure_ascii=False) + "\n").encode()
            path = directory / f"{index:04d}.json"
            path.write_bytes(data)
            files.append({"path": path.relative_to(args.output_dir).as_posix(), "sha256": digest(data)})
    manifest = {"kind": "public-validation-development-not-qualification", "input_sha256": digest(raw),
                "importer_sha256": digest(Path(__file__).read_bytes()),
                "license": "CC-BY-SA-4.0", "attribution": "Yang et al., HotpotQA, EMNLP 2018",
                "source": "https://hotpotqa.github.io/", "adaptation": "Paragraph framing and source-title connected-component partitioning",
                "records": len(tasks), "components": len({t["cluster_id"] for t in tasks}),
                "record_split_counts": dict(Counter(t["development_split"] for t in tasks)),
                "component_split_counts": dict(Counter(split for _, split in {
                    (t["cluster_id"], t["development_split"]) for t in tasks})),
                "selection": "SHA256 of record ID order, per-split cap; no answer or model-outcome filtering",
                "limit_per_split": args.limit_per_split, "files": files,
                "limitations": ["Public validation may occur in model training; it is not the official held-out test.",
                                "Connected components prevent known title overlap, not semantic independence.",
                                "Exact-string answer scoring is not the official HotpotQA normalized EM/F1 metric.",
                                "This snapshot is retrieval QA, not live retrieval or an agent trajectory."]}
    if args.one_per_component:
        manifest.update(selection="SHA256 record-ID order; first eligible task per source-title component, then per-split cap; no answer/outcome filtering",
                        one_per_component=True, excluded_components=sorted(excluded),
                        exclusion_sha256=exclusion_sha)
    (args.output_dir / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
