"""Pinned, cache-only Headroom universal/SmartCrusher arms with bounded workers."""

import hashlib
import json
import logging
import os
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from benchmark_provenance import checkout_provenance

MAX_BYTES = 1024 * 1024


def restore_authorized_rows(source, payload, recovered, row_path, marker):
    """Caller-bound research restoration, not a cache lookup or authorization oracle.

    The trusted host supplies the authorized source/path/marker. No foreign wrapper
    changes are admitted; returning original bytes is full recovery, not compression.
    """
    from native_model import unique_object
    import copy
    if (any(not isinstance(text, str) or len(text.encode()) > MAX_BYTES for text in (source, payload, recovered))
            or not isinstance(marker, str) or not marker.startswith("<<ccr:") or not marker.endswith(">>")
            or len(marker) > 256 or "<<ccr:" in source or payload.count(marker) != 1
            or payload.count("<<ccr:") != 1 or "<<ccr:" in recovered
            or not isinstance(row_path, list) or len(row_path) > 16):
        raise ValueError("bounded caller-bound recovery inputs required")
    original = json.loads(source, object_pairs_hook=unique_object)
    compressed = json.loads(payload, object_pairs_hook=unique_object)
    rows = json.loads(recovered, object_pairs_hook=unique_object)
    if not isinstance(rows, list) or len(rows) > 512:
        raise ValueError("bounded recovered row array required")
    def at(document, path):
        for key in path:
            if isinstance(key, str) and key and len(key) <= 256 and isinstance(document, dict) and key in document:
                document = document[key]
            elif type(key) is int and 0 <= key < 512 and isinstance(document, list) and key < len(document):
                document = document[key]
            else:
                raise ValueError("recovery path does not bind to source/payload")
        return document
    def canonical(value):
        return json.dumps(value, sort_keys=True, ensure_ascii=False, allow_nan=False, separators=(",", ":"))
    if canonical(rows) != canonical(at(original, row_path)) or marker not in canonical(at(compressed, row_path)):
        raise ValueError("recovered rows or marker do not match authorized scope")
    rebuilt = copy.deepcopy(compressed)
    if row_path:
        at(rebuilt, row_path[:-1])[row_path[-1]] = rows
    else:
        rebuilt = rows
    if canonical(rebuilt) != canonical(original):
        raise ValueError("recovery changed fields outside authorized scope")
    return source


def audit_recovery(source, payload, keys, retrieve, mirrored, reopened):
    """Audit authorized JSON-row recovery; never supplies recovered text to an arm."""
    from collections import Counter
    if "<<ccr:" in source or len(source.encode()) > MAX_BYTES:
        raise ValueError("recovery audit requires bounded marker-free source")
    original = json.loads(source)
    kept, end = json.JSONDecoder().raw_decode(payload)
    suffix = payload[end:]
    if suffix.strip():
        import re
        if not re.fullmatch(r'\s*<headroom:tool_digest sha256="[0-9a-f]{16}">\s*', suffix):
            raise ValueError("unsupported recovery audit wrapper")
    if not isinstance(original, list) or not isinstance(kept, list) or not keys or len(keys) > 64:
        raise ValueError("recovery audit requires JSON arrays and bounded generated markers")
    canonical = lambda row: json.dumps(row, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
    expected = Counter(canonical(row) for row in original)
    remaining = expected.copy()
    for row in kept:
        if isinstance(row, dict) and row.get("_ccr_dropped", "").startswith("<<ccr:"):
            continue
        remaining[canonical(row)] -= 1
    records = []
    restored = all(count >= 0 for count in remaining.values())
    for key in dict.fromkeys(keys):
        start = time.perf_counter()
        text = retrieve(key)
        elapsed = (time.perf_counter() - start) * 1000
        if not isinstance(text, str) or len(text.encode()) > MAX_BYTES:
            raise ValueError("bounded native recovery required")
        rows = json.loads(text)
        if not isinstance(rows, list):
            raise ValueError("row recovery must be a JSON array")
        # Pinned SmartCrusher stores the full original array, not just offloaded rows.
        restored = restored and Counter(canonical(row) for row in rows) == expected
        start = time.perf_counter()
        mirror_matches = mirrored(key) == text
        mirror_ms = (time.perf_counter() - start) * 1000
        start = time.perf_counter()
        reopen_matches = reopened(key) == text
        reopen_ms = (time.perf_counter() - start) * 1000
        records.append({"bytes": len(text.encode()), "native_retrieval_ms": elapsed,
                        "python_mirror_matches": mirror_matches, "python_mirror_ms": mirror_ms,
                        "reopened_store_matches": reopen_matches, "reopened_store_ms": reopen_ms})
    return {"kind": "source-bound-row-recovery-audit-not-agent-qualification",
            "exact_row_multiset_restored": restored,
            "retrievals": records,
            "limitation": "JSON row equality only, not byte reconstruction, model-directed retrieval, "
                          "cross-request authorization, cache qualification or durable process restart."}


class HeadroomArm:
    def __init__(self, root: Path, revision: str, python: Path, scratch: Path, timeout: float,
                 native_core: Path | None = None, *, capture_path: Path | None = None,
                 guard_binary: Path | None = None):
        if (capture_path is None) != (guard_binary is None):
            raise ValueError("recovery capture requires explicit native guard binary")
        self.capture_path = capture_path.resolve() if capture_path else None
        self.guard_binary = guard_binary.resolve() if guard_binary else None
        self.root, self.python, self.scratch = root.resolve(), python.resolve(), scratch.resolve()
        self.timeout, self.last_receipt = timeout, None
        self.native_core = native_core.resolve() if native_core else None
        module = "headroom/transforms/smart_crusher.py" if self.native_core else "headroom/compression/universal.py"
        self.provenance = checkout_provenance(self.root, revision, self.root / module)
        self.provenance["python_sha256"] = hashlib.sha256(self.python.read_bytes()).hexdigest()
        if self.native_core:
            self.provenance["native_core_sha256"] = hashlib.sha256(self.native_core.read_bytes()).hexdigest()
        self.scratch.mkdir(parents=True, exist_ok=False)

    def __call__(self, task: dict, target: int, seed: int) -> str:
        self.last_receipt = None
        if self.capture_path is not None:
            if self.capture_path.exists():
                raise ValueError("recovery capture refuses overwrite")
            from generate_observation import native_guard
            native_guard(self.guard_binary, task["source"], task["query"])
        if len(task["source"].encode()) > MAX_BYTES:
            raise ValueError("headroom input byte limit")
        # Explicit cache-only runtime: no inherited API keys/proxies or model downloads.
        environment = {key: os.environ[key] for key in ("SystemRoot", "WINDIR", "USERPROFILE", "HOME", "TEMP", "TMP")
                       if key in os.environ}
        environment.update(HF_HUB_OFFLINE="1", TRANSFORMERS_OFFLINE="1",
                           HEADROOM_WORKSPACE_DIR=str(self.scratch),
                           HEADROOM_CONFIG_DIR=str(self.scratch / "config"))
        request = {"root": str(self.root), "source": task["source"]}
        if self.native_core:
            request.update(query=task["query"], native_core=str(self.native_core),
                           native_core_sha256=self.provenance["native_core_sha256"])
        result = subprocess.run([str(self.python), "-I", str(Path(__file__).resolve()), "--worker"],
            input=json.dumps(request),
            encoding="utf-8", capture_output=True, timeout=self.timeout, env=environment)
        if result.returncode or len(result.stdout.encode()) > MAX_BYTES:
            raise ValueError("headroom worker unavailable or degraded")
        response = json.loads(result.stdout)
        self.last_receipt = response["receipt"]
        if self.capture_path is not None:
            capture = {"kind": "headroom-worker-payload-diagnostic-not-valid-arm",
                       "source_sha256": hashlib.sha256(task["source"].encode()).hexdigest(),
                       "query_sha256": hashlib.sha256(task["query"].encode()).hexdigest(),
                       "payload": response["payload"], "receipt": self.last_receipt,
                       "provenance": self.provenance}
            encoded = json.dumps(capture, ensure_ascii=False)
            if len(encoded.encode()) > MAX_BYTES:
                raise ValueError("recovery capture byte limit")
            native_guard(self.guard_binary, encoded, task["query"])
            with self.capture_path.open("x", encoding="utf-8") as stream:
                stream.write(encoded)
        if self.last_receipt.get("healthy") is not True:
            raise ValueError("headroom compressor degraded")
        return response["payload"]

    def unchanged(self):
        if self.native_core and hashlib.sha256(self.native_core.read_bytes()).hexdigest() != self.provenance["native_core_sha256"]:
            return False
        return (hashlib.sha256(self.python.read_bytes()).hexdigest() == self.provenance["python_sha256"]
                and checkout_provenance(self.root, self.provenance["revision"],
                    self.root / self.provenance["imported_module"]) ==
                {k: v for k, v in self.provenance.items() if k not in {"python_sha256", "native_core_sha256"}})


def worker(request):
    import contextlib
    import io
    import logging
    from unittest.mock import patch

    sys.path.insert(0, request["root"])
    # Imported dependencies sometimes print diagnostics; never mix them with the wire reply.
    with contextlib.redirect_stdout(io.StringIO()):
        if request.get("audit_restart") is True:
            return audit_store_restart(request)
        if request.get("native_core"):
            return smart_worker(request)
        from headroom.compression import universal
        from headroom.transforms.kompress_compressor import KompressCompressor
        if Path(universal.__file__).resolve() != Path(request["root"]) / "headroom/compression/universal.py":
            raise ValueError("wrong comparator import")
        model = KompressCompressor()
        backend = model.preload(allow_download=False)
        canary = getattr(model, "_canary_thread", None)
        if canary is not None:
            canary.join(timeout=10)
            if canary.is_alive():
                raise ValueError("comparator canary incomplete")
        if getattr(model, "_degraded_reason", None):
            raise ValueError("comparator canary degraded")

        class Health(logging.Handler):
            degraded = False

            def emit(self, record):
                # Conservatively refuse warnings instead of turning partial inference into a win.
                if record.levelno >= logging.WARNING:
                    self.degraded = True

        health = Health()
        logger = logging.getLogger("headroom.transforms.kompress_compressor")
        logger.addHandler(health)
        try:
            with patch.object(universal.UniversalCompressor, "_simple_compress",
                              side_effect=ValueError("comparator fallback refused")):
                result = universal.compress(request["source"])
        finally:
            logger.removeHandler(health)
        return {"payload": result.compressed,
                "receipt": {"kind": "headroom-universal-default-cache-only", "healthy": not health.degraded,
                            "backend": backend, "content_type": str(result.content_type),
                            "handler": result.handler_used,
                            "limitation": "Cold isolated API worker; not proxy, retrieval or matched-budget qualification."}}


def audit_store_restart(request):
    """Source-bound diagnostic in a separate reader process; never exports stored text."""
    from headroom.ccr.tool_injection import CCRToolInjector
    from headroom.cache.compression_store import get_compression_store
    source, payload = request["source"], request["payload"]
    if "<<ccr:" in source or max(len(source.encode()), len(payload.encode())) > MAX_BYTES:
        raise ValueError("bounded marker-free recovery source required")
    keys = CCRToolInjector().scan_for_markers([{"role": "tool", "content": payload}])
    if not keys or len(keys) > 64:
        raise ValueError("bounded generated recovery markers required")
    from collections import Counter
    rows = json.loads(source)
    row_path = request.get("source_row_path", [])
    if (not isinstance(row_path, list) or len(row_path) > 16 or any(
            not ((isinstance(key, str) and key and len(key) <= 256)
                 or (type(key) is int and 0 <= key < 512)) for key in row_path)):
        raise ValueError("bounded explicit source row path required")
    for key in row_path:
        if isinstance(key, str) and isinstance(rows, dict) and key in rows:
            rows = rows[key]
        elif type(key) is int and isinstance(rows, list) and key < len(rows):
            rows = rows[key]
        else:
            raise ValueError("source row path does not identify current source data")
    if not isinstance(rows, list):
        raise ValueError("JSON array recovery source required")
    def canonical(row):
        return json.dumps(row, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
    expected = Counter(canonical(row) for row in rows)
    results = []
    for key in keys:
        start = time.perf_counter()
        entry = get_compression_store().retrieve(key)
        elapsed = (time.perf_counter() - start) * 1000
        matches = False
        if entry is not None and len(entry.original_content.encode()) <= MAX_BYTES:
            recovered = json.loads(entry.original_content)
            matches = isinstance(recovered, list) and Counter(canonical(row) for row in recovered) == expected
        results.append({"source_matches": matches, "store_retrieval_ms": elapsed,
                        "status": "miss" if entry is None else "hit"})
    return {"kind": "separate-process-source-bound-store-audit", "retrievals": results,
            "source_row_path": row_path,
            "limitation": "No recovered text is exported; not model-directed retrieval, "
                          "cross-request authorization, outer-wrapper restoration, complete recovery economics or quality qualification."}


def smart_worker(request):
    """Default upstream tool-response path; missing CCR recovery is invalid, not a loss."""
    import importlib.util
    from copy import deepcopy
    from dataclasses import asdict

    if request.get("audit_recovery") is True and "<<ccr:" in request["source"]:
        raise ValueError("recovery audit refuses source-authored markers")

    path = Path(request["native_core"]).resolve()
    if hashlib.sha256(path.read_bytes()).hexdigest() != request["native_core_sha256"]:
        raise ValueError("native comparator artifact changed")
    spec = importlib.util.spec_from_file_location("headroom._core", path)
    core = importlib.util.module_from_spec(spec)
    sys.modules["headroom._core"] = core
    spec.loader.exec_module(core)
    from headroom.transforms import smart_crusher
    from headroom import OpenAIProvider, Tokenizer
    if Path(smart_crusher.__file__).resolve() != Path(request["root"]) / "headroom/transforms/smart_crusher.py":
        raise ValueError("wrong SmartCrusher import")
    messages = [{"role": "user", "content": request["query"]},
                {"role": "assistant", "content": None, "tool_calls": [{"id": "call_1", "type": "function",
                    "function": {"name": "public_fixture", "arguments": "{}"}}]},
                {"role": "tool", "tool_call_id": "call_1", "content": request["source"]}]
    original = deepcopy(messages)

    class Health(logging.Handler):
        degraded = False

        def emit(self, record):
            if record.levelno >= logging.WARNING:
                self.degraded = True

    health = Health()
    logger = logging.getLogger()
    logger.addHandler(health)
    try:
        config = smart_crusher.SmartCrusherConfig()
        if request.get("audit_recovery") is True and request.get("audit_force_lossy") is True:
            config.lossless_min_savings_ratio = 0.99  # Explicit upstream protocol canary, never default qualification.
        compressor = smart_crusher.SmartCrusher(config)
        tokenizer = Tokenizer(OpenAIProvider().get_token_counter("gpt-4o"), "gpt-4o")
        result = compressor.apply(messages, tokenizer)
    finally:
        logger.removeHandler(health)
    if (len(result.messages) != 3 or result.messages[:2] != original[:2]
            or result.messages[2].get("role") != "tool" or result.messages[2].get("tool_call_id") != "call_1"):
        raise ValueError("SmartCrusher changed tool protocol")
    payload = result.messages[2]["content"]
    retrieval_required = "<<ccr:" in payload
    response = {"payload": payload, "receipt": {"kind": "headroom-smartcrusher-default-tool-snapshot-v1",
        "healthy": not health.degraded and not result.warnings and not retrieval_required,
        "valid_attempt": not health.degraded and not result.warnings and not retrieval_required,
        "native_core_sha256": request["native_core_sha256"], "config": asdict(config),
        "transforms": result.transforms_applied, "markers_inserted": result.markers_inserted,
        "reason": "unimplemented-ccr-recovery" if retrieval_required else "warning" if health.degraded or result.warnings else None,
        "limitation": "Pinned default tool snapshot, not full proxy/cache/retrieval qualification; upstream default relevance backend is used unchanged."}}
    if request.get("audit_recovery") is True:
        from headroom.ccr.tool_injection import CCRToolInjector
        from headroom.cache.compression_store import get_compression_store, detach_compression_store
        keys = CCRToolInjector().scan_for_markers(result.messages[2:])
        def mirrored(key):
            entry = get_compression_store().retrieve(key)
            return entry.original_content if entry is not None else None
        def reopened(key):
            detach_compression_store()  # Upstream non-destructive reopen, not reset/clear.
            return mirrored(key)
        response["receipt"]["recovery_audit"] = audit_recovery(
            request["source"], payload, keys, compressor.ccr_get, mirrored, reopened)
        response["receipt"]["recovery_audit"]["forced_lossy_canary"] = request.get("audit_force_lossy") is True
    return response


if __name__ == "__main__":
    if sys.argv[1:] != ["--worker"]:
        raise SystemExit("Use acon_benchmark.py with an explicitly pinned comparator.")
    try:
        request = json.loads(sys.stdin.read(MAX_BYTES + 1))
        print(json.dumps(worker(request)))
    except Exception as exc:
        print(type(exc).__name__, file=sys.stderr)  # Type only; never source/server diagnostics.
        raise SystemExit(1)
