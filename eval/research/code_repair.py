"""Bounded in-memory edits for research repair capsules; never executes candidate code."""

import json
import hashlib
import time
import sys
from pathlib import Path

from native_model import unique_object


class EditError(ValueError):
    """Fixed codes only; never source spans or parser diagnostics."""


def isolated_test_command(image_id: str, capsule: Path, name: str) -> list[str]:
    """Prepare native Docker policy only; caller must approve/runtime-preflight execution.

    Not an isolation proof. The caller must enforce a deadline and remove a timed-out
    container; killing the CLI alone does not stop server-side work.
    """
    if (not isinstance(image_id, str) or not image_id.startswith("sha256:") or len(image_id) != 71
            or any(c not in "0123456789abcdef" for c in image_id[7:])
            or not isinstance(name, str) or not name.startswith("tokenfold-repair-")
            or len(name) > 64 or any(c not in "abcdefghijklmnopqrstuvwxyz0123456789-" for c in name)):
        raise ValueError("immutable local image ID and bounded container name required")
    capsule = capsule.resolve()
    if not capsule.is_dir() or not (capsule / "evaluator.py").is_file() or "," in str(capsule):
        raise ValueError("explicit capsule directory/evaluator required; mount delimiters refused")
    return ["docker", "run", "--name", name, "--rm", "--pull", "never", "--network", "none",
            "--read-only", "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
            "--user", "65534:65534", "--pids-limit", "32", "--memory", "256m", "--cpus", "1",
            "--tmpfs", "/tmp:rw,noexec,nosuid,size=16m", "--mount",
            "type=bind,src=" + str(capsule) + ",dst=/capsule,readonly", "--workdir", "/capsule",
            image_id, "python", "-I", "-B", "/capsule/evaluator.py"]


def freeze_capsule(actor: Path, evaluators: list[Path], dependencies: list[Path]) -> dict:
    """Pin role-separated local inputs; no execution/export authorization."""
    if not evaluators or len(evaluators) + len(dependencies) > 128:
        raise ValueError("bounded nonempty evaluator inputs required")
    roles = {"actor": [actor.resolve()], "evaluator": [p.resolve() for p in evaluators],
             "dependency": [p.resolve() for p in dependencies]}
    paths = [p for group in roles.values() for p in group]
    if len(set(paths)) != len(paths):
        raise ValueError("capsule input roles must not overlap")
    data = {}
    for path in paths:
        with path.open("rb") as stream:
            data[path] = stream.read(1024 * 1024 + 1)
        if len(data[path]) > 1024 * 1024:
            raise ValueError("capsule input byte limit")
    actor_data = json.loads(data[roles["actor"][0]], object_pairs_hook=unique_object)
    if (not isinstance(actor_data, dict) or set(actor_data) - {"kind", "query", "source_files", "output_contract", "cluster_id", "contamination"}
            or not {"query", "source_files", "output_contract"} <= set(actor_data)):
        raise ValueError("actor envelope must exclude evaluator fields")
    for path in paths:
        with path.open("rb") as stream:
            final = stream.read(1024 * 1024 + 1)
        if final != data[path]:
            raise ValueError("capsule input changed while freezing")
    return {"kind": "local-code-capsule-freeze-not-qualification", "roles": {
        role: [{"path": str(path), "sha256": hashlib.sha256(data[path]).hexdigest()} for path in group]
        for role, group in roles.items()}, "python": {"version": sys.version,
        "executable_sha256": hashlib.sha256(Path(sys.executable).read_bytes()).hexdigest()},
        "limitation": "Role separation/hashes do not prove source authorization, test quality, representativeness or isolated execution."}


def edit_receipt(source: str, response: str) -> dict:
    start = time.perf_counter()
    receipt = {"kind": "literal-edit-application-not-repair-verdict", "applied": False,
               "reason": None, "source_sha256": None, "response_sha256": None,
               "repaired_source_sha256": None, "candidate_code_executed": False}
    try:
        for name, value, limit in (("source", source, 1024 * 1024), ("response", response, 65536)):
            if not isinstance(value, str) or len(value.encode()) > limit:
                raise EditError("input_limit")
            receipt[name + "_sha256"] = hashlib.sha256(value.encode()).hexdigest()
        repaired = apply_edits(source, response)
        receipt.update(applied=True, repaired_source_sha256=hashlib.sha256(repaired.encode()).hexdigest())
    except EditError as exc:
        code = str(exc)
        receipt["reason"] = code if code in {"input_limit", "invalid_edit_list", "invalid_edit_schema",
                                            "nonunique_or_missing_span", "output_limit"} else "edit_application_failed"
    except ValueError:
        receipt["reason"] = "invalid_json_or_encoding"
    receipt["wall_ms"] = (time.perf_counter() - start) * 1000
    return receipt


def apply_edits(source: str, response: str) -> str:
    """Apply explicit literal edits to one caller-owned file, with no paths or commands.

    Execution/evaluation requires a separate isolated runtime; parsing is not success.
    """
    if (not isinstance(source, str) or not isinstance(response, str)
            or len(source.encode()) > 1024 * 1024 or len(response.encode()) > 65536):
        raise EditError("input_limit")
    candidate = json.loads(response, object_pairs_hook=unique_object)
    if (not isinstance(candidate, dict) or set(candidate) != {"edits"}
            or not isinstance(candidate["edits"], list) or not 1 <= len(candidate["edits"]) <= 16):
        raise EditError("invalid_edit_list")
    result = source
    for edit in candidate["edits"]:
        if (not isinstance(edit, dict) or set(edit) != {"find", "replace"}
                or not isinstance(edit["find"], str) or not edit["find"]
                or not isinstance(edit["replace"], str)):
            raise EditError("invalid_edit_schema")
        if result.count(edit["find"]) != 1:
            raise EditError("nonunique_or_missing_span")
        result = result.replace(edit["find"], edit["replace"], 1)
        if len(result.encode()) > 1024 * 1024:
            raise EditError("output_limit")
    return result
