"""Offline pinned full-RAG adaptation; no inference or short-answer qualification."""

import argparse
import hashlib
import json
import re
from collections import Counter
from pathlib import Path

from replay_context import replay_source, validate_replay
from select_observation import source_context

REVISION = "2c618bb98db3c8526433e22d8a2f7320f10a7470"
DATA = "mtrag-human/generation_tasks/RAG.jsonl"
DATA_SHA256 = "5d5201da9fabd072fd8f6b8d051bfaafa7ef031e76722a4920c66e94cede1873"
REPOSITORY = "https://github.com/IBM/mt-rag-benchmark"
MAX_BYTES = 64 * 1024 * 1024


def digest(data):
    return hashlib.sha256(data).hexdigest()


def document_root(collection, identity):
    """Conservative overlap key for the pinned passage-ID formats, not a retrieval ID."""
    patterns = {"clapnq": r"([0-9]+)_[0-9]+(?:-[0-9]+)+",
                "fiqa": r"([0-9]+)(?:-[0-9]+)+",
                "govt": r"([0-9a-f]{16})(?:-[0-9]+)+",
                "ibmcloud": r"(ibmcld_[0-9]+)(?:-[0-9]+)+"}
    for family, pattern in patterns.items():
        if collection.startswith("mt-rag-" + family + "-"):
            match = re.fullmatch(pattern, identity)
            if match:
                return collection + ":" + match[1]
            break
    raise ValueError("unsupported MTRAG collection/passage ID")


def convert(records):
    if not isinstance(records, list) or not records or len(records) > 10000:
        raise ValueError("bounded nonempty MTRAG record array required")
    tasks, parents, owners = [], {}, {}

    def find(identity):
        while parents[identity] != identity:
            parents[identity] = parents[parents[identity]]
            identity = parents[identity]
        return identity

    for record in records:
        if not isinstance(record, dict):
            raise ValueError("MTRAG records must be objects")
        identity, conversation, collection = (record.get(k) for k in
                                               ("task_id", "conversation_id", "Collection"))
        if (any(not isinstance(v, str) or not v.strip() for v in (identity, conversation, collection))
                or identity in parents):
            raise ValueError("unique task IDs and nonempty conversation/collection required")
        messages, contexts, targets = (record.get(k) for k in ("input", "contexts", "targets"))
        if (not isinstance(messages, list) or not 1 <= len(messages) <= 65
                or not isinstance(contexts, list) or len(contexts) != 5
                or not isinstance(targets, list) or len(targets) != 1
                or not isinstance(targets[0], dict)
                or not isinstance(targets[0].get("text"), str) or not targets[0]["text"].strip()):
            raise ValueError("bounded messages, five full-RAG passages and one evaluation target required")
        clean_messages = []
        for message in messages:
            if (not isinstance(message, dict) or message.get("speaker") not in {"user", "agent"}
                    or not isinstance(message.get("text"), str) or not message["text"].strip()):
                raise ValueError("text user/agent messages required")
            clean_messages.append({"role": "assistant" if message["speaker"] == "agent" else "user",
                                   "content": message["text"]})
        if clean_messages[-1]["role"] != "user" or len(clean_messages[-1]["content"].encode()) > 4096:
            raise ValueError("bounded final user question required")
        history = clean_messages[:-1]
        parents[identity] = identity
        keys = ["conversation:" + conversation]
        groups = [{"id": digest(f"{identity}:history:{i}".encode())[:16],
                   "text": m["role"].upper() + ":\n" + m["content"] + "\n"}
                  for i, m in enumerate(history)]
        keys.extend("history:" + digest((m["speaker"] + "\n" + m["text"]).encode())
                    for m in messages[:-1])
        if history:
            groups.append({"id": digest(f"{identity}:marker".encode())[:16],
                           "text": "OBSERVATION:\n", "required": True})
        passages = []
        for index, context in enumerate(contexts):
            if (not isinstance(context, dict) or not isinstance(context.get("document_id"), str)
                    or not isinstance(context.get("text"), str) or not context["text"].strip()
                    or not isinstance(context.get("title", ""), str)):
                raise ValueError("literal passage ID/text/title required")
            keys += ["document:" + document_root(collection, context["document_id"]),
                     "passage:" + digest(context["text"].encode())]
            # Only source fields: never reference/relevance/score/author annotations.
            passages.append({"id": digest(f"{identity}:passage:{index}".encode())[:16],
                             "text": f"[document {index}] {context.get('title', '')}\n"
                                     + context["text"] + "\n"})
        groups.extend(passages)
        observation = "".join(g["text"] for g in passages)
        source = replay_source(history, observation) if history else observation
        if len(source.encode()) > 1024 * 1024:
            raise ValueError("MTRAG framed source byte limit")
        task = {"id": "mtrag-" + identity, "upstream_id": identity,
                "family": "multi-turn" if history else "retrieval",
                "workload": "multi-turn" if history else "retrieval", "tier": "A",
                "source": source, "query": clean_messages[-1]["content"],
                "gold_answer": targets[0]["text"], "critical_atoms": [],
                "select_context": {"groups": groups},
                "origin": "public-mtrag-full-rag-development-not-agent-qualification",
                "evaluation_requirement": "grounded-long-form-and-abstention-not-short-value-exact",
                "upstream_collection": collection, "upstream_conversation_id": conversation}
        if history:
            task.update(history_messages=history, observation=observation)
        source_context(task)
        validate_replay(task)
        tasks.append(task)
        for key in keys:
            if key in owners:
                left, right = sorted((find(identity), find(owners[key])))
                parents[right] = left
            else:
                owners[key] = identity
    members = {}
    for identity in parents:
        members.setdefault(find(identity), []).append(identity)
    clusters = {root: digest(json.dumps(sorted(ids)).encode()) for root, ids in members.items()}
    for task in tasks:
        cluster = clusters[find(task["upstream_id"])]
        task["cluster_id"] = "mtrag-source-component-" + cluster
        bucket = int(cluster[:8], 16) % 10
        task["development_split"] = "train" if bucket < 6 else "validation" if bucket < 8 else "test"
    return sorted(tasks, key=lambda task: digest(task["id"].encode()))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--snapshot", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--limit-per-split", type=int, default=20)
    args = parser.parse_args(argv)
    if args.limit_per_split < 1:
        parser.error("positive per-split cap required")
    root = args.snapshot.resolve()
    raw_manifest = (root / "manifest.json").read_bytes()
    manifest = json.loads(raw_manifest)
    if manifest.get("upstream") != REPOSITORY or manifest.get("revision") != REVISION:
        raise ValueError("pinned official MTRAG snapshot required")
    inputs = {}
    for rel in (DATA, "LICENSE", "README.md", "mtrag-human/README.md"):
        path = (root / rel).resolve()
        path.relative_to(root)
        with path.open("rb") as stream:
            data = stream.read(MAX_BYTES + 1)
        entry = manifest["files"][rel]
        if (len(data) > MAX_BYTES or digest(data) != entry["sha256"]
                or entry["url"] != f"https://raw.githubusercontent.com/IBM/mt-rag-benchmark/{REVISION}/{rel}"):
            raise ValueError("MTRAG snapshot bytes/URL mismatch")
        inputs[rel] = data
    if digest(inputs[DATA]) != DATA_SHA256:
        raise ValueError("only pinned full-RAG data accepted, never oracle settings")
    tasks = convert([json.loads(line) for line in inputs[DATA].decode("utf-8").splitlines()])
    args.output_dir.mkdir(parents=True, exist_ok=False)
    (args.output_dir / "LICENSE.mtrag").write_bytes(inputs["LICENSE"])
    files = []
    for split in ("train", "validation", "test"):
        directory = args.output_dir / split
        directory.mkdir()
        for index, task in enumerate([t for t in tasks if t["development_split"] == split][:args.limit_per_split]):
            data = (json.dumps(task, indent=2, ensure_ascii=False) + "\n").encode()
            path = directory / f"{index:04d}.json"
            path.write_bytes(data)
            files.append({"path": path.relative_to(args.output_dir).as_posix(), "sha256": digest(data)})
    output = {"kind": "public-mtrag-full-rag-development-not-qualification", "revision": REVISION,
              "input_sha256": DATA_SHA256, "snapshot_manifest_sha256": digest(raw_manifest),
              "importer_sha256": digest(Path(__file__).read_bytes()),
              "replay_sha256": digest(Path(__file__).with_name("replay_context.py").read_bytes()),
              "license": "Pinned repository Apache-2.0 notice; underlying corpus rights/attribution require review before export",
              "attribution": "Katsis et al., MTRAG, TACL 2025", "source": REPOSITORY,
              "records": len(tasks), "components": len({t["cluster_id"] for t in tasks}),
              "record_split_counts": dict(Counter(t["development_split"] for t in tasks)),
              "component_split_counts": dict(Counter(split for _, split in {
                  (t["cluster_id"], t["development_split"]) for t in tasks})),
              "selection": "SHA256 record-ID order, capped only after full source-only component split",
              "limit_per_split": args.limit_per_split, "files": files,
              "limitations": ["Recorded full-RAG contexts/history, not live retrieval or agent execution.",
                              "Conservative parent-ID keys prevent known overlap, not semantic independence.",
                              "No target/annotation filtering or gold-derived protection; public pretraining contamination unknown.",
                              "No native export guard or grounded long-form evaluator is run by this offline importer."]}
    (args.output_dir / "manifest.json").write_text(json.dumps(output, indent=2) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
