"""Research caller-scored Select; no inference and no production quality admission."""

import json
import re
import subprocess
import tempfile
from pathlib import Path

import run_baselines as rb
from generate_observation import MAX_BYTES, native_guard
from select_observation import source_context

GUIDELINE = "tokenfold-caller-bm25-native-select-v1"
HEADING_GUIDELINE = "tokenfold-caller-bm25-heading-native-select-v1"


def heading_scores(texts, query):
    """Research paragraph framing only: boost source titles literally named in the query."""
    scores = rb.sel_bm25(texts, query)
    bonus = 2 * max([1.0, *scores])
    normalized_query = " " + " ".join(rb._tokens(query)) + " "
    for i, text in enumerate(texts):
        match = re.fullmatch(r"\[document \d+\] (.+)", text.split("\n", 1)[0])
        if match:
            title = " ".join(rb._tokens(match[1]))
            if title and " " + title + " " in normalized_query:
                scores[i] += bonus
    return scores


def validate_context(task):
    context = source_context(task)
    groups = context["groups"]
    if (any(set(g) - {"id", "text", "required"} or not isinstance(g.get("id"), str)
            or not g["id"] or len(g["id"]) > 256 or type(g.get("required", False)) is not bool for g in groups)
            or len({g["id"] for g in groups}) != len(groups) or len(task["query"].encode()) > 4096):
        raise ValueError("invalid caller-scored grouping or query")
    return context


class Bm25SelectArm:
    def __init__(self, binary: Path, heading_aware: bool = False):
        self.binary = binary
        self.heading_aware = heading_aware
        self.last_receipt = None

    def __call__(self, task, target, seed):
        self.last_receipt = {"kind": HEADING_GUIDELINE if self.heading_aware else GUIDELINE, "valid_attempt": False,
                             "quality": "unqualified", "disposition": "fell_back", "reason": None}
        context = validate_context(task)
        groups = context["groups"]
        native_guard(self.binary, task["source"], task["query"])
        native_guard(self.binary, json.dumps(context, ensure_ascii=False), task["query"])
        texts = [g["text"] for g in groups]
        scores = heading_scores(texts, task["query"]) if self.heading_aware else rb.sel_bm25(texts, task["query"])
        document = {**context, "groups": [{**g, "fallback_score": score}
                                           for g, score in zip(groups, scores)]}
        encoded = json.dumps(document, ensure_ascii=False).encode()
        if len(encoded) > MAX_BYTES:
            raise ValueError("caller-scored context byte limit")
        with tempfile.TemporaryDirectory(prefix="tokenfold-bm25-") as directory:
            path = Path(directory) / "receipt.json"
            result = subprocess.run([str(self.binary), "--experimental", "select", "--query", task["query"],
                "--target-tokens", str(target), "--receipt-file", str(path)],
                input=encoded, capture_output=True, timeout=30)
            if result.returncode:
                raise ValueError("native caller-scored selection refused")
            receipt = json.loads(path.read_bytes())
        payload = result.stdout.decode()
        prefix, suffix = context.get("prefix", ""), context.get("suffix", "")
        if not payload.startswith(prefix) or not payload.endswith(suffix):
            raise ValueError("native caller protection mismatch")
        body = payload[len(prefix):len(payload) - len(suffix) if suffix else len(payload)]
        ids, cursor = [], 0
        # Conservative witness: ambiguous prefix-overlapping groups may be rejected, never rewritten.
        for group in groups:
            if body.startswith(group["text"], cursor):
                ids.append(group["id"])
                cursor += len(group["text"])
        if (receipt["used_scorer"] or cursor != len(body) or receipt["kept_group_count"] != len(ids)
                or any(g.get("required", False) and g["id"] not in ids for g in groups)):
            raise ValueError("native caller-scored receipt mismatch")
        native_guard(self.binary, payload, task["query"])
        self.last_receipt.update(native_receipt=receipt,
                                 witness_group_ids=ids,
                                 ranking="bm25-with-literal-query-title-boost" if self.heading_aware else "existing-harness-bm25-k1-1.5-b-0.75",
                                 model_calls=0)
        if not receipt["selected"]:
            if payload != task["source"]:
                raise ValueError("native fallback differs from source")
            self.last_receipt["valid_attempt"] = True
            self.last_receipt["reason"] = "native_baseline"
            return payload
        if receipt["fallback_reason"] != "scorer unavailable":
            raise ValueError("unexpected native caller-score path")
        self.last_receipt["valid_attempt"] = True
        if rb.count_tokens(payload) > target:
            self.last_receipt["reason"] = "independent_recount_over_budget"
            return task["source"]
        if rb.count_tokens(payload) >= rb.count_tokens(task["source"]):
            self.last_receipt["reason"] = "not_smaller"
            return task["source"]
        self.last_receipt.update(disposition="selected", reason=None)
        return payload
