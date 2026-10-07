"""Experimental JSON presentation of native logfold; not model-quality qualification."""

import json
import re
import subprocess
from collections import Counter
from pathlib import Path

from generate_observation import MAX_BYTES, native_guard

GUIDELINE = "tokenfold-logfold-json-presentation-v2"


def render(frame: str) -> tuple[str, str]:
    """Return JSON presentation and independently reconstructed original text."""
    if len(frame.encode()) > MAX_BYTES:
        raise ValueError("frame byte limit")
    lines = frame.split("\n")
    if len(lines) < 3 or lines[0] != "__tf_logfold1__" or len(lines) > 10002:
        raise ValueError("bounded native logfold frame required")
    templates = json.loads(lines[1])
    if (not isinstance(templates, list) or not templates or len(templates) > 512
            or any(not isinstance(t, str) for t in templates)):
        raise ValueError("invalid logfold templates")
    parts = [t.split("\0") for t in templates]
    rows, recovered = [], []
    size = 0
    for row in lines[2:]:
        fields = row.split(" ")
        if not re.fullmatch(r"[0-9]+", fields[0]):
            raise ValueError("invalid logfold template index")
        index, values = int(fields[0]), fields[1:]
        if (index >= len(parts) or len(values) != len(parts[index]) - 1
                or any(not re.fullmatch(r"(?:0x[0-9a-fA-F]+|[0-9]+)", v) for v in values)):
            raise ValueError("invalid logfold captures")
        text = parts[index][0] + "".join(v + part for v, part in zip(values, parts[index][1:]))
        size += len(text.encode())
        if size > MAX_BYTES:
            raise ValueError("expanded text byte limit")
        recovered.append(text)
        # Captures stay strings: leading zeroes, hex and identifier bytes are not rounded.
        rows.append([index, *values])
    # Singleton lines do not benefit from template indirection; keep their bytes literal.
    counts = Counter(row[0] for row in rows)
    repeated = [index for index in range(len(parts)) if counts[index] > 1]
    indices = {index: ordinal for ordinal, index in enumerate(repeated)}
    rows = [[indices[row[0]], *row[1:]] if row[0] in indices else text
            for row, text in zip(rows, recovered)]
    presentation = json.dumps({"format": GUIDELINE,
        "meaning": "Each row is one source line in original order. String rows are literal lines. "
                   "For array rows, first item selects template_parts; "
                   "remaining string items fill gaps between its parts, left to right. Concatenate literally.",
        "template_parts": [parts[index] for index in repeated], "rows": rows},
        ensure_ascii=False, separators=(",", ":"))
    if len(presentation.encode()) > MAX_BYTES:
        raise ValueError("presentation byte limit")
    return presentation, "".join(recovered)


class ReadableLogfoldArm:
    def __init__(self, binary: Path, compress, count_tokens):
        self.binary, self.compress, self.count_tokens = binary, compress, count_tokens
        self.last_receipt = None

    def __call__(self, task, target, seed):
        self.last_receipt = {"kind": GUIDELINE, "disposition": "fell_back", "reason": None,
                             "valid_attempt": False, "model_readability": "unqualified"}
        source = task["source"]
        native_guard(self.binary, source, task["query"])
        frame = self.compress(source, target)
        if not isinstance(frame, str):
            raise ValueError("native compression unavailable")
        if not frame.startswith("__tf_logfold1__\n"):
            self.last_receipt.update(reason="not_logfold", valid_attempt=True)
            return source
        payload, recovered = render(frame)
        native = subprocess.run([str(self.binary), "decode", "-"], input=frame.encode(),
                                capture_output=True, timeout=30)
        if native.returncode or native.stdout != source.encode() or recovered != source:
            raise ValueError("JSON presentation differs from native exact recovery")
        native_guard(self.binary, payload, task["query"])
        self.last_receipt.update(valid_attempt=True, native_frame_tokens=self.count_tokens(frame),
                                 presentation_tokens=self.count_tokens(payload), byte_exact_recovery=True)
        if self.count_tokens(payload) >= self.count_tokens(source):
            self.last_receipt["reason"] = "not_smaller"
            return source
        if self.count_tokens(payload) > target:
            self.last_receipt["reason"] = "over_budget"
            return source
        self.last_receipt.update(disposition="rendered", reason=None)
        return payload
