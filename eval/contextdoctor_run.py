"""Opt-in bounded execution. Imported only by the explicit `run` command."""
from __future__ import annotations

from datetime import datetime, timezone
from decimal import Decimal
import json
import hashlib
import os
from pathlib import Path, PurePosixPath
import re
import signal
import subprocess
import sys
import tempfile
import threading
import time
import uuid

import contextdoctor as cd


class ExecutionError(RuntimeError):
    def __init__(self, reason, cost=None, request_id=None, usage=None):
        self.reason = reason
        self.cost = cost
        self.request_id = request_id
        self.usage = usage
        super().__init__(reason)


def implementation_hash():
    root = Path(__file__).resolve().parent
    return cd.canonical_digest({name: cd.hash_bytes(cd.read_bytes(root / name))
                                for name in ("contextdoctor.py", "contextdoctor_run.py", "contextdoctor_openrouter.py")})


def executable_hash(path):
    cd.require(path.is_file(), "executable is not a regular file")
    value = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            value.update(block)
    return "sha256:" + value.hexdigest()


def validate_approval(m, a):
    cd.fields(a, "schema_version kind manifest_hash schedule_hash execution_hash implementation_hash python_hash expires_at adapter limits image docker_path docker_hash external_data credential_env", "approval")
    cd.header(a, "approval")
    cd.require(m["execution"] is not None, "execution not configured")
    plan = cd.build_plan(m)
    for key in ("manifest_hash", "schedule_hash", "execution_hash"):
        cd.require(a[key] == plan[key], "approval scope mismatch")
    cd.require(a["implementation_hash"] == implementation_hash(), "implementation approval mismatch")
    cd.require(a["python_hash"] == executable_hash(Path(sys.executable)), "Python executable approval mismatch")
    cd.require(a["adapter"] == m["execution"]["adapter"] and a["image"] == m["execution"]["image"], "adapter/image approval mismatch")
    cd.require(a["limits"] == m["limits"], "approval cap mismatch")
    cd.require(Decimal(m["limits"]["max_cost"]) == 0, "paid execution is not supported")
    cd.require(m["limits"]["max_tokens"] <= 4096 and m["limits"]["max_output_bytes"] <= 8 * 1024 * 1024 and
               Decimal(m["limits"]["max_seconds"]) <= 3600, "execution hard limit exceeded")
    cd.require(m["verifier"]["kind"] == "exact_match" and m["verifier"]["reference_health"] == "passed" and
               m["verifier"]["digest"] == cd.hash_bytes(cd.EXACT_VERIFIER.encode()), "unsupported/unhealthy verifier")
    cd.require(m["verifier"]["spec_hash"] == cd.canonical_digest({"algorithm": cd.EXACT_VERIFIER,
               "references": {t["id"]: t["reference_hash"] for t in m["tasks"]}}), "verifier reference spec mismatch")
    cd.require(m["cost_policy"]["categories"]["infrastructure"]["allocation"] == "equal_arms", "setup requires frozen equal-arm allocation")
    cd.require(a["external_data"] in ("synthetic_only", "approved_local_inputs"), "invalid data approval")
    cd.require(isinstance(a["expires_at"], str) and re.fullmatch(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ", a["expires_at"]), "invalid approval expiry")
    try:
        expiry = datetime.fromisoformat(a["expires_at"].replace("Z", "+00:00"))
    except ValueError as exc:
        raise cd.ContractError("invalid expiry") from exc
    cd.require(expiry > datetime.now(timezone.utc), "approval expired")
    if a["adapter"] == "openrouter_free":
        cd.require(a["external_data"] == "synthetic_only" and all(t["provenance"]["class"] == "synthetic_mechanics" for t in m["tasks"]),
                   "remote testing admits only explicitly approved synthetic text")
        cd.require(a["credential_env"] == "OPENROUTER_API_KEY", "only OPENROUTER_API_KEY is admitted")
        cd.require(m["execution"]["catalog_hash"] is not None, "missing frozen catalog")
        cd.require(m["analysis"]["mode"] == "diagnostic", "free-provider testing is diagnostic only")
        rates = m["cost_policy"]["rates"]
        cd.require(m["cost_policy"]["currency"] == "USD" and {r["metric"] for r in rates} == set(cd.METRICS) and
                   all(Decimal(r["rate"]) == 0 and r["evidence_hash"] == m["execution"]["catalog_hash"] for r in rates),
                   "free-provider usage rates must be zero and bound to the frozen catalog")
    else:
        cd.require(a["credential_env"] is None and a["image"] is not None, "container credentials/image invalid")
    needs_docker = a["adapter"] == "container" or any(v["transformation"] == "tokenfold" for v in m["arms"].values())
    if needs_docker:
        cd.require(isinstance(a["docker_path"], str) and Path(a["docker_path"]).is_absolute(), "Docker must be an approved absolute executable")
        cd.require(executable_hash(Path(a["docker_path"])) == a["docker_hash"], "Docker executable hash mismatch")
        cd.hash_value(a["image"], "Docker image")
    for arm in cd.ARMS:
        cd.require(m["arms"][arm]["transformation"] in ("identity", "tokenfold"), "unsupported execution transformation")
        if m["arms"][arm]["transformation"] == "tokenfold":
            cd.require(m["arms"][arm]["config_hash"] == cd.canonical_digest(m["execution"]["tokenfold_argv"]), "Tokenfold config pin mismatch")
            validate_tokenfold_argv(m["execution"]["tokenfold_argv"])


def minimal_environment(extra=None):
    env = {k: os.environ[k] for k in ("SystemRoot", "WINDIR", "TEMP", "TMP") if k in os.environ}
    env.update({"PATH": os.defpath, "PYTHONNOUSERSITE": "1", "PYTHONUTF8": "1"})
    if extra:
        env.update(extra)
    return env


def bounded_process(argv, payload, seconds, output_limit, env=None, capture_stderr=False):
    """Bound a trusted transport shim; untrusted programs additionally require Docker."""
    if seconds <= 0:
        raise ExecutionError("timeout")
    kwargs = {"start_new_session": True} if os.name != "nt" else {"creationflags": subprocess.CREATE_NEW_PROCESS_GROUP}
    clean = tempfile.TemporaryDirectory(prefix="contextdoctor_transport_")
    environment = minimal_environment(env)
    environment.update({"HOME": clean.name, "USERPROFILE": clean.name, "DOCKER_CONFIG": clean.name, "XDG_CONFIG_HOME": clean.name})
    try:
        process = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   env=environment, cwd=clean.name, shell=False, **kwargs)
    except BaseException:
        clean.cleanup()
        raise
    overflow, lock = threading.Event(), threading.Lock()
    chunks = {"stdout": [], "stderr": []}
    used = [0]

    def drain(pipe, name):
        try:
            while True:
                block = os.read(pipe.fileno(), min(4096, output_limit + 1))
                if not block:
                    return
                with lock:
                    used[0] += len(block)
                    if used[0] > output_limit:
                        overflow.set()
                        return
                    chunks[name].append(block)
        except (OSError, ValueError):
            return

    def feed():
        try:
            process.stdin.write(payload)
            process.stdin.close()
        except (OSError, ValueError):
            pass

    readers = [threading.Thread(target=drain, args=(process.stdout, "stdout"), daemon=True),
               threading.Thread(target=drain, args=(process.stderr, "stderr"), daemon=True)]
    writer = threading.Thread(target=feed, daemon=True)
    deadline = time.monotonic() + seconds
    reason = None
    try:
        for thread in (*readers, writer):
            thread.start()
        while process.poll() is None:
            if overflow.wait(0.01):
                reason = "output_limit"
                break
            if time.monotonic() >= deadline:
                reason = "timeout"
                break
        if reason:
            raise ExecutionError(reason)
        process.wait()
        for thread in readers:
            thread.join(timeout=max(0, deadline - time.monotonic()))
        if overflow.is_set():
            raise ExecutionError("output_limit")
        if any(thread.is_alive() for thread in readers):
            raise ExecutionError("timeout")
        if process.returncode:
            raise ExecutionError("process_failed")
        output = b"".join(chunks["stdout"])
        return (output, b"".join(chunks["stderr"])) if capture_stderr else output
    finally:
        if os.name != "nt":
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        elif process.poll() is None:
            process.kill()
        process.wait(timeout=5)
        for pipe in (process.stdin, process.stdout, process.stderr):
            try:
                pipe.close()
            except (OSError, ValueError):
                pass
        for thread in (*readers, writer):
            if thread.ident:
                thread.join(timeout=1)
        clean.cleanup()


class Container:
    def __init__(self, approval, execution):
        self.approval, self.execution = approval, execution
        local = "npipe:////./pipe/docker_engine" if os.name == "nt" else "unix:///var/run/docker.sock"
        self.base = [approval["docker_path"], "--host", local]

    def preflight(self, seconds=10):
        raw = bounded_process(self.base + ["image", "inspect", self.approval["image"], "--format", "{{json .}}"], b"", min(seconds, 10), 4 * 1024 * 1024)
        image = json.loads(raw)
        cd.require(image.get("Id") == self.approval["image"] and image.get("Os") == "linux", "approved local Linux image unavailable")
        cd.require(not image.get("Config", {}).get("Volumes"), "image-declared volumes are refused")
        # CPU-only, no device passthrough, bind mounts, daemon socket, host env or network.

    def invoke(self, argv, payload, seconds, limit, capture_stderr=False):
        cd.require(isinstance(argv, list) and bool(argv) and PurePosixPath(argv[0]).is_absolute() and
                   all(isinstance(a, str) and "\x00" not in a for a in argv), "invalid pinned container argv")
        name = "contextdoctor-" + uuid.uuid4().hex
        command = self.base + ["run", "--name", name, "--rm", "--interactive", "--pull=never", "--network=none", "--read-only",
                               "--cap-drop=ALL", "--security-opt=no-new-privileges", "--user=65534:65534", "--pids-limit=64",
                               "--memory=512m", "--memory-swap=512m", "--cpus=1", "--log-driver=none", "--stop-timeout=1",
                               "--no-healthcheck",
                               "--workdir=/tmp", "--tmpfs=/tmp:rw,noexec,nosuid,nodev,size=16777216",
                               "--entrypoint", argv[0], self.approval["image"], *argv[1:]]
        try:
            return bounded_process(command, payload, seconds, limit, capture_stderr=capture_stderr)
        finally:
            # Removing by our generated name also cancels descendants in the container.
            # Auto-removal races with explicit removal on normal completion; absence is OK.
            try:
                bounded_process(self.base + ["rm", "--force", name], b"", 5, 8192)
            except ExecutionError as exc:
                if exc.reason == "process_failed":
                    try:
                        remaining = bounded_process(self.base + ["ps", "--all", "--filter", f"name=^/{name}$", "--format", "{{.ID}}"], b"", 5, 8192)
                        if remaining.strip():
                            raise ExecutionError("container_cleanup_unconfirmed")
                    except (ExecutionError, OSError) as cleanup:
                        raise ExecutionError("container_cleanup_unconfirmed") from cleanup
                else:
                    raise ExecutionError("container_cleanup_unconfirmed") from exc


def validate_tokenfold_argv(argv):
    cd.require(len(argv) >= 3 and argv[1:3] == ["compress", "-"], "only stdin tokenfold compress is supported")
    pairs = {"--format", "--target-tokens", "--preset", "--encoding"}
    index = 3
    while index < len(argv):
        cd.require(argv[index] in pairs and index + 1 < len(argv) and not argv[index + 1].startswith("-"), "unsupported Tokenfold flag/recovery scope")
        index += 2


def tokenfold_transform(container, argv, source, seconds, limit):
    validate_tokenfold_argv(argv)
    payload, receipt_bytes = container.invoke(argv, source, seconds, limit, capture_stderr=True)
    try:
        receipt = json.loads(receipt_bytes, object_pairs_hook=cd._pairs, parse_float=str, parse_constant=cd._not_integer)
    except (json.JSONDecodeError, UnicodeError) as exc:
        raise cd.ContractError("missing Tokenfold receipt") from exc
    cd.require(isinstance(receipt, dict) and receipt.get("schema_version") in ("1.0", "2.0") and
               receipt.get("status") in ("compressed", "passthrough"), "unsupported Tokenfold receipt")
    cd.require(b'"$tf_ref"' not in payload and not receipt.get("pruning") and not receipt.get("retrieval"), "retrieval-capable execution needs a separately admitted adapter")
    disposition = "fallback" if receipt.get("bypass") else "no_op" if payload == source else "applied"
    cd.require(disposition != "fallback" or payload == source, "fallback did not preserve source")
    return {"payload": payload, "receipt_bytes": receipt_bytes, "disposition": disposition}


def _append(path, value):
    with path.open("a", encoding="utf-8", newline="\n") as handle:
        handle.write(json.dumps(value, sort_keys=True, ensure_ascii=False, allow_nan=False) + "\n")
        handle.flush()
        os.fsync(handle.fileno())


def _unknown_cost(m, reason):
    return {"basis": "unavailable", "amount": None, "currency": m["cost_policy"]["currency"],
            "receipt_hash": None, "reconciled": False, "missing_reason": reason}


def run(manifest_path, approval_path, out):
    m, _ = cd.load_manifest(manifest_path)
    approval_raw = cd.read_bytes(approval_path)
    approval = cd.parse_json(approval_raw)
    validate_approval(m, approval)
    root = manifest_path.parent.resolve()
    source_artifacts = {}
    def read_artifact(h):
        if h not in source_artifacts:
            cd.require(h in m["bindings"]["artifacts"], "unbound execution input")
            raw = cd.read_bytes(cd.safe_path(root, m["bindings"]["artifacts"][h]))
            cd.require(cd.hash_bytes(raw) == h, "execution input tampered")
            source_artifacts[h] = raw
        return source_artifacts[h]
    for task in m["tasks"]:
        for k in ("source_hash", "prompt_hash", "reference_hash"):
            read_artifact(task[k]).decode("utf-8")
        cd.require(task["snapshot_hash"] == task["source_hash"], "only immutable text snapshots are supported")
        cd.require(len(read_artifact(task["source_hash"])) + len(read_artifact(task["prompt_hash"])) + 128 <= m["limits"]["max_output_bytes"], "execution IPC input limit exceeded")
    catalog = None
    if approval["adapter"] == "openrouter_free":
        from contextdoctor_openrouter import validate_catalog
        catalog = cd.parse_json(read_artifact(m["execution"]["catalog_hash"]))
        validate_catalog(catalog)
        cd.require(catalog["model"] == m["pins"]["model"] and catalog["provider"] == m["execution"]["provider"], "catalog pin mismatch")
        cd.require(bool(os.environ.get("OPENROUTER_API_KEY")), "OPENROUTER_API_KEY_missing")
    container = Container(approval, m["execution"]) if approval["adapter"] == "container" or any(a["transformation"] == "tokenfold" for a in m["arms"].values()) else None
    # Availability is checked within the ledger below, not silently skipped.
    out.mkdir(parents=True, exist_ok=False, mode=0o700)
    private = out / "private"
    private.mkdir(mode=0o700)
    m["bindings"] = {"operations": "operations.jsonl", "artifacts": {}}
    op_path, ep_path = out / "operations.jsonl", out / "episodes.jsonl"
    op_path.touch(exist_ok=False)
    ep_path.touch(exist_ok=False)
    (private / "approval.json").write_bytes(approval_raw)

    def save_manifest():
        temporary = out / "experiment.json.tmp"
        cd.write_json(temporary, m)
        with temporary.open("r+b") as handle:
            os.fsync(handle.fileno())
        os.replace(temporary, out / "experiment.json")

    def store(raw, suffix):
        h = cd.hash_bytes(raw)
        relative = "private/" + h[7:] + suffix
        path = out / relative
        if not path.exists():
            with path.open("xb") as handle:
                handle.write(raw)
                handle.flush()
                os.fsync(handle.fileno())
        m["bindings"]["artifacts"][h] = relative
        save_manifest()
        return h

    for raw in source_artifacts.values():
        store(raw, ".input")
    store(approval_raw, ".approval.json")
    save_manifest()
    started, used, failure = time.monotonic(), 0, None
    active = None
    limits = m["limits"]
    episodes = []

    def operation(eid, role, category, callback):
        nonlocal used
        remaining = float(limits["max_seconds"]) - (time.monotonic() - started)
        if used >= limits["max_operations"] or remaining <= 0:
            raise ExecutionError("scope_cap_exhausted")
        used += 1
        oid = f"operation-{used:06d}"
        if active is not None:
            active["operation_ids"].append(oid)
        base = {"schema_version": cd.SCHEMA_VERSION, "kind": "contextdoctor.operation", "operation_id": oid,
                "episode_id": eid, "experiment_id": m["experiment_id"], "manifest_hash": m["content_hash"],
                "event": "dispatch", "role": role, "category": category, "status": "dispatched", "duration_seconds": None,
                "usage": None, "cost": _unknown_cost(m, "not_reconciled"), "request_id": None}
        _append(op_path, base)
        begin = time.monotonic()
        terminal = {**base, "event": "terminal"}
        try:
            value = callback(min(remaining, float(limits["operation_seconds"])))
            terminal.update(status="completed", cost=_unknown_cost(m, "excluded_category" if m["cost_policy"]["categories"][category]["coverage"] == "excluded" else "missing_cost_evidence"))
            if isinstance(value, dict) and "usage" in value:
                terminal.update(usage=value["usage"], request_id=value.get("request_id"))
                if value["usage"] is not None:
                    terminal["cost"] = {"basis": "usage_priced", "amount": None, "currency": m["cost_policy"]["currency"],
                                        "receipt_hash": None, "reconciled": False, "missing_reason": value.get("cost_coverage_reason")}
            return value
        except BaseException as exc:
            terminal.update(status="timeout" if isinstance(exc, ExecutionError) and exc.reason == "timeout" else
                            "cancelled" if isinstance(exc, KeyboardInterrupt) else "failed", cost=_unknown_cost(m, "operation_failed_billing_unknown"))
            if isinstance(exc, ExecutionError):
                terminal["request_id"] = exc.request_id
                if exc.cost is not None:
                    terminal["cost"] = exc.cost
                if exc.usage is not None:
                    terminal["usage"] = exc.usage
            raise
        finally:
            terminal["duration_seconds"] = format(Decimal(str(time.monotonic() - begin)).quantize(Decimal("0.000001")), "f")
            _append(op_path, terminal)

    def provider_call(action, context, query, seconds):
        payload = {"action": action, "catalog": catalog, "context": context, "query": query,
                   "max_tokens": m["execution"]["settings"]["max_tokens"], "temperature": m["execution"]["settings"]["temperature"],
                   "timeout": str(seconds), "output_limit": limits["max_output_bytes"]}
        raw = bounded_process([sys.executable, "-I", str(Path(__file__).with_name("contextdoctor_openrouter.py")), "worker"],
                              json.dumps(payload).encode(), seconds, limits["max_output_bytes"],
                              env={"OPENROUTER_API_KEY": os.environ["OPENROUTER_API_KEY"]})
        value = cd.parse_json(raw)
        receipt_hash = None
        if "receipt" in value:
            receipt_hash = store(json.dumps(value["receipt"], sort_keys=True, ensure_ascii=False).encode(), ".provider.json")
        if action == "request" and not value.get("ok"):
            receipt = value.get("receipt") or {}
            observed = (receipt.get("usage") or {}).get("cost")
            known_cost = None
            if observed is not None and not isinstance(observed, bool):
                try:
                    amount = cd.decimal(str(observed), "provider observed charge")
                    known_cost = {"basis": "provider_billed", "amount": cd.money(amount), "currency": "USD", "receipt_hash": receipt_hash,
                                  "reconciled": False, "missing_reason": "provider_charge_unreconciled"}
                except ValueError:
                    pass
            usage = value.get("usage")
            cd.normalized_usage(usage, m["pins"]["model"])
            reason = "provider_response_refused"
            if value.get("reason") == "provider_http_error":
                cd.integer(value.get("http_status"), "HTTP status", 400, 599)
                reason = "provider_http_" + str(value["http_status"])
            raise ExecutionError(reason, cost=known_cost, request_id=value.get("request_id"), usage=usage)
        return value

    try:
        if container:
            operation(None, "setup", "infrastructure", lambda seconds: container.preflight(seconds))
        if catalog:
            operation(None, "setup", "infrastructure", lambda seconds: provider_call("preflight", "", "", seconds))
        tasks = {t["id"]: t for t in m["tasks"]}
        for cell in cd.build_plan(m)["cells"]:
            task = tasks[cell["task_id"]]
            arm = cell["arm"]
            eid = "episode-" + str(len(episodes) + 1).zfill(6)
            begin = time.monotonic()
            source = source_artifacts[task["source_hash"]]
            prompt = source_artifacts[task["prompt_hash"]]
            reference = source_artifacts[task["reference_hash"]]
            active = {"schema_version": cd.SCHEMA_VERSION, "kind": "contextdoctor.episode", "id": eid,
                      "experiment_id": m["experiment_id"], "manifest_hash": m["content_hash"], **cell,
                      "pins_hash": cd.canonical_digest(m["pins"]), "source_hash": task["source_hash"], "prompt_hash": task["prompt_hash"],
                      "snapshot_hash": task["snapshot_hash"], "transform_hash": cd.canonical_digest(m["arms"][arm]),
                      "status": "unknown", "request_hash": None, "output_hash": None, "verification_hash": None,
                      "treatment": {"enabled": arm == "candidate", "disposition": "unknown", "input_hash": task["source_hash"],
                                    "output_hash": None, "receipt_hash": None}, "operation_ids": [], "wall_seconds": None,
                      "safety_violation": False}
            try:
                transformation = m["arms"][arm]["transformation"]
                result = operation(eid, "transform", "transform", lambda seconds: {"payload": source, "disposition": "no_op"} if transformation == "identity" else
                                   tokenfold_transform(container, m["execution"]["tokenfold_argv"], source, seconds, limits["max_output_bytes"]))
                transformed = result["payload"]
                receipt = {"schema_version": cd.SCHEMA_VERSION, "kind": "contextdoctor.transformation",
                           "input_hash": task["source_hash"], "output_hash": cd.hash_bytes(transformed), "transformation": transformation}
                active["treatment"].update(disposition=result["disposition"], output_hash=cd.hash_bytes(transformed),
                                           receipt_hash=store(result.get("receipt_bytes", json.dumps(receipt, sort_keys=True).encode()), ".transform.json"))
                payload = json.dumps({"context": transformed.decode("utf-8"), "query": prompt.decode("utf-8")}, ensure_ascii=False).encode()
                active["request_hash"] = cd.hash_bytes(payload)
                if catalog:
                    answer = operation(eid, "agent", "model", lambda seconds: provider_call("request", transformed.decode("utf-8"), prompt.decode("utf-8"), seconds))
                    output = answer["output"].encode("utf-8")
                else:
                    output = operation(eid, "agent", "model", lambda seconds: container.invoke(m["execution"]["agent_argv"], payload, seconds, limits["max_output_bytes"]))
                active["output_hash"] = store(output, ".output")
                def verify(seconds):
                    passed = output.decode("utf-8").strip() == reference.decode("utf-8").strip()
                    return {"schema_version": cd.SCHEMA_VERSION, "kind": "contextdoctor.verification", "episode_id": eid,
                            "manifest_hash": m["content_hash"], "output_hash": active["output_hash"], "verifier_hash": m["verifier"]["digest"],
                            "independence": "external", "reference_health": "passed", "status": "passed" if passed else "failed",
                            "checks": [{"id": "exact_answer", "result": "passed" if passed else "failed"}], "evidence_hash": task["reference_hash"]}
                verification = operation(eid, "verification", "verification", verify)
                active["verification_hash"] = store(json.dumps(verification, sort_keys=True).encode(), ".verification.json")
                active["status"] = "completed"
            finally:
                if not active["operation_ids"]:
                    active["status"] = "unexecuted"
                active["wall_seconds"] = format(Decimal(str(time.monotonic() - begin)).quantize(Decimal("0.000001")), "f")
                _append(ep_path, active)
                episodes.append(active)
                active = None
    except (Exception, KeyboardInterrupt) as exc:
        failure = exc.reason if isinstance(exc, ExecutionError) else "execution_stopped"
    finally:
        save_manifest()
        cd.write_json(out / "execution.json", {"schema_version": cd.SCHEMA_VERSION, "kind": "contextdoctor.execution",
                                               "manifest_hash": m["content_hash"], "approval_hash": cd.hash_bytes(approval_raw),
                                               "operations_dispatched": used, "episodes_recorded": len(episodes),
                                               "stopped_reason": failure, "complete": failure is None})
    report = cd.build_report(cd.load_experiment(out / "experiment.json", ep_path))
    cd.write_json(out / "report.json", report)
    cd.write_json(out / "pilot.json", {"schema_version": cd.SCHEMA_VERSION, "kind": "contextdoctor.pilot",
                                       "report_hash": cd.hash_bytes(cd.read_bytes(out / "report.json")),
                                       "approval_hash": cd.hash_bytes(approval_raw), "adapter": approval["adapter"],
                                       "completed": failure is None, "evidence_class": m["tasks"][0]["provenance"]["class"]})
    with (out / "report.md").open("w", encoding="utf-8", newline="\n") as handle:
        handle.write(cd.render_markdown(report))
    print(f"execution {'stopped' if failure else 'finished'}; {out / 'report.json'}")
    return 1 if failure else 0
