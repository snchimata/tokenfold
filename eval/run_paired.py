#!/usr/bin/env python3
"""Paired raw-vs-candidate run records and their aggregation (offline, stdlib only).

WHAT THIS IS
------------
The scoring half of the quality/economics runner: a *paired* record schema, a
fail-closed reader, and the aggregation the contract fixes in advance -- all
four paired outcomes, the conditional contrastive regression rate (CFR), the
absolute success delta, and an uncertainty bound.

    CFR = #{raw success AND candidate failure} / #{raw success}

No raw successes means CFR is *unavailable* (`None`, never `0.0`): the metric
has no denominator, and reporting zero there would invent evidence.

WHAT THIS IS NOT
----------------
This module does not measure downstream task quality by itself. It only
*aggregates* outcomes a runner produced, and its offline driver uses a
deterministic dummy model (a scripted reader, not an LLM) to prove the
mechanism end to end. A green run here certifies the record contract and the
arithmetic -- never that a transform is safe to promote. Live budgeted
execution and per-regression attribution stay outside this file.

The measurement fields a real runner copies in are the EP-02 per-attempt
`MeasurementEvent`; they are attached verbatim under `measurement` and are
optional here, so an offline record needs no provider round trip.

USAGE
-----
    python eval/run_paired.py --records runs.jsonl
    python eval/run_paired.py --records runs.jsonl --gate --max-cfr 0.005
    python eval/run_paired.py --run-offline --tasks-dir eval/tasks/paired
    python eval/run_paired.py --run-offline --observation-arm

The `--observation-arm` pass additionally drives the real `tokenfold-proxy` (build
it, or set `TOKENFOLD_PROXY_BIN`) over a loopback echo upstream, so the observation
path is exercised on real requests rather than only unit-tested. It is skipped,
with a notice, when the proxy binary is absent.
    python eval/run_paired.py --live-arm --live-model MODEL --live-upstream URL

The `--live-arm` pass drives raw vs observation-compressed transcripts through a
REAL model behind the real proxy and scores the model's own answers. Ollama
(http://localhost:11434) and OpenRouter free models (:free, upstream
https://openrouter.ai/api) are supported; a paid OpenRouter model is refused
even with `--live-allow-paid`: a priced runner with an enforced monetary cap is
not yet available. The consent flag alone cannot enforce a spend cap.

DEPENDENCIES
------------
Python standard library only, plus the existing `run_baselines` harness for
token counting, the isolated retrieval config and the `tokenfold` CLI
subprocesses (`--run-offline` only); the `--observation-arm` pass also needs the
`tokenfold-proxy` binary.
"""

from __future__ import annotations

import argparse
import hashlib
import http.server
import json
import math
import os
import random
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse
import urllib.request
import urllib.error
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import run_baselines as rb  # noqa: E402

PAIRED_SCHEMA_VERSION = "1.0"
ARMS = ("raw", "candidate")
OUTCOMES = ("success", "failure", "invalid", "unknown")
_BOOTSTRAP_RESAMPLES = 2000
_BOOTSTRAP_SEED = 20260930

SCRIPT_DIR = Path(__file__).resolve().parent
DEFAULT_TASKS_DIR = SCRIPT_DIR / "tasks" / "paired"
# ---------------------------------------------------------------------------
# observation arm (offline, real proxy)
# ---------------------------------------------------------------------------
#
# The observation path lives in `tokenfold-proxy`, not in the CLI the other arms
# drive, so it is reached through the proxy binary over a loopback echo upstream:
# no provider, no credential, no network egress. The arm is skipped -- reported,
# never failed -- when the proxy binary is not built, so a default offline run is
# exactly what it was before this flag existed.

DEFAULT_OBSERVATION_MIN_CONTENT_BYTES = 16
_PROXY_BIN: str | None = None
_PROXY_RESOLVED = False
_PROXY_NOTICE = ""


def _find_proxy() -> str | None:
    """Locate the proxy binary: TOKENFOLD_PROXY_BIN, then a local target build, then PATH.

    Mirrors `run_baselines._find_tokenfold` (newest build wins), so a freshly built
    debug proxy is not shadowed by a stale release one."""
    env = os.environ.get("TOKENFOLD_PROXY_BIN")
    if env and Path(env).is_file():
        return env
    root = Path(__file__).resolve().parent.parent
    exe = "tokenfold-proxy.exe" if os.name == "nt" else "tokenfold-proxy"
    candidates = [root / sub / exe for sub in ("target/release", "target/debug")]
    existing = [c for c in candidates if c.is_file()]
    if existing:
        return str(max(existing, key=lambda c: c.stat().st_mtime))
    return shutil.which("tokenfold-proxy")


def proxy_binary() -> str | None:
    global _PROXY_BIN, _PROXY_RESOLVED, _PROXY_NOTICE
    if not _PROXY_RESOLVED:
        _PROXY_RESOLVED = True
        _PROXY_BIN = _find_proxy()
        if _PROXY_BIN is None:
            _PROXY_NOTICE = (
                "observation arm skipped: tokenfold-proxy not found "
                "(build it and set TOKENFOLD_PROXY_BIN)"
            )
    return _PROXY_BIN


class _CallBudget:
    """Shared across both arms; readiness and retries also consume attempts."""
    def __init__(self, limit: int = 64):
        self.limit = limit
        self.used = 0
        self.lock = threading.Lock()

    def consume(self) -> None:
        with self.lock:
            if self.used >= self.limit:
                raise ValueError("live upstream call budget exhausted")
            self.used += 1


class _NoRelayRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ValueError("live upstream redirects are not authorized")


class _EchoUpstream(http.server.BaseHTTPRequestHandler):
    """Records the forwarded request body and answers it.

    With `server.relay` set, it forwards the captured request to that URL and
    returns the provider's response, so the harness sees the exact body the proxy
    sent *and* gets a real model answer through one code path -- a separate
    "capture proxy" would be a second thing that could drift from the first.
    """

    # HTTP/1.1 so the proxy's pooled upstream sockets stay valid between requests.
    # The stdlib default is HTTP/1.0, which closes the socket after every response;
    # the proxy then reused a dead socket and surfaced the provider request as an
    # intermittent "connection forcibly closed" 502.
    protocol_version = "HTTP/1.1"

    def do_POST(self):  # noqa: N802 - BaseHTTPRequestHandler API
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        self.server.captured = body  # type: ignore[attr-defined]
        relay = getattr(self.server, "relay", None)
        if relay is None:
            self._respond(200, b'{"ok":true}')
            return
        headers = {"Content-Type": "application/json"}
        authorization = self.headers.get("Authorization")
        if authorization:
            headers["Authorization"] = authorization
        try:
            request = urllib.request.Request(relay, data=body, headers=headers, method="POST")
            budget = getattr(self.server, "call_budget", None)
            if budget is not None:
                budget.consume()
                value = json.loads(body)
                value["max_tokens"] = min(value.get("max_tokens", 1024), 1024)
                request.data = json.dumps(value).encode("utf-8")
            opener = urllib.request.build_opener(_NoRelayRedirect())
            with opener.open(request, timeout=180) as response:
                self._respond(200, response.read())
        except Exception as exc:  # noqa: BLE001 - a relayed failure becomes a readable 502
            self._respond(502, str(exc).encode("utf-8", errors="replace")[:300])

    def _respond(self, status: int, payload: bytes) -> None:
        # Content-Length on every response is what makes HTTP/1.1 keep-alive legal
        # here; without it the client cannot tell where the body ends.
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, *args):  # noqa: D102 - silence the default access log
        pass


class _QuietThreadingHTTPServer(http.server.ThreadingHTTPServer):
    """ThreadingHTTPServer without the default per-connection traceback.

    A client that closes a keep-alive socket on the way out is normal here, not a
    server fault, and `socketserver` prints a full traceback for each one. The
    quiet override keeps a passing run's output readable.
    """

    def handle_error(self, request, client_address):  # noqa: D102 - stdlib API
        pass


def _start_echo(relay: str | None = None) -> http.server.ThreadingHTTPServer:
    """A loopback echo upstream, already serving and ready to accept the proxy."""
    # Threading server: each connection gets its own handler thread, so a pooled
    # connection being reused cannot block a second, concurrent one.
    server = _QuietThreadingHTTPServer(("127.0.0.1", 0), _EchoUpstream)
    server.captured = b""  # type: ignore[attr-defined]
    server.relay = relay  # type: ignore[attr-defined]
    # The default poll interval is used deliberately: a 0s poll busy-spins
    # `select` in this thread, which under the GIL starved the request handler
    # threads and showed up as an intermittent upstream connection reset.
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def _await_proxy(process: subprocess.Popen, timeout: float = 30.0) -> str:
    """Read the proxy's bound-address line and return its `http://host:port`.

    The proxy is started with `--bind 127.0.0.1:0`, so the port is assigned by the
    kernel and this line is the only way to learn it."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline and process.stderr is not None:
        line = process.stderr.readline().decode("utf-8", errors="replace")
        if "listening on " in line:
            bound = line.split("listening on ", 1)[1].split(" ->", 1)[0].strip()
            return f"http://{bound}"
        if process.poll() is not None:
            break
    raise ValueError("tokenfold-proxy never reported a bound address")


def _proxy_ready(url: str, headers: dict | None = None, model: str = "readiness", timeout: float = 20.0) -> None:
    """POST a minimal body until the proxy answers 200, or raise.

    The listening line proves the socket is bound; it does not prove the accept
    loop or the upstream is ready yet. Driving the fixtures against a proxy that
    has already answered one request is the difference between a race and a test.
    `model` must be real when the upstream is a real provider, so the probe is a
    valid request rather than an unknown-model 404.
    """
    body = json.dumps(
        {"model": model, "messages": [{"role": "user", "content": "ping"}]}
    ).encode("utf-8")
    deadline = time.monotonic() + timeout
    last: Exception | None = None
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(
                urllib.request.Request(
                    f"{url}/v1/chat/completions",
                    data=body,
                    headers={"Content-Type": "application/json", **(headers or {})},
                    method="POST",
                ),
                timeout=10,
            ) as response:
                if response.status == 200:
                    return
        except Exception as exc:  # noqa: BLE001 - a readiness poll expects failures
            last = exc
        time.sleep(0.05)
    raise ValueError(f"tokenfold-proxy never became ready: {last}")


def _drain_stderr(process: subprocess.Popen) -> list[str]:
    """Read the proxy's stderr into a list on a thread, so a blocked pipe or a
    crashed proxy becomes a diagnosable error instead of a bare 502."""
    lines: list[str] = []

    def run() -> None:
        if process.stderr is None:
            return
        for raw in process.stderr:
            lines.append(raw.decode("utf-8", errors="replace").rstrip())

    threading.Thread(target=run, daemon=True).start()
    return lines


def _post_once(url: str, body: bytes, timeout: float) -> None:
    """POST one body through the proxy and discard the (echoed) response."""
    request = urllib.request.Request(
        f"{url}/v1/chat/completions",
        data=body,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=timeout) as response:
        response.read()


def _post_resilient(url: str, body: bytes, timeout: float = 30.0, attempts: int = 3) -> None:
    """`_post_once`, retrying a loopback transport hiccup.

    The proxy keeps a pooled socket to this upstream, and on Windows a socket that
    is torn down between requests surfaces as one "connection forcibly closed"
    502. That is a property of the loopback test transport, not of the
    observation transform, which is deterministic: replaying the same body
    produces the same forwarded bytes. A persistent failure still raises, so a real
    defect is never hidden -- it just is not reported as one.
    """
    last: OSError | None = None
    for attempt in range(attempts):
        try:
            _post_once(url, body, timeout)
            return
        except OSError as exc:  # HTTPError is an OSError, so a 502 lands here too
            last = exc
            if attempt + 1 < attempts:
                time.sleep(0.1 * (attempt + 1))
    raise last  # type: ignore[misc]


def observation_envelope(task: dict) -> dict:
    """Wrap a fixture's source as the string content of one OpenAI tool result.

    The adapter only rewrites a `role: "tool"` result string, so the source must
    arrive as a JSON-encoded message in a valid transcript: one assistant turn
    requesting a single call, answered by one matching result. Built from the
    fixture rather than stored, so no second copy can drift out of sync."""
    source = task["source"]
    if source.lstrip()[:1] not in ("{", "["):
        raise ValueError(f"{task['id']}: source is not a JSON object/array result")
    return {
        "model": "offline-observation",
        "messages": [
            {"role": "user", "content": "inspect the tool output"},
            {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    {
                        "id": "call_0",
                        "type": "function",
                        "function": {"name": "inspect", "arguments": "{}"},
                    }
                ],
            },
            {"role": "tool", "tool_call_id": "call_0", "content": source},
        ],
    }


def observation_tool_content(payload: str) -> str:
    """The forwarded tool-result string, decoded; raises if it is missing."""
    value = json.loads(payload)
    for message in value["messages"]:
        if message.get("role") == "tool":
            return message["content"]
    raise ValueError("observation envelope lost its tool result")


def run_observation(tasks_dir: Path, min_content_bytes: int) -> tuple[list[dict], list[dict]]:
    """Drive the real proxy over each fixture's tool result; `([], [])` when unavailable.

    Each fixture is one attempt: the proxy rewrites the result string in place and
    the echo upstream hands the forwarded body back, so what is scored is exactly
    what the proxy would send to a provider. Returns `(records, rows)`."""
    binary = proxy_binary()
    if binary is None:
        print(f"# {_PROXY_NOTICE}", file=sys.stderr)
        return [], []

    upstream = _start_echo()

    proxy = subprocess.Popen(
        [
            binary,
            "--upstream",
            f"http://127.0.0.1:{upstream.server_address[1]}",
            "--bind",
            "127.0.0.1:0",
            "--insecure-upstream",
            "--observations",
            "--observation-min-content-bytes",
            str(min_content_bytes),
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )
    records: list[dict] = []
    rows: list[dict] = []
    try:
        proxy_url = _await_proxy(proxy)
        proxy_stderr = _drain_stderr(proxy)
        # Prove the proxy is serving before trusting the fixtures to it.
        _proxy_ready(proxy_url)
        for task in load_tasks(tasks_dir):
            baseline = json.dumps(observation_envelope(task), separators=(",", ":")).encode("utf-8")
            raw_tokens = rb.count_tokens(task["source"])
            try:
                _post_resilient(proxy_url, baseline)
            except OSError as exc:
                tail = "\n".join(proxy_stderr[-5:])
                raise ValueError(
                    f"observation arm request failed for {task['id']}: {exc}\n{tail}"
                ) from exc
            forwarded = upstream.captured  # type: ignore[attr-defined]
            content = observation_tool_content(forwarded.decode("utf-8"))
            # Honest naming: the gate is a size threshold, so a small fixture keeps its
            # baseline and is reported ineligible rather than scored as if compressed.
            label = "observation" if content != task["source"] else "observation-ineligible"

            answer = dummy_model_answer(recoverable_text(content), task["gold_answer"])
            succeeded = answer == task["gold_answer"]
            rows.append(
                {
                    "task": task["id"],
                    "family": task["family"],
                    "arm": "observation",
                    "target_ratio": None,
                    "raw_tokens": raw_tokens,
                    "candidate_tokens": rb.count_tokens(content),
                    **structural_check(task, content),
                    "downstream_success": succeeded,
                    "rewritten": content != task["source"],
                }
            )
            for arm, text, ok in (
                ("raw", task["source"], True),
                ("candidate", content, succeeded),
            ):
                records.append(
                    {
                        "schema_version": PAIRED_SCHEMA_VERSION,
                        "run_id": "offline-observation-proxy",
                        "task_id": f"{task['id']}@{label}",
                        "environment_snapshot": snapshot_hash(task["source"]),
                        "arm": arm,
                        "model": f"dummy-scorer-v0+{label}",
                        "policy_revision": label,
                        "seed": 0,
                        "attempt": 1,
                        "outcome": "success" if ok else "failure",
                        "evidence": [f"eval/tasks/paired/{task['id']}.json"],
                        "measurement": {
                            "local_before_tokens": raw_tokens,
                            "local_after_tokens": rb.count_tokens(text),
                            "provider_usage": None,
                        },
                    }
                )
    finally:
        proxy.terminate()
        try:
            proxy.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proxy.kill()
        upstream.shutdown()
        upstream.server_close()
    return records, rows


# ---------------------------------------------------------------------------
# live arm (opt-in; a real model behind the real proxy)
# ---------------------------------------------------------------------------
#
# `--live-arm` closes the last EP-04 item's offline gap by driving the real proxy
# to a REAL model and scoring the model's own answers. Ollama (local, free) and
# OpenRouter free models (`:free`) are supported; a paid OpenRouter model is
# refused even with `--live-allow-paid`: a priced runner with an enforced monetary cap is
# not yet available. The consent flag alone cannot enforce a spend cap.
#
# Both arms share one transcript per turn: `raw` forwards verbatim
# (`--no-compress`), `candidate` runs the observation path. Each arm forwards to
# its own relay, which records the exact forwarded body and passes it to the one
# real provider, so the recorded body and the model's answer cannot diverge.

LIVE_GOLD = "risk-registry-4471"
# The decoy in the report is risk-registry-1180, so the trailing digits alone
# discriminate the primary blocker from every other id in the result.
LIVE_GOLD_TAIL = "4471"
LIVE_ASK = (
    "Which risk-registry id does the fleet report name as the primary blocker? "
    "Reply with only that id."
)


def live_answered(answer: str | None) -> bool:
    """True when `answer` names the primary blocker.

    The model is asked for "only that id", but free models vary their format: the
    same model returns `risk-registry-4471` on one turn and bare `4471` on the next.
    Requiring the whole gold string scored that as a failure, which is a scoring
    artifact rather than lost content. Matching the distinctive id tail instead is
    still discriminating, because the report's decoy is risk-registry-1180.
    """
    if not answer:
        return False
    text = rb._ws_strip(answer)
    return LIVE_GOLD in text or LIVE_GOLD_TAIL in text


def _live_report() -> dict:
    """One tool result with the answer buried in repeated structure.

    Fifteen findings share the same keys so the columnar fold actually engages;
    exactly one carries the primary risk id and a second carries a decoy, so the
    answer is only right when the result's real content survived."""
    findings = [
        {
            "index": index,
            "label": f"check-{index:02d}",
            "severity": "high" if index % 3 == 0 else "info",
            "host": f"worker-{index:02d}",
            "detail": "routine sweep, no action required in this shard",
        }
        for index in range(15)
    ]
    findings[4]["detail"] = "primary risk-registry-4471 blocks rollout on this shard"
    findings[9]["detail"] = "decoy risk-registry-1180 recurs when the shard restarts"
    return {"tool": "inspect", "resource": "mesos", "report_id": "rep-9931", "findings": findings}


def _live_join_result(turn: int) -> str:
    return json.dumps({"status": "ok", "turn": turn, "note": "join acknowledged in this shard"})


def _live_base() -> list:
    """The fixed opening turns every live transcript starts from."""
    return [
        {"role": "system", "content": "Answer strictly from the tool output. Never invent ids."},
        {
            "role": "user",
            "content": "We are auditing the fleet. Inspect the mesos report and keep verifying as workers join.",
        },
    ]


def _append_turn(messages: list, index: int, result: str, question: str) -> list:
    """Append one tool group and the question. The new list shares the old prefix.

    Every turn returns a *new* list so a caller can choose what history the next turn
    is built from: the originals it still holds, or the observations the proxy committed.
    """
    call_id = f"call_{index}"
    return [
        *messages,
        {
            "role": "assistant",
            "content": None,
            "tool_calls": [
                {
                    "id": call_id,
                    "type": "function",
                    "function": {"name": "inspect", "arguments": json.dumps({"resource": "mesos"})},
                }
            ],
        },
        {"role": "tool", "tool_call_id": call_id, "content": result},
        {"role": "assistant", "content": f"acknowledged {call_id}"},
        {"role": "user", "content": question},
    ]


def _live_body(results: list[str], question: str) -> dict:
    """A truly append-only multi-turn chat body: one tool group per join turn.

    Each result is followed by the question, so the body for `results[:n]` is a
    message-for-message extension of the body for `results[:n-1]`. That is what
    makes the run a real append-only transcript -- and what lets
    `messages_prefix_stable` mean anything: a non-append-only layout (re-asking the
    question from a different final message each turn) would report instability
    caused by the harness rather than by the compression.
    """
    messages = _live_base()
    for index, result in enumerate(results):
        messages = _append_turn(messages, index, result, question)
    return {"messages": messages, "temperature": 0}


def _answer_text(response: dict) -> str | None:
    """The assistant text, or `None` when the provider returned no content string."""
    try:
        content = response["choices"][0]["message"]["content"]
    except (KeyError, IndexError, TypeError):
        return None
    if isinstance(content, str):
        return content
    if isinstance(content, list):  # some providers return content parts
        return " ".join(part.get("text", "") for part in content if isinstance(part, dict))
    return None


def _usage_of(response: dict) -> dict | None:
    usage = response.get("usage")
    return usage if isinstance(usage, dict) else None


def _post_chat(url: str, body: bytes, headers: dict, timeout: float, attempts: int = 1) -> dict:
    """POST a chat body and return the provider's JSON response.

    Retries a transport hiccup (see `_post_resilient`); a live provider call is
    not free, so the default is a single attempt and only the loopback-shaped
    callers opt into replaying.
    """
    request = urllib.request.Request(
        f"{url}/v1/chat/completions",
        data=body,
        headers={"Content-Type": "application/json", **headers},
        method="POST",
    )
    last: OSError | None = None
    for attempt in range(max(1, attempts)):
        try:
            with urllib.request.urlopen(request, timeout=timeout) as response:
                return json.loads(response.read())
        except urllib.error.HTTPError as exc:
            detail = exc.read().decode("utf-8", errors="replace")[:300]
            raise ValueError(f"live provider returned {exc.code}: {detail}") from exc
        except OSError as exc:
            last = exc
            if attempt + 1 < attempts:
                time.sleep(0.2 * (attempt + 1))
    raise last  # type: ignore[misc]


def cache_prefix_stable(previous_body: bytes, current_body: bytes) -> bool:
    """True when the current prompt keeps `previous_body` as a byte prefix.

    That is the property a provider prompt cache needs, and the one a proxy can
    only claim for turns it has already seen. The live arm reports it per turn
    instead of asserting it.""" 
    return current_body.startswith(previous_body)


def prompt_messages(body: bytes) -> list | None:
    """The `messages` array of a captured provider body, or `None` if unreadable."""
    try:
        return json.loads(body)["messages"]
    except (ValueError, KeyError, TypeError):
        return None


def messages_prefix_stable(previous: list, current: list) -> bool:
    """True when `current` extends `previous` message-for-message.

    This is the append-only property a provider prompt cache needs, measured on the
    parsed transcript. A raw byte-prefix cannot express it: a serialized array
    closes with `]}`, so a perfectly appended transcript is never a byte-prefix of
    the next document. Comparing the captured bodies message-by-message avoids
    reporting JSON framing as compression instability.
    """
    return current[: len(previous)] == previous


def _sum_usage(rows: list[dict], field: str, key: str) -> int | None:
    values = [
        row[field][key]
        for row in rows
        if isinstance(row.get(field), dict) and isinstance(row[field].get(key), int)
    ]
    return sum(values) if values else None


def _sum_cost(rows: list[dict]) -> float | None:
    costs = [
        float(row[field]["cost"])
        for row in rows
        for field in ("raw_usage", "candidate_usage")
        if isinstance(row.get(field), dict) and isinstance(row[field].get("cost"), (int, float))
    ]
    return round(sum(costs), 6) if costs else None


# A stable, opaque id for the live run's session. Not a secret: it exists only so the
# proxy can recognise a committed observation, and it is hashed before it is recorded.
_SESSION_HEADER = {"X-TokenFold-Session-Id": "live-paired-run"}


def paid_guard(upstream: str | None, model: str | None, allow_paid: bool) -> None:
    """Only reviewed local/free endpoints; a consent flag is not a spend cap."""
    if not upstream:
        raise ValueError("live upstream is required")
    parsed = urllib.parse.urlsplit(upstream)
    if parsed.username or parsed.password or parsed.query or parsed.fragment:
        raise ValueError("live upstream must not contain credentials, query or fragment")
    if parsed.scheme == "http" and parsed.hostname in ("localhost", "127.0.0.1", "::1"):
        return
    if parsed.scheme != "https" or parsed.hostname != "openrouter.ai" or parsed.port not in (None, 443):
        raise ValueError("only loopback HTTP or HTTPS openrouter.ai live upstreams are reviewed")
    if not model or not model.endswith(":free"):
        raise ValueError("paid live models require a priced runner with an enforced spend cap; --live-allow-paid is insufficient")



def run_live(
    model: str,
    upstream: str,
    api_key: str | None,
    min_content_bytes: int,
    turns: int,
    timeout: float = 180.0,
    session: bool = False,
    commit_once: bool = False,
) -> tuple[list[dict], list[dict], dict]:
    """Drive raw vs observation-compressed transcripts through a real model.

    Returns `(records, rows, summary)`. Both arms point at `upstream` (the real
    provider) through a per-arm relay; the model answers its own prompt, so a
    failure is a failure of what the observation path forwarded, never of a dummy
    scorer."""
    paid_guard(upstream, model, False)
    if not 1 <= turns <= 8 or not math.isfinite(timeout) or not 0 < timeout <= 180:
        raise ValueError("live run requires 1..8 turns and a finite timeout in (0, 180]")
    binary = proxy_binary()
    if binary is None:
        raise ValueError(_PROXY_NOTICE)
    if not model:
        raise ValueError("--live-model is required with --live-arm")
    if not upstream:
        raise ValueError("--live-upstream is required with --live-arm")
    if urllib.parse.urlsplit(upstream).hostname == "openrouter.ai" and not api_key:
        raise ValueError("an OpenRouter upstream needs --live-api-key-env to resolve a key")

    results = [json.dumps(_live_report()), *(_live_join_result(t) for t in range(1, turns))]
    headers = {"Authorization": f"Bearer {api_key}"} if api_key else {}
    relay = f"{upstream.rstrip('/')}/v1/chat/completions"
    raw_echo, cand_echo = _start_echo(relay), _start_echo(relay)
    call_budget = _CallBudget()
    raw_echo.call_budget = cand_echo.call_budget = call_budget

    def spawn(extra: list[str], echo: http.server.HTTPServer) -> subprocess.Popen:
        return subprocess.Popen(
            [
                binary,
                "--upstream", f"http://127.0.0.1:{echo.server_address[1]}",
                "--insecure-upstream", "--bind", "127.0.0.1:0",
                *extra,
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
        )

    # raw forwards verbatim; candidate runs only the observation path. Neither
    # compresses the whole body, so the only difference is the observation adapter.
    raw_proxy = spawn(["--no-compress"], raw_echo)
    cand_proxy = spawn(
        ["--observations", "--observation-min-content-bytes", str(min_content_bytes)], cand_echo
    )
    records: list[dict] = []
    rows: list[dict] = []
    try:
        raw_url = _await_proxy(raw_proxy)
        cand_url = _await_proxy(cand_proxy)
        _drain_stderr(raw_proxy)
        cand_log = _drain_stderr(cand_proxy)
        _proxy_ready(raw_url, headers, model)
        _proxy_ready(cand_url, headers, model)

        previous_messages: list | None = None
        # A commit-once host keeps what the proxy actually sent instead of re-sending the
        # original results. That is the contract the session ledger exists to make safe:
        # without it, replaying a committed observation folds a frame inside a frame.
        raw_messages = _live_base()
        cand_messages = _live_base()
        for turn in range(1, turns + 1):
            result = results[turn - 1]
            raw_messages = _append_turn(raw_messages, turn - 1, result, LIVE_ASK)
            cand_messages = _append_turn(cand_messages, turn - 1, result, LIVE_ASK)
            body = json.dumps(
                {"messages": raw_messages, "temperature": 0, "model": model}
            ).encode("utf-8")
            cand_body = json.dumps(
                {"messages": cand_messages, "temperature": 0, "model": model}
            ).encode("utf-8")
            try:
                raw_response = _post_chat(raw_url, body, headers, timeout, attempts=2)
                cand_response = _post_chat(
                    cand_url,
                    cand_body,
                    {**headers, **_SESSION_HEADER} if session else headers,
                    timeout,
                    attempts=2,
                )
            except OSError as exc:
                tail = "\n".join(cand_log[-5:])
                raise ValueError(f"live arm request failed on turn {turn}: {exc}\n{tail}") from exc

            raw_forwarded = raw_echo.captured  # type: ignore[attr-defined]
            cand_forwarded = cand_echo.captured  # type: ignore[attr-defined]
            raw_answer = _answer_text(raw_response)
            cand_answer = _answer_text(cand_response)
            raw_ok = live_answered(raw_answer)
            cand_ok = live_answered(cand_answer)
            raw_tokens = rb.count_tokens(raw_forwarded.decode("utf-8", errors="replace"))
            cand_tokens = rb.count_tokens(cand_forwarded.decode("utf-8", errors="replace"))
            prefix_stable = None
            if previous_messages is not None:
                current_messages = prompt_messages(cand_forwarded)
                prefix_stable = (
                    False
                    if current_messages is None
                    else messages_prefix_stable(previous_messages, current_messages)
                )
            previous_messages = prompt_messages(cand_forwarded)
            if commit_once:
                # Commit once: the next turn's history is what the proxy just sent. Without a
                # session header the proxy has no ledger to recognise it by, so this is the
                # negative control that shows what the ledger prevents.
                committed = prompt_messages(cand_forwarded)
                if committed is not None:
                    cand_messages = committed

            rows.append(
                {
                    "turn": turn,
                    "raw_tokens": raw_tokens,
                    "candidate_tokens": cand_tokens,
                    "raw_success": raw_ok,
                    "candidate_success": cand_ok,
                    "rewritten": cand_forwarded != raw_forwarded,
                    "prefix_stable": prefix_stable,
                    "raw_usage": _usage_of(raw_response),
                    "candidate_usage": _usage_of(cand_response),
                    "raw_answer": raw_answer,
                    "candidate_answer": cand_answer,
                }
            )
            for arm, ok, usage, tokens in (
                ("raw", raw_ok, _usage_of(raw_response), raw_tokens),
                ("candidate", cand_ok, _usage_of(cand_response), cand_tokens),
            ):
                records.append(
                    {
                        "schema_version": PAIRED_SCHEMA_VERSION,
                        "run_id": f"live-{model}",
                        "task_id": f"live_multi_turn_observation@turn{turn}",
                        "environment_snapshot": snapshot_hash(json.dumps(results[:turn])),
                        "arm": arm,
                        "model": model,
                        "policy_revision": (
                            "raw"
                            if arm == "raw"
                            else f"observation:live:min_content_bytes={min_content_bytes}"
                        ),
                        "seed": 0,
                        "attempt": 1,
                        "outcome": "success" if ok else "failure",
                        "evidence": [f"live:{upstream}", f"model:{model}", f"turn:{turn}"],
                        "measurement": {
                            "local_before_tokens": raw_tokens,
                            "local_after_tokens": tokens,
                            "provider_usage": usage,
                        },
                    }
                )
    finally:
        for process in (raw_proxy, cand_proxy):
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
        for echo in (raw_echo, cand_echo):
            echo.shutdown()
            echo.server_close()

    summary = {
        "model": model,
        "upstream": upstream,
        "turns": turns,
        "upstream_attempts": call_budget.used,
        "upstream_attempt_limit": call_budget.limit,
        "min_content_bytes": min_content_bytes,
        "session_commit_once": session,
        "commit_once": commit_once or session,
        "raw_success": sum(row["raw_success"] for row in rows),
        "candidate_success": sum(row["candidate_success"] for row in rows),
        "rewritten_turns": sum(row["rewritten"] for row in rows),
        "prefix_stable_turns": sum(row["prefix_stable"] is True for row in rows),
        "provider_prompt_tokens": {
            "raw": _sum_usage(rows, "raw_usage", "prompt_tokens"),
            "candidate": _sum_usage(rows, "candidate_usage", "prompt_tokens"),
        },
        "reported_cost": _sum_cost(rows),
    }
    return records, rows, summary


# ---------------------------------------------------------------------------
# record schema
# ---------------------------------------------------------------------------

_STRING_FIELDS = ("task_id", "environment_snapshot", "arm", "model", "outcome")
_REQUIRED_FIELDS = (
    "schema_version",
    "run_id",
    "task_id",
    "environment_snapshot",
    "arm",
    "model",
    "outcome",
    "evidence",
)


def snapshot_hash(payload: str) -> str:
    """`sha256:<hex>` of the exact environment bytes both arms must start from.

    A pair is only a pair when both arms hashed the *same* starting state, so
    the digest is part of the record and part of the pairing key -- not a
    decoration added at report time.
    """
    return "sha256:" + hashlib.sha256(payload.encode("utf-8")).hexdigest()


def validate_record(record: dict, where: str = "record") -> None:
    """Reject anything that could silently distort an aggregate. Raises ValueError.

    Fail-closed on purpose: a dropped or mislabelled record is an evaluation
    defect, so it must stop the run rather than shrink a denominator.
    """
    if not isinstance(record, dict):
        raise ValueError(f"{where}: must be a JSON object")
    for field in _REQUIRED_FIELDS:
        if field not in record:
            raise ValueError(f"{where}: missing required field {field!r}")
    version = record["schema_version"]
    if version != PAIRED_SCHEMA_VERSION:
        raise ValueError(
            f"{where}: unsupported schema_version {version!r} "
            f"(this harness reads {PAIRED_SCHEMA_VERSION})"
        )
    for field in _STRING_FIELDS:
        if not isinstance(record[field], str) or not record[field].strip():
            raise ValueError(f"{where}: {field} must be a non-empty string")
    if record["arm"] not in ARMS:
        raise ValueError(f"{where}: arm must be one of {ARMS}, got {record['arm']!r}")
    if record["outcome"] not in OUTCOMES:
        raise ValueError(f"{where}: outcome must be one of {OUTCOMES}, got {record['outcome']!r}")
    if not record["environment_snapshot"].startswith("sha256:"):
        raise ValueError(f"{where}: environment_snapshot must be a 'sha256:' digest")
    for field in ("seed", "attempt"):
        if not isinstance(record[field], int) or isinstance(record[field], bool):
            raise ValueError(f"{where}: {field} must be an integer")
    if record["attempt"] < 1:
        raise ValueError(f"{where}: attempt is 1-based, got {record['attempt']}")
    evidence = record["evidence"]
    if not isinstance(evidence, list) or not evidence or any(
        not isinstance(item, str) or not item.strip() for item in evidence
    ):
        raise ValueError(f"{where}: evidence must be a non-empty list of strings")
    measurement = record.get("measurement")
    if measurement is not None and not isinstance(measurement, dict):
        raise ValueError(f"{where}: measurement must be an object when present")
    if record["outcome"] == "invalid" and not record.get("invalid_reason"):
        raise ValueError(
            f"{where}: an invalid environment run must carry invalid_reason; an invalid run "
            "is reported, never relabelled a model failure"
        )


def load_records(path: Path) -> list[dict]:
    """Read newline-delimited paired records, validating each one."""
    records = []
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        if not line.strip():
            continue
        record = json.loads(line)
        validate_record(record, f"{path}:{number}")
        records.append(record)
    if not records:
        raise ValueError(f"{path}: no paired records")
    return records

# ---------------------------------------------------------------------------
# pairing + aggregation
# ---------------------------------------------------------------------------


def _pair_key(record: dict) -> tuple:
    return (
        record["task_id"],
        record["environment_snapshot"],
        record["seed"],
        record["attempt"],
    )


def pair_records(records: list[dict]) -> tuple[list[dict], list[dict], list[dict]]:
    """Group records into arm pairs. Returns `(pairs, unpaired, invalid)`.

    - A pair needs exactly one `raw` and one `candidate` record for the same
      task/snapshot/seed/attempt. Nothing is inferred and nothing is dropped: an
      arm without its partner is returned in `unpaired`, and an
      `invalid`/`unknown` outcome is returned in `invalid` rather than counted
      as a model failure.
    - A duplicated (key, arm) is a runner bug -- it would double-weight one
      observation -- so it is reported as unpaired rather than silently merged.
    """
    buckets: dict[tuple, dict[str, dict]] = {}
    unpaired: list[dict] = []
    invalid: list[dict] = []
    for record in records:
        if record["outcome"] in ("invalid", "unknown"):
            invalid.append(record)
            continue
        bucket = buckets.setdefault(_pair_key(record), {})
        arm = record["arm"]
        if arm in bucket:
            unpaired.append(
                {
                    "reason": "duplicate_arm",
                    "task_id": record["task_id"],
                    "arm": arm,
                    "seed": record["seed"],
                    "attempt": record["attempt"],
                }
            )
            continue
        bucket[arm] = record

    pairs = []
    for key in sorted(buckets):
        bucket = buckets[key]
        if set(bucket) != set(ARMS):
            unpaired.append(
                {
                    "reason": "missing_arm",
                    "task_id": key[0],
                    "arm": sorted(set(ARMS) - set(bucket))[0],
                    "seed": key[2],
                    "attempt": key[3],
                }
            )
            continue
        raw, candidate = bucket["raw"], bucket["candidate"]
        pairs.append(
            {
                "task_id": key[0],
                "environment_snapshot": key[1],
                "seed": key[2],
                "attempt": key[3],
                "raw": raw["outcome"] == "success",
                "candidate": candidate["outcome"] == "success",
            }
        )
    return pairs, unpaired, invalid


def _conditional_failure_rate(pairs: list[dict]) -> float | None:
    """CFR over raw successes. `None` (unavailable) when nothing succeeded raw."""
    raw_successes = sum(1 for p in pairs if p["raw"])
    if raw_successes == 0:
        return None
    regressions = sum(1 for p in pairs if p["raw"] and not p["candidate"])
    return round(regressions / raw_successes, 4)


def _success_delta(pairs: list[dict]) -> float | None:
    if not pairs:
        return None
    raw_rate = sum(1 for p in pairs if p["raw"]) / len(pairs)
    candidate_rate = sum(1 for p in pairs if p["candidate"]) / len(pairs)
    return round(candidate_rate - raw_rate, 4)

def _cluster_bootstrap_ci(pairs: list[dict], stat) -> list[float] | None:
    """Percentile interval from resampling *tasks*, not records.

    Repeating one task three times is three observations of one thing; treating
    them as independent would report an interval narrower than the data can
    support. Resampling whole tasks keeps the cluster as the resampling unit.
    Seeded, so the number in the artifact is reproducible.
    """
    by_task: dict[str, list[dict]] = {}
    for pair in pairs:
        by_task.setdefault(pair["task_id"], []).append(pair)
    tasks = sorted(by_task)
    if len(tasks) < 2:
        # One cluster has no between-task variance to estimate.
        return None
    rng = random.Random(_BOOTSTRAP_SEED)
    values = []
    for _ in range(_BOOTSTRAP_RESAMPLES):
        sample = [tasks[rng.randrange(len(tasks))] for _ in tasks]
        value = stat([pair for task in sample for pair in by_task[task]])
        if value is not None:
            values.append(value)
    if len(values) < 2:
        return None
    values.sort()
    low = values[int(0.025 * len(values))]
    high = values[min(len(values) - 1, int(0.975 * len(values)))]
    return [round(low, 4), round(high, 4)]


def aggregate(pairs: list[dict], unpaired: list[dict], invalid: list[dict]) -> dict:
    """The predeclared report. Every count, denominator and bound is published.

    All four paired outcomes are reported side by side on purpose: a candidate
    that fixes tasks the raw arm failed is real movement, and a summary showing
    only the regression count would hide exactly the offsetting gain the
    contract forbids cancelling out.
    """
    cells = {
        "raw_success_candidate_success": sum(1 for p in pairs if p["raw"] and p["candidate"]),
        "raw_success_candidate_failure": sum(1 for p in pairs if p["raw"] and not p["candidate"]),
        "raw_failure_candidate_success": sum(1 for p in pairs if not p["raw"] and p["candidate"]),
        "raw_failure_candidate_failure": sum(1 for p in pairs if not p["raw"] and not p["candidate"]),
    }
    raw_successes = cells["raw_success_candidate_success"] + cells["raw_success_candidate_failure"]
    candidate_successes = cells["raw_success_candidate_success"] + cells["raw_failure_candidate_success"]
    repeats = sum(1 for pair in pairs if sum(1 for q in pairs if q["task_id"] == pair["task_id"]) > 1)
    return {
        "paired_count": len(pairs),
        "task_count": len({p["task_id"] for p in pairs}),
        "repeated_attempt_count": repeats,
        "paired_outcomes": cells,
        "raw_successes": raw_successes,
        "candidate_successes": candidate_successes,
        "raw_success_rate": round(raw_successes / len(pairs), 4) if pairs else None,
        "candidate_success_rate": round(candidate_successes / len(pairs), 4) if pairs else None,
        "conditional_failure_rate": _conditional_failure_rate(pairs),
        "conditional_failure_rate_ci95": _cluster_bootstrap_ci(pairs, _conditional_failure_rate),
        "success_delta": _success_delta(pairs),
        "success_delta_ci95": _cluster_bootstrap_ci(pairs, _success_delta),
        "unpaired": unpaired,
        "invalid_count": len(invalid),
        "invalid": [
            {"task_id": r["task_id"], "arm": r["arm"], "reason": r.get("invalid_reason")}
            for r in invalid
        ],
    }


# ---------------------------------------------------------------------------
# resettable sandbox + deterministic dummy model
# ---------------------------------------------------------------------------


class Sandbox:
    """A resettable, per-attempt copy of a task's mutable environment.

    Both arms get their own instance created from the same snapshot bytes, so
    "identical starting state" is a property of the code path rather than a
    claim in the report. Writes are confined to the sandbox root: an escaping or
    absolute path is refused, so an evaluation run cannot reach a production
    path even if a future runner asks it to.
    """

    def __init__(self, snapshot: str):
        self._snapshot = snapshot
        self._root = Path(tempfile.mkdtemp(prefix="tokenfold_paired_"))
        self.reset()

    @property
    def root(self) -> Path:
        return self._root

    def reset(self) -> str:
        """Restore the pristine snapshot and return its digest."""
        shutil.rmtree(self._root, ignore_errors=True)
        self._root.mkdir(parents=True)
        (self._root / "environment.json").write_text(self._snapshot, encoding="utf-8")
        return snapshot_hash(self.read("environment.json"))

    def read(self, relative: str) -> str:
        return (self._root / self._confine(relative)).read_text(encoding="utf-8")

    def write(self, relative: str, value: str) -> Path:
        target = self._root / self._confine(relative)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(value, encoding="utf-8")
        return target

    def _confine(self, relative: str) -> Path:
        candidate = Path(relative)
        # `.anchor` catches `C:\...` and also the driveless rooted forms Windows
        # accepts (`\etc\passwd`, which is absolute on the current drive even
        # though `Path.is_absolute()` reports False for it).
        if candidate.anchor or ".." in candidate.parts:
            raise ValueError(f"sandbox refuses a path outside its root: {relative!r}")
        return candidate

    def close(self) -> None:
        shutil.rmtree(self._root, ignore_errors=True)


def dummy_model_answer(observation: str, gold_answer: str) -> str:
    """A deterministic stand-in for the downstream model, and nothing more.

    It answers from the text it can actually see: the gold answer when the
    observation still carries it (whitespace-insensitively, as the existing
    scorer does), otherwise a fixed non-answer. That makes "the candidate arm
    failed" a statement about what survived compression -- the property under
    test -- and says nothing about any real model's reasoning.
    """
    if rb._ws_strip(gold_answer) in rb._ws_strip(rb._logical_text(observation)):
        return gold_answer
    return "<unavailable>"

# ---------------------------------------------------------------------------
# offline fixture driver
# ---------------------------------------------------------------------------


def load_tasks(tasks_dir: Path, *, require_literal_answer: bool = True) -> list[dict]:
    """Load paired-task fixtures, failing closed on anything non-discriminating."""
    tasks = []
    seen = set()
    for path in sorted(tasks_dir.glob("*.json")):
        task = json.loads(path.read_text(encoding="utf-8"))
        for field in ("id", "family", "tier", "source", "query", "gold_answer", "critical_atoms"):
            if field not in task:
                raise ValueError(f"{path}: missing required field {field!r}")
        if task["id"] in seen:
            raise ValueError(f"{path}: duplicate task id {task['id']!r}")
        seen.add(task["id"])
        for atom in task["critical_atoms"]:
            if atom not in task["source"]:
                raise ValueError(f"{path}: critical atom {atom!r} is not grounded in source")
        if require_literal_answer and task["gold_answer"] not in task["source"]:
            raise ValueError(f"{path}: gold_answer must occur in source")
        tasks.append(task)
    if not tasks:
        raise ValueError(f"no paired-task fixtures found in {tasks_dir}")
    return tasks


def recoverable_text(payload: str) -> str:
    """Payload text plus every dropped item the host could retrieve.

    A `$tf_ref` marker is a *handle*: the content still exists in the retrieval
    store. Judging the candidate arm on marker text alone would score a working
    recovery path as data loss, so retrieval is resolved here; a retrieval that
    fails stays absent rather than being faked.
    """
    if "$tf_ref" not in payload:
        return payload
    try:
        hashes = rb._find_tf_ref_hashes(json.loads(payload))
    except ValueError:
        return payload
    recovered = []
    for digest in hashes:
        try:
            proc = subprocess.run(
                [
                    rb._TOKENFOLD_BIN,
                    "--config",
                    rb.isolated_retrieval_config(),
                    "retrieve",
                    digest,
                    "--retrieval-namespace",
                    "default",
                ],
                capture_output=True,
                timeout=30,
            )
        except (OSError, subprocess.SubprocessError):
            continue
        if proc.returncode == 0 and proc.stdout:
            recovered.append(proc.stdout.decode("utf-8", errors="replace"))
    return payload + "\n" + "\n".join(recovered)


def structural_check(task: dict, payload: str) -> dict:
    """Structural correctness, deliberately separate from downstream success.

    Structural = the payload is still well-formed JSON and still carries the
    atoms the task declares (inline or through a resolvable retrieval handle).
    Downstream = the dummy model could still answer. A payload can be perfectly
    well-formed and already useless, so conflating the two is how a broken
    transform gets reported as a quality regression -- or the reverse.
    """
    try:
        json.loads(payload)
        well_formed = True
    except ValueError:
        well_formed = False
    haystack = rb._ws_strip(recoverable_text(payload))
    surviving = [a for a in task["critical_atoms"] if rb._ws_strip(a) in haystack]
    return {
        "envelope_well_formed": well_formed,
        "critical_atoms": len(task["critical_atoms"]),
        "critical_atoms_surviving": len(surviving),
        "structure_ok": well_formed and len(surviving) == len(task["critical_atoms"]),
    }

def run_offline(
    tasks_dir: Path,
    ratios: list[float],
    observation: bool = False,
    observation_min_content_bytes: int = DEFAULT_OBSERVATION_MIN_CONTENT_BYTES,
) -> tuple[list[dict], list[dict]]:
    """Run every fixture through both arms. Returns `(records, rows)`.

    `raw` is the untouched source; `candidate` is the tokenfold CLI output at
    the requested ratio. Each arm runs in its own sandbox reset from the same
    snapshot bytes, and the two start digests are asserted equal rather than
    assumed -- that assertion is the whole point of the sandbox.

    With `observation`, a third candidate arm drives the real proxy over the same
    fixtures (see `run_observation`) and is appended to the run, so the proxy path
    is exercised rather than only unit-tested. It is additive: the CLI arms are
    produced first and are unchanged whether the proxy binary exists or not.
    """
    if rb._TOKENFOLD_BIN is None:
        raise ValueError(
            "tokenfold binary not found; --run-offline drives the real CLI "
            "(build it and set TOKENFOLD_BIN)"
        )
    records: list[dict] = []
    rows: list[dict] = []
    for task in load_tasks(tasks_dir):
        source = task["source"]
        digest = snapshot_hash(source)
        raw_tokens = rb.count_tokens(source)
        for ratio in ratios:
            budget = round(raw_tokens * ratio)
            for label, payload in (
                ("lossless", rb.compress_tokenfold(source, budget)),
                ("lossy", rb.compress_tokenfold_lossy(source, budget)),
            ):
                if payload is None:
                    continue
                raw_sandbox, candidate_sandbox = Sandbox(source), Sandbox(source)
                try:
                    starts = (raw_sandbox.reset(), candidate_sandbox.reset())
                finally:
                    raw_sandbox.close()
                    candidate_sandbox.close()
                assert starts == (digest, digest), f"{task['id']}: arms started from different snapshots"

                answer = dummy_model_answer(recoverable_text(payload), task["gold_answer"])
                succeeded = answer == task["gold_answer"]
                rows.append(
                    {
                        "task": task["id"],
                        "family": task["family"],
                        "arm": label,
                        "target_ratio": ratio,
                        "raw_tokens": raw_tokens,
                        "candidate_tokens": rb.count_tokens(payload),
                        **structural_check(task, payload),
                        "downstream_success": succeeded,
                    }
                )
                for arm, local_text, ok in (
                    ("raw", source, True),
                    ("candidate", answer, succeeded),
                ):
                    records.append(
                        {
                            "schema_version": PAIRED_SCHEMA_VERSION,
                            "run_id": "offline-dummy-model",
                            "task_id": f"{task['id']}@{label}@{ratio}",
                            "environment_snapshot": digest,
                            "arm": arm,
                            "model": f"dummy-scorer-v0+{label}@{ratio}",
                            "policy_revision": f"{label}@{ratio}",
                            "seed": 0,
                            "attempt": 1,
                            "outcome": "success" if ok else "failure",
                            "evidence": [f"eval/tasks/paired/{task['id']}.json"],
                            "measurement": {
                                "local_before_tokens": raw_tokens,
                                "local_after_tokens": rb.count_tokens(local_text),
                                "provider_usage": None,
                            },
                        }
                    )
    if observation:
        observation_records, observation_rows = run_observation(
            tasks_dir, observation_min_content_bytes
        )
        records.extend(observation_records)
        rows.extend(observation_rows)
    return records, rows

# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def gate_failures(report: dict, max_cfr: float) -> list[str]:
    """Assert the predeclared promotion thresholds. Returns failure strings."""
    failures = []
    if report["paired_count"] == 0:
        failures.append("no valid pairs: every record was unpaired or invalid")
    if report["unpaired"]:
        failures.append(f"{len(report['unpaired'])} unpaired arm(s): {report['unpaired'][:3]}")
    if report["invalid_count"]:
        failures.append(
            f"{report['invalid_count']} invalid/unknown environment run(s) must be resolved, "
            "not aggregated"
        )
    cfr = report["conditional_failure_rate"]
    if cfr is None:
        # Unavailable, not zero. With no raw successes the ceiling has no
        # denominator here, so it cannot be cleared.
        failures.append("conditional_failure_rate is unavailable (no raw successes)")
    elif cfr > max_cfr:
        failures.append(f"conditional_failure_rate {cfr} > max {max_cfr}")
    return failures


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Paired raw-vs-candidate run aggregation")
    parser.add_argument("--records", help="newline-delimited paired-run records to aggregate")
    parser.add_argument(
        "--tasks-dir",
        default=str(DEFAULT_TASKS_DIR),
        help="offline paired-task fixture directory (used with --run-offline)",
    )
    parser.add_argument(
        "--run-offline",
        action="store_true",
        help="drive the real tokenfold CLI over --tasks-dir and aggregate the result",
    )
    parser.add_argument(
        "--observation-arm",
        action="store_true",
        help="also drive the real tokenfold-proxy observation path over the same "
        "fixtures (needs the proxy binary; skipped, with a notice, when it is absent)",
    )
    parser.add_argument(
        "--observation-min-content-bytes",
        type=int,
        default=DEFAULT_OBSERVATION_MIN_CONTENT_BYTES,
        help="tool-result size floor passed to the proxy observation arm "
        "(below it a fixture keeps its baseline and is reported ineligible)",
    )
    parser.add_argument(
        "--ratios",
        default="0.5,0.1",
        help="comma-separated target retention ratios; 0.1 is where lossy pruning "
        "actually engages (at 0.5 the walk keeps every candidate, which is real "
        "measured behaviour but discriminates nothing)",
    )
    parser.add_argument("--gate", action="store_true", help="assert thresholds; exit non-zero on failure")
    parser.add_argument(
        "--max-cfr",
        type=float,
        default=0.005,
        help="predeclared conditional-failure ceiling for --gate (default 0.005)",
    )
    parser.add_argument(
        "--live-arm",
        action="store_true",
        help="drive raw vs observation-compressed transcripts through a real model "
        "(needs --live-model and --live-upstream)",
    )
    parser.add_argument("--live-model", help="model id for --live-arm")
    parser.add_argument(
        "--live-upstream",
        help="provider base URL for --live-arm, e.g. http://localhost:11434 (Ollama) "
        "or https://openrouter.ai/api (OpenRouter)",
    )
    parser.add_argument(
        "--live-api-key-env",
        default="OPENROUTER_API_KEY",
        help="environment variable holding the provider key (default OPENROUTER_API_KEY)",
    )
    parser.add_argument(
        "--live-allow-paid",
        action="store_true",
        help="reserved consent flag; paid models remain refused until a priced spend-capped runner exists",
    )
    parser.add_argument(
        "--live-session",
        action="store_true",
        help="commit the proxy's transformed observations and replay them next turn, under "
        "an X-TokenFold-Session-Id, exercising the proxy's session contract end to end",
    )
    parser.add_argument(
        "--live-commit-once",
        action="store_true",
        help="replay the proxy's committed observations next turn WITHOUT a session header: "
        "the negative control showing what the ledger prevents",
    )
    parser.add_argument(
        "--live-turns", type=int, default=3, help="multi-turn depth for --live-arm (default 3)"
    )
    parser.add_argument(
        "--live-min-content-bytes",
        type=int,
        default=DEFAULT_OBSERVATION_MIN_CONTENT_BYTES,
        help="tool-result size floor for the live observation arm",
    )
    return parser


def main(argv=None) -> int:
    args = build_parser().parse_args(argv)
    records: list[dict] = []
    rows = None
    live_summary = None
    try:
        if args.records:
            records, rows = load_records(Path(args.records)), None
        elif args.run_offline:
            ratios = [float(x) for x in args.ratios.split(",") if x.strip()]
            if not ratios or any(not math.isfinite(r) or r <= 0.0 or r > 1.0 for r in ratios):
                raise ValueError("ratios must be comma-separated numbers in (0, 1]")
            records, rows = run_offline(
                Path(args.tasks_dir),
                ratios,
                observation=args.observation_arm,
                observation_min_content_bytes=args.observation_min_content_bytes,
            )
        elif args.live_arm:
            paid_guard(args.live_upstream, args.live_model, args.live_allow_paid)
            records, rows, live_summary = run_live(
                args.live_model,
                args.live_upstream,
                os.environ.get(args.live_api_key_env),
                args.live_min_content_bytes,
                args.live_turns,
                session=args.live_session,
                commit_once=args.live_session or args.live_commit_once,
            )
        else:
            print("one of --records, --run-offline or --live-arm is required", file=sys.stderr)
            return 2
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"paired run failed: {error}", file=sys.stderr)
        return 2

    pairs, unpaired, invalid = pair_records(records)
    report = aggregate(pairs, unpaired, invalid)
    report["schema_version"] = PAIRED_SCHEMA_VERSION
    if rows is not None:
        report["live_rows" if args.live_arm else "offline_rows"] = rows
    if live_summary is not None:
        report["live"] = live_summary
    if args.gate:
        report["failures"] = gate_failures(report, args.max_cfr)
        report["gate"] = "pass" if not report["failures"] else "fail"
    print(json.dumps(report, indent=2))
    return 1 if args.gate and report["failures"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
