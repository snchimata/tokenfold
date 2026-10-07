"""Offline USGS GeoJSON API snapshots: structured lookup development, not qualification."""
import argparse
import hashlib
import json
import math
from pathlib import Path
from urllib.parse import urlsplit, parse_qs


def digest(data):
    return hashlib.sha256(data).hexdigest()


def event_ids(feature):
    aliases = feature["properties"].get("ids", "")
    if not isinstance(aliases, str):
        raise ValueError("event aliases must be a comma-separated string")
    return {feature["id"], *(value.strip() for value in aliases.split(",") if value.strip())}


def convert(document, cluster, task_mode="lookup"):
    if task_mode not in {"lookup", "latest-shallow-moderate"}:
        raise ValueError("unsupported USGS task mode")
    features = document.get("features")
    if (document.get("type") != "FeatureCollection" or not isinstance(features, list)
            or not features or len(features) > 512 or not cluster):
        raise ValueError("bounded nonempty FeatureCollection and source cluster required")
    ids = []; seen = set()
    for feature in features:
        identity = feature.get("id")
        if (feature.get("type") != "Feature" or not isinstance(identity, str) or not identity
                or identity in ids or not isinstance(feature.get("properties"), dict)
                or feature["properties"].get("net") != "us"):
            raise ValueError("unique USGS-network features required")
        ids.append(identity)
        aliases = event_ids(feature)
        if seen & aliases:
            raise ValueError("event alias overlap within snapshot")
        seen.update(aliases)
    # Preserve all response metadata and original field order. Only whitespace changes.
    encode = lambda value: json.dumps(value, ensure_ascii=False, allow_nan=False, separators=(",", ":"))
    fields = list(document)
    index = fields.index("features")
    prefix = "{" + "".join(encode(k)+":"+encode(document[k])+"," for k in fields[:index]) + '"features":['
    suffix = "]" + "".join(","+encode(k)+":"+encode(document[k]) for k in fields[index+1:]) + "}"
    groups = [{"id": identity, "text": ("," if i else "")+encode(feature)}
              for i, (identity, feature) in enumerate(zip(ids, features))]
    source = prefix + "".join(g["text"] for g in groups) + suffix
    assert json.loads(source) == document
    target = features[len(features)//2]  # Source-only position, not outcome/answer selection.
    place = target["properties"].get("place")
    if task_mode == "lookup" and (not isinstance(place, str) or not place.strip()):
        raise ValueError("selected source event must have a place string")
    task = {"id": "usgs-"+digest(cluster.encode())[:16], "family":"json", "workload":"json",
            "tier":"A", "source":source,
            "query":f"For event ID {target['id']}, return the exact properties.place string only.",
            "gold_answer":place, "critical_atoms":[], "cluster_id":cluster,
            "select_context":{"prefix":prefix,"groups":groups,"suffix":suffix},
            "origin":"public-usgs-api-lookup-development-not-qualification"}
    if task_mode == "latest-shallow-moderate":
        eligible = []
        for feature in features:
            properties = feature["properties"]
            geometry = feature.get("geometry")
            coordinates = geometry.get("coordinates") if isinstance(geometry, dict) else None
            depth = coordinates[2] if isinstance(coordinates, list) and len(coordinates) >= 3 else None
            magnitude, timestamp = properties.get("mag"), properties.get("time")
            # Null/missing values cannot establish eligibility. Malformed values
            # are refused, not silently interpreted as measurements.
            for value in (magnitude, timestamp, depth):
                if value is not None and (type(value) not in (int, float) or not math.isfinite(value)):
                    raise ValueError("finite numeric selection measurements required")
            if (geometry is not None and (not isinstance(geometry, dict) or geometry.get("type") != "Point")):
                raise ValueError("Point geometry required for selection")
            if all(value is not None for value in (magnitude, timestamp, depth)) and magnitude >= 4.5 and depth <= 70:
                eligible.append((timestamp, feature["id"]))
        eligible.sort(key=lambda item: (-item[0], item[1]))
        task.update(id=task["id"]+"-selection",
            query="From this response only, select the latest earthquake with properties.mag >= 4.5 "
                  "and geometry.coordinates[2] (depth in km) <= 70. Exclude events with missing or null "
                  "magnitude, time or depth. Latest means largest properties.time; break ties by "
                  "lexicographically smallest event ID. Return only its event ID, or NONE if no event qualifies.",
            gold_answer=eligible[0][1] if eligible else "NONE",
            origin="public-usgs-api-multistep-selection-development-not-qualification")
    return task


def main(argv=None):
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--snapshot",type=Path,required=True)
    parser.add_argument("--manifest",type=Path,required=True)
    parser.add_argument("--output-dir",type=Path,required=True)
    parser.add_argument("--task-mode",choices=("lookup","latest-shallow-moderate"),default="lookup")
    args=parser.parse_args(argv)
    root=args.snapshot.resolve();raw=args.manifest.read_bytes();manifest=json.loads(raw)
    tasks=[];owners={};clusters={};paths=set()
    for entry in manifest["files"]:
        url=urlsplit(entry["url"]);query=parse_qs(url.query)
        if (url.scheme!="https" or url.netloc!="earthquake.usgs.gov"
                or url.path!="/fdsnws/event/1/query" or query.get("format")!=["geojson"]
                or query.get("contributor")!=["us"] or entry["split"] not in {"train","validation","test"}):
            raise ValueError("explicit official USGS JSON query and split required")
        path=(root/entry["path"]).resolve();path.relative_to(root)
        if path in paths:raise ValueError("duplicate snapshot path")
        paths.add(path)
        with path.open("rb") as file:data=file.read(1024*1024+1)
        if len(data)>1024*1024 or digest(data)!=entry["sha256"]:
            raise ValueError("snapshot size or digest mismatch")
        document=json.loads(data);cluster=entry["cluster_id"];split=entry["split"]
        if cluster in clusters:raise ValueError("one complete snapshot per conservative cluster required")
        clusters[cluster]=split
        task=convert(document,cluster,args.task_mode)
        for feature in document["features"]:
            for identity in event_ids(feature):
                if identity in owners:raise ValueError("event or alias overlap across snapshots")
                owners[identity]=cluster
        task["development_split"]=split;tasks.append(task)
    if not tasks:raise ValueError("nonempty snapshot manifest required")
    args.output_dir.mkdir(parents=True,exist_ok=False)
    files=[]
    for split in ("train","validation","test"):
        directory=args.output_dir/split;directory.mkdir()
        for task in [t for t in tasks if t["development_split"]==split]:
            path=directory/(task["id"]+".json");data=(json.dumps(task,ensure_ascii=False,indent=2)+"\n").encode()
            path.write_bytes(data);files.append({"path":path.relative_to(args.output_dir).as_posix(),"sha256":digest(data)})
    result={"kind":"public-usgs-json-development-not-qualification","source":manifest,
            "input_manifest_sha256":digest(raw),"importer_sha256":digest(Path(__file__).read_bytes()),
            "records":len(tasks),"clusters":len(clusters),"files":files,
            "limitations":["Bounded historical API lookup, not full catalog or live tool/agent evaluation.",
                           "Source clusters and disjoint event IDs do not prove statistical independence.",
                           "Public pretraining contamination and underlying contributor rights require review before hosted export.",
                           "No model calls, secret guard or quality qualification performed by this importer."]}
    if args.task_mode != "lookup":
        result["task_mode"] = args.task_mode
    (args.output_dir/"manifest.json").write_text(json.dumps(result,indent=2)+"\n",encoding="utf-8")
    return 0


if __name__=="__main__":raise SystemExit(main())
