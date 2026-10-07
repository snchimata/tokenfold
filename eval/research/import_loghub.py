"""Offline Loghub adaptation: real log windows and JSON projections, not qualification."""

import argparse
import csv
import hashlib
import io
import json
from pathlib import Path

REPOSITORY = "https://github.com/logpai/loghub"
MAX_BYTES = 4 * 1024 * 1024


def digest(data):
    return hashlib.sha256(data).hexdigest()


def convert(family, raw, structured, window=64):
    if not family or window < 1 or window > 512:
        raise ValueError("family and window in [1, 512] required")
    lines = raw.decode("utf-8").splitlines(keepends=True)
    rows = list(csv.DictReader(io.StringIO(structured.decode("utf-8"))))
    if not rows or len(rows) != len(lines):
        raise ValueError("raw and structured log rows must align")
    for index, (row, line) in enumerate(zip(rows, lines), 1):
        if (row.get("LineId") != str(index) or not row.get("Content", "").strip()
                or row["Content"] not in line or None in row):
            raise ValueError("structured content must match its numbered raw line")
    tasks = []
    for start in range(0, len(rows), window):
        batch = rows[start:start + window]
        # Source-order middle record, never selected by gold, model outcome or severity.
        target = batch[len(batch) // 2]
        for workload in ("logs", "json"):
            if workload == "logs":
                groups = [{"id": digest(f"{family}:{start+i}".encode())[:16],
                           "text": f"[LineId {row['LineId']}] " + lines[start+i]}
                          for i, row in enumerate(batch)]
                context = {"groups": groups}
            else:
                # Exclude ground-truth parsing labels; this is a projection, not an API trace.
                groups = [{"id": digest(f"{family}:{start+i}".encode())[:16],
                           "text": ("," if i else "") + json.dumps(
                               {k: v for k, v in row.items() if k not in {"EventId", "EventTemplate"}},
                               ensure_ascii=False, separators=(",", ":"))}
                          for i, row in enumerate(batch)]
                context = {"prefix": "[", "groups": groups, "suffix": "]"}
            source = context.get("prefix", "") + "".join(g["text"] for g in groups) + context.get("suffix", "")
            tasks.append({"id": f"loghub-{family}-{start:05d}-{workload}", "family": workload,
                          "workload": workload, "tier": "A", "source": source,
                          "query": f"For LineId {target['LineId']}, return the exact Content message only, without the log header.",
                          "gold_answer": target["Content"], "critical_atoms": [],
                          "select_context": context, "cluster_id": "loghub-source-family-" + digest(family.encode()),
                          "origin": "public-loghub-window-development-not-agent-qualification"})
    return tasks


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--snapshot", required=True, type=Path)
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument("--window", type=int, default=64)
    args = parser.parse_args(argv)
    root = args.snapshot.resolve()
    manifest_bytes = args.manifest.read_bytes()
    manifest = json.loads(manifest_bytes)
    revision = manifest.get("revision", "")
    if (manifest.get("repository") != REPOSITORY or len(revision) != 40
            or any(c not in "0123456789abcdef" for c in revision)):
        raise ValueError("pinned official Loghub snapshot required")
    inputs = {}
    for entry in manifest["files"]:
        path = (root / entry["path"]).resolve()
        path.relative_to(root)
        with path.open("rb") as source:
            data = source.read(MAX_BYTES + 1)
        if (len(data) > MAX_BYTES or digest(data) != entry["sha256"]
                or entry["url"] != f"https://raw.githubusercontent.com/logpai/loghub/{revision}/{entry['path']}"
                or entry["path"] in inputs):
            raise ValueError("snapshot bytes, URL or identity mismatch")
        inputs[entry["path"]] = data
    if "LICENSE" not in inputs:
        raise ValueError("upstream license notice required")
    families = sorted({Path(p).parts[0] for p in inputs if p.endswith("_2k.log")}, key=lambda f: digest(f.encode()))
    if not families:
        raise ValueError("no log families")
    train_end = max(1, min(len(families) - 2, len(families) * 6 // 10))
    validation_end = max(train_end + 1, min(len(families) - 1, len(families) * 8 // 10))
    tasks = []
    for index, family in enumerate(families):
        split = "train" if index < train_end else "validation" if index < validation_end else "test"
        for task in convert(family, inputs[f"{family}/{family}_2k.log"],
                            inputs[f"{family}/{family}_2k.log_structured.csv"], args.window):
            task["development_split"] = split
            tasks.append(task)
    args.output_dir.mkdir(parents=True, exist_ok=False)
    (args.output_dir / "LICENSE.loghub").write_bytes(inputs["LICENSE"])
    files = []
    for split in ("train", "validation", "test"):
        directory = args.output_dir / split
        directory.mkdir()
        for task in (t for t in tasks if t["development_split"] == split):
            data = (json.dumps(task, ensure_ascii=False, indent=2) + "\n").encode()
            path = directory / (task["id"] + ".json")
            path.write_bytes(data)
            files.append({"path": path.relative_to(args.output_dir).as_posix(), "sha256": digest(data)})
    result = {"kind": "public-loghub-development-not-qualification", "source": manifest,
              "snapshot_manifest_sha256": digest(manifest_bytes), "importer_sha256": digest(Path(__file__).read_bytes()),
              "attribution": "Zhu et al., Loghub, ISSRE 2023; https://github.com/logpai/loghub",
              "license": "Loghub research/academic notice; see LICENSE.loghub", "window": args.window,
              "tasks": len(tasks), "source_family_clusters": len(families), "files": files,
              "limitations": ["All windows and both projections of a source family share one cluster and split.",
                              "Families are not proven independent; public data can overlap model training.",
                              "JSON is a log projection, not representative tool/API traffic or another independent sample.",
                              "Indexed message lookup is not incident diagnosis or a live agent trajectory.",
                              "Upstream logs are not necessarily sanitized. Native secret guards are mandatory before hosted export."]}
    (args.output_dir / "manifest.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
