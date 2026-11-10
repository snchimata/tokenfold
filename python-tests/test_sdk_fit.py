"""Replacement SDK contracts, exercised against an installed wheel."""
import json
import pytest
import tokenfold as tf


def test_native_fit_and_immutable_receipt():
    assert hasattr(tf, "fit"), "replacement fit API must be installed"
    result = tf.fit(" unchanged  text\n", target_tokens=20)
    assert result.require_met() == " unchanged  text\n"
    assert result.receipt.kind == "tokenfold.fit"
    assert result.receipt.accounting.final_basis == "exact"
    assert isinstance(result.groups_kept, tuple)
    assert tf.Receipt.from_json(result.receipt.to_json()).to_json() == result.receipt.to_json()
    with pytest.raises((AttributeError, TypeError)):
        result.receipt.target_tokens = 0


def test_data_and_full_provider_snapshot():
    value = {"z": [1, 2], "a": True}
    assert tf.fit(tf.Data(value), target_tokens=100).payload == '{"z":[1,2],"a":true}'
    payload = {"model": "gpt-4o-2024-08-06", "messages": [{"role": "user", "content": "Hello"}], "temperature": 0}
    result = tf.fit(tf.Request.openai(payload), target_tokens=100)
    assert result.payload == payload
    assert result.budget_status == "indeterminate"
    assert result.final_tokens is None
    result.payload["messages"].clear()
    assert result.payload == payload
    assert json.loads(json.dumps(result.payload)) == payload


def test_provider_profile_and_checked_access():
    request = tf.Request.openai({"model": "gpt-4o-2024-08-06", "messages": [{"role": "user", "content": "Hello"}]})
    with tf.Tokenfold(counter=tf.ProviderCounter.openai(model="gpt-4o-2024-08-06")) as client:
        strict = client.fit(request, target_tokens=100)
        with pytest.raises(tf.BudgetIndeterminateError) as error:
            strict.require_fit()
        assert error.value.receipt is strict.receipt
        best = client.fit(request, target_tokens=100, budget_mode="best_effort")
        assert best.estimated_fit is True and best.budget_status == "indeterminate"
        assert best.require_fit() == request.payload
        with pytest.raises(tf.BudgetIndeterminateError):
            best.require_met()

@pytest.mark.parametrize("value", [True, -1, 1.2, 2**53, "20", None])
def test_invalid_target_is_typed(value):
    with pytest.raises(tf.InvalidInputError):
        tf.fit("text", target_tokens=value)

@pytest.mark.parametrize("value", [{1: "bad"}, {"x": float("nan")}, {"x": 2**53}, b"bytes", object(), {"x": "\ud800"}])
def test_invalid_json_never_coerces(value):
    with pytest.raises(tf.InvalidInputError):
        tf.Data(value)


def test_cycles_unicode_depth_bounds_and_number_representation():
    value = []
    value.append(value)
    with pytest.raises(tf.InvalidInputError):
        tf.Data(value)
    nested = 1
    for _ in range(64):
        nested = [nested]
    assert tf.fit(tf.Data(nested), target_tokens=1000).budget_status == "met"
    with pytest.raises(tf.InvalidInputError):
        tf.Data([nested])
    # Rust canonicalizes typed JSON numbers once for both language bindings.
    assert tf.fit(tf.Data([1.0, -0.0]), target_tokens=100).payload == "[1,0]"
    text = "x" * (2 * 1024 * 1024 - 2)
    class Counter:
        id = "custom:boundary:v1"
        def count(self, request): return tf.TokenCount(1)
    assert len(tf.fit(tf.Data(text), target_tokens=2**53 - 1, counter=Counter()).payload.encode()) == 2 * 1024 * 1024
    with pytest.raises(tf.InvalidInputError):
        tf.fit(tf.Data(text + "x"), target_tokens=2**53 - 1)


def test_closed_config_and_disabled_runtime():
    with pytest.raises(tf.ConfigError): tf.Tokenfold(max_pending_scores=True)
    with pytest.raises(tf.ConfigError): tf.Tokenfold(score_wait_timeout_ms=0)
    with pytest.raises(tf.ConfigError): tf.SelectRuntime("approval.json")
    with pytest.raises(tf.ConfigError): tf.fit("text", target_tokens=20, budget_mode="unknown")
    client = tf.Tokenfold()
    with pytest.raises(tf.ConfigError): client.warmup()
    assert client.fit(tf.Context("query", (tf.Group("a", "content"),)), target_tokens=100).reason is None
    client.close(); client.close()
    with pytest.raises(tf.ClosedError): client.fit("text", target_tokens=20)
    with pytest.raises(tf.ClosedError): client.warmup()
    token = tf.CancellationToken(); token.cancel()
    with tf.Tokenfold() as client, pytest.raises(tf.CancelledError):
        client.fit("text", target_tokens=20, cancel=token)


@pytest.mark.parametrize("pathlike", [False, True])
def test_select_config_surrogate_is_typed(pathlike):
    class Path:
        def __fspath__(self): return "bad\ud800"
    with pytest.raises(tf.ConfigError, match="invalid Select configuration path"):
        tf.SelectRuntime(Path() if pathlike else "bad\ud800", experimental=True)


def test_counter_errors_are_sanitized_and_counts_validated():
    class Counter:
        id = "custom:test:v1"
        def count(self, request): raise tf.ConfigError("private callback data")
    with pytest.raises(tf.AccountingError) as error:
        tf.fit("text", target_tokens=20, counter=Counter())
    assert "private" not in str(error.value)
    for tokens, basis in [(True, "exact"), (-1, "estimated"), (0, "unavailable"), (None, "exact"), (2**53, "exact")]:
        with pytest.raises(tf.AccountingError): tf.TokenCount(tokens, basis)


def test_complete_request_counter_and_immutable_snapshots():
    seen = []
    class Counter:
        id = "custom:full-request:v1"
        def count(self, request):
            assert request.scope == "provider_request"
            payload = request.payload
            assert payload["tools"] == [{"name": "lookup"}]
            seen.append(payload)
            payload["messages"].clear()
            return tf.TokenCount(12, "exact")
    payload = {"model": "model", "messages": [{"role": "user", "content": "hi"}], "tools": [{"name": "lookup"}]}
    request = tf.Request.openai(payload)
    payload["messages"][0]["content"] = "mutated"
    result = tf.fit(request, target_tokens=12, counter=Counter())
    assert result.require_met()["messages"][0]["content"] == "hi"
    assert result.final_tokens == 12 and len(seen) == 1


def test_counter_concurrency_has_no_client_lock():
    import concurrent.futures, threading
    barrier = threading.Barrier(2)
    class Counter:
        id = "custom:concurrent:v1"
        def count(self, request):
            barrier.wait(timeout=5)
            return tf.TokenCount(len(request.payload), "exact")
    with tf.Tokenfold(counter=Counter()) as client, concurrent.futures.ThreadPoolExecutor(2) as pool:
        results = list(pool.map(lambda _: client.fit("hello", target_tokens=10), range(2)))
    assert [r.payload for r in results] == ["hello", "hello"]


def test_async_loop_responsive_and_cancel_waits_for_callback():
    import asyncio, threading
    entered, release = threading.Event(), threading.Event()
    class Counter:
        id = "custom:blocked:v1"
        def count(self, request):
            entered.set()
            assert release.wait(5)
            return tf.TokenCount(1)
    async def check():
        async with tf.Tokenfold(counter=Counter()) as client:
            task = asyncio.create_task(client.afit("hello", target_tokens=10))
            while not entered.is_set(): await asyncio.sleep(.002)
            task.cancel()
            await asyncio.sleep(.02)
            assert not task.done(), "cancellation must await in-flight native/host cleanup"
            release.set()
            with pytest.raises(asyncio.CancelledError): await task
            assert (await client.afit("hello", target_tokens=10)).budget_status == "met"
    asyncio.run(check())


def test_receipt_reader_rejects_duplicates_and_other_family():
    receipt = tf.fit("text", target_tokens=20).receipt.to_json()
    for source in (receipt.replace('"schema_version":"1.0"', '"schema_version":"2.0"'), receipt.replace('"kind":', '"kind":"tokenfold.fit","kind":'), '{"schema_version":"2.0"}'):
        with pytest.raises(tf.InvalidInputError): tf.Receipt.from_json(source)

@pytest.fixture
def runtime(tmp_path):
    import hashlib, sys
    from pathlib import Path
    names = ["__init__.py", "__main__.py", "ettin.py", "integrity.py", "protocol.py", "provision.py", "worker.py"]
    source = tmp_path / "src" / "tokenfold_select_runtime"
    source.mkdir(parents=True)
    digest = bytearray()
    fake = (Path(__file__).parents[1] / "crates/tokenfold-adapters/tests/resident_fake.py").read_bytes()
    for name in names:
        raw = fake if name == "__main__.py" else b"# fixture\n"
        (source / name).write_bytes(raw)
        digest.extend(name.encode() + b"\0" + raw + b"\0")
    (tmp_path / "mode").write_text("normal")
    manifest = json.dumps({"revision": "a" * 64}).encode()
    (tmp_path / "ettin-manifest.json").write_bytes(manifest)
    python = Path(sys.executable).absolute()
    approval = dict(python_executable=str(python), python_sha256=hashlib.sha256(python.read_bytes()).hexdigest(), runtime_source=str(source), runtime_source_sha256=hashlib.sha256(digest).hexdigest(), runtime_version="0.6.0", bundle=str(tmp_path), manifest_sha256=hashlib.sha256(manifest).hexdigest(), revision="a"*64, device="cpu", precision="fp32", approved_by="test", scratch_root=str(tmp_path), startup_timeout_ms=3000, inference_timeout_ms=5000, cleanup_timeout_ms=100)
    config = tmp_path / "approval.json"
    config.write_text(json.dumps(approval))
    yield tf.SelectRuntime(config, experimental=True), tmp_path
    for line in (tmp_path / "pids").read_text().splitlines() if (tmp_path / "pids").exists() else []:
        pid, birth = map(int, line.split(":"))
        if sys.platform == "win32":
            import ctypes
            kernel = ctypes.windll.kernel32
            kernel.OpenProcess.restype = ctypes.c_void_p
            handle = kernel.OpenProcess(0x1000, False, pid)
            if handle:
                created, exited, krn, user = (ctypes.c_uint64() for _ in range(4))
                status = ctypes.c_ulong()
                ok = kernel.GetProcessTimes(ctypes.c_void_p(handle), *(ctypes.byref(v) for v in (created, exited, krn, user)))
                kernel.GetExitCodeProcess(ctypes.c_void_p(handle), ctypes.byref(status))
                kernel.CloseHandle(ctypes.c_void_p(handle))
                assert not (ok and created.value == birth and status.value == 259), "owned child not reaped"
        else:
            import os
            with pytest.raises(ProcessLookupError): os.kill(pid, 0)


class LengthCounter:
    id = "custom:length:v1"
    def __init__(self): self.calls = []
    def count(self, request):
        self.calls.append(request.payload)
        return tf.TokenCount(len(request.payload))


def optional_context(query="query"):
    return tf.Context(query, (tf.Group("a", "alpha " * 40), tf.Group("b", "beta " * 40)))


def test_resident_fake_selection_cache_consent_and_no_base_start(runtime):
    config, root = runtime
    counter = LengthCounter()
    with tf.Tokenfold(counter=counter, select=config) as client:
        assert client.fit("fits", target_tokens=20).budget_status == "met"
        assert not (root / "pids").exists()
        assert client.fit(optional_context(), target_tokens=20).reason == "selection_disabled"
        assert not (root / "pids").exists()
        counter.calls.clear()
        result = client.fit(optional_context(), target_tokens=20, allow_selection=True)
        assert result.payload == "query" and result.selection_applied
        assert result.groups_dropped == ("a", "b")
        assert result.receipt.scorer_attempted and result.receipt.scorer_succeeded
        assert result.receipt.scorer.qualification == "unqualified"
        assert len(counter.calls) == 2, "required-only candidate reused, not recounted"
        client.warmup(); client.warmup()
        assert len((root / "pids").read_text().splitlines()) == 1


def test_scorer_failure_checked_error_retains_baseline(runtime):
    config, root = runtime
    with tf.Tokenfold(counter=LengthCounter(), select=config) as client:
        result = client.fit(optional_context("nan"), target_tokens=20, allow_selection=True)
        assert result.reason == "scorer_response_invalid"
        assert result.groups_kept == ("a", "b")
        assert result.receipt.scorer_attempted and not result.receipt.scorer_succeeded
        with pytest.raises(tf.BudgetUnmetError) as error: result.require_met()
        assert error.value.receipt is result.receipt


async def wait_active(root):
    import asyncio, time
    until = time.monotonic() + 5
    while not (root / "active").exists():
        assert time.monotonic() < until, "worker did not start"
        await asyncio.sleep(.002)


def test_async_ticket_wait_cancellation_busy_and_core_executor_independence(runtime):
    import asyncio, concurrent.futures
    config, root = runtime
    async def check():
        loop = asyncio.get_running_loop()
        # A single executor slot proves scorer wait does not occupy it.
        loop.set_default_executor(concurrent.futures.ThreadPoolExecutor(1))
        async with tf.Tokenfold(counter=LengthCounter(), select=config, max_pending_scores=1) as client:
            active = asyncio.create_task(client.afit(optional_context("hold"), target_tokens=20, allow_selection=True))
            await wait_active(root)
            waiting_token = tf.CancellationToken()
            waiting = asyncio.create_task(client.afit(optional_context(), target_tokens=20, allow_selection=True, cancel=waiting_token))
            await asyncio.sleep(.05)
            with pytest.raises(tf.BusyError):
                await client.afit(optional_context(), target_tokens=20, allow_selection=True)
            assert (await asyncio.wait_for(client.afit("Core fits", target_tokens=20), 2)).budget_status == "met"
            waiting_token.cancel()
            with pytest.raises(tf.CancelledError): await waiting
            assert not active.done() and (root / "active").read_text() == "hold"
            # Cancellation of a running inference reaps its child before completion.
            active.cancel()
            with pytest.raises(asyncio.CancelledError): await active
            assert (await client.afit(optional_context(), target_tokens=20, allow_selection=True)).selection_applied
    asyncio.run(check())


@pytest.mark.parametrize("mode", ["queue_timeout", "close"])
def test_queue_timeout_and_close_wake_waiters(runtime, mode):
    import asyncio, threading
    config, root = runtime
    submitted = threading.Event()
    class ObservedAdmission(tf.Tokenfold):
        def _step(self, session, action=None):
            reply = super()._step(session, action)
            if reply["action"] == "score": submitted.set()
            return reply
    async def check():
        client = ObservedAdmission(counter=LengthCounter(), select=config, score_wait_timeout_ms=30 if mode == "queue_timeout" else 5000)
        active = asyncio.create_task(client.afit(optional_context("hold"), target_tokens=20, allow_selection=True))
        await wait_active(root)
        if mode == "queue_timeout":
            timed = await client.afit(optional_context(), target_tokens=20, allow_selection=True)
            assert timed.reason == "scorer_queue_timeout" and not timed.receipt.scorer_attempted
        else:
            submitted.clear()
            waiting = asyncio.create_task(client.afit(optional_context(), target_tokens=20, allow_selection=True))
            async with asyncio.timeout(10):
                while not submitted.is_set(): await asyncio.sleep(0)
            await asyncio.to_thread(client.close)
            with pytest.raises(tf.ClosedError): await waiting
        await asyncio.to_thread(client.close)
        with pytest.raises((tf.CancelledError, tf.ClosedError)): await active
        client.close()
    asyncio.run(check())


def test_native_tokenizer_failure_is_sanitized_accounting():
    with pytest.raises(tf.AccountingError) as error:
        tf.fit("x" * (2 * 1024 * 1024), target_tokens=2**53 - 1)
    assert str(error.value) == "accounting failed"


def test_provider_json_numeric_canonicalization_and_input_untouched():
    payload = {"model": "gpt-4o-2024-08-06", "messages": [{"role": "user", "content": "1.0 -0.0"}], "temperature": 1.0, "seed": -0.0}
    result = tf.fit(tf.Request.openai(payload), target_tokens=100)
    assert type(result.payload["temperature"]) is int and result.payload["temperature"] == 1
    assert type(result.payload["seed"]) is int and result.payload["seed"] == 0
    assert result.payload["messages"][0]["content"] == "1.0 -0.0"
    assert type(payload["temperature"]) is float


def test_custom_id_drift_and_invalid_configuration_are_typed():
    class Counter:
        id = "custom:before:v1"
        def count(self, request):
            self.id = "custom:after:v1"
            return tf.TokenCount(1)
    with pytest.raises(tf.AccountingError): tf.fit("text", target_tokens=10, counter=Counter())
    for make in (lambda: tf.EncodingCounter(None), lambda: tf.ProviderCounter.openai(model=1)):
        with pytest.raises(tf.ConfigError): make()


@pytest.mark.parametrize("value", ["\ud800", "profile:\udfff"])
@pytest.mark.parametrize("kind", ["custom", "encoding", "provider"])
def test_counter_unicode_errors_are_typed(value, kind):
    class Counter:
        id = value
        def count(self, request):
            raise AssertionError("invalid ID reached callback")
    with pytest.raises(tf.AccountingError if kind == "custom" else tf.ConfigError):
        if kind == "custom":
            tf.fit("text", target_tokens=10, counter=Counter())
        elif kind == "encoding":
            tf.EncodingCounter(value)
        else:
            tf.ProviderCounter.openai(model=value)


@pytest.mark.parametrize("mode", ["cancel", "close"])
def test_count_accessor_cancellation_prevents_callback_start(mode):
    token = tf.CancellationToken()
    calls = []
    class Counter:
        id = "custom:accessor:v1"
        @property
        def count(self):
            token.cancel() if mode == "cancel" else client.close()
            def callback(request):
                calls.append(request)
                return tf.TokenCount(1)
            return callback
    with tf.Tokenfold(counter=Counter()) as client:
        with pytest.raises(tf.CancelledError):
            client.fit("text", target_tokens=10, cancel=token)
    assert calls == [], "cancelled accessor admitted a new callback"


def test_builtin_counters_stay_native_not_host_callbacks(monkeypatch):
    def forbidden(*args): raise AssertionError("built-in count crossed host boundary")
    monkeypatch.setattr(tf.EncodingCounter, "count", forbidden)
    monkeypatch.setattr(tf.ProviderCounter, "count", forbidden)
    assert tf.fit("text", target_tokens=20).require_met() == "text"
    request = tf.Request.openai({"model": "gpt-4o-2024-08-06", "messages": [{"role": "user", "content": "hi"}]})
    result = tf.fit(request, target_tokens=100, counter=tf.ProviderCounter.openai(model="gpt-4o-2024-08-06"), budget_mode="best_effort")
    assert result.budget_status == "indeterminate" and result.require_fit() == request.payload


def test_estimated_selection_and_preflight_basis_drift(runtime):
    config, root = runtime
    class Estimated(LengthCounter):
        id = "custom:estimated:v1"
        def count(self, request): return tf.TokenCount(len(request.payload), "estimated")
    with tf.Tokenfold(counter=Estimated(), select=config) as client:
        strict = client.fit(optional_context(), target_tokens=20, allow_selection=True)
        assert strict.reason == "unknown_accounting" and not (root / "pids").exists()
        best = client.fit(optional_context(), target_tokens=20, allow_selection=True, budget_mode="best_effort")
        assert best.selection_applied and best.budget_status == "indeterminate"
        assert best.require_fit() == "query"
        assert best.receipt.warnings[0].code == "estimated_accounting"
        with pytest.raises(tf.BudgetIndeterminateError): best.require_met()
    class Drift(LengthCounter):
        id = "custom:drift:v1"
        def count(self, request):
            return tf.TokenCount(len(request.payload), "exact" if "alpha" in request.payload else "estimated")
    with tf.Tokenfold(counter=Drift(), select=config) as client:
        result = client.fit(optional_context(), target_tokens=20, allow_selection=True)
        assert result.reason == "unknown_accounting" and result.budget_status == "unmet"
        assert result.groups_kept == ("a", "b") and not result.receipt.scorer_attempted


def test_provider_protection_intervals_and_secrets_precede_counter():
    payload = {"model": "model", "messages": [{"role": "system", "content": "instruction"}, {"role": "assistant", "content": "old"}, {"role": "user", "content": "active"}]}
    for indices in ((0,), (2,), (1, 3), (2, 1)):
        request = tf.Request.openai(payload, groups=[tf.MessageGroup("bad", indices)])
        with pytest.raises(tf.InvalidInputError): tf.fit(request, target_tokens=10)
    class Counter:
        id = "custom:safety:v1"
        def count(self, request): raise AssertionError("unsafe input reached counter")
    with pytest.raises(tf.SafetyError):
        tf.fit("Authorization: Bearer " + "x" * 32, target_tokens=100, counter=Counter())


def test_shared_numeric_fixture():
    from pathlib import Path
    fixture = json.loads((Path(__file__).parents[1] / "tests/fixtures/sdk-fit/numbers.json").read_text())
    results = [tf.fit(tf.Data(value), target_tokens=100) for value in fixture["equivalent_inputs"]]
    assert [result.payload for result in results] == [fixture["canonical_json"]] * 2
    assert results[0].original_tokens == results[1].original_tokens
    for value in fixture["unsafe_values"]:
        with pytest.raises(tf.InvalidInputError): tf.Data(value)


def paused_count_client(counter, produced, release):
    """Pause only after a real native Count exists, before the host callback admission."""
    class PausedHandoff(tf.Tokenfold):
        def _step(self, session, action=None):
            reply = super()._step(session, action)
            if action is None and reply["action"] == "count" and not produced.is_set():
                produced.set()
                assert release.wait(10), "Count handoff was not released"
            return reply
    return PausedHandoff(counter=counter)


def test_cancelled_count_handoff_never_starts_sync_custom_callback():
    import concurrent.futures, threading
    produced, release = threading.Event(), threading.Event()
    counter, token = LengthCounter(), tf.CancellationToken()
    with paused_count_client(counter, produced, release) as client, concurrent.futures.ThreadPoolExecutor(1) as pool:
        future = pool.submit(client.fit, "hello", target_tokens=10, cancel=token)
        try:
            assert produced.wait(10), "native Count was not produced"
            token.cancel()
        finally:
            release.set()
        with pytest.raises(tf.CancelledError): future.result(timeout=10)
    assert counter.calls == [], "cancelled handoff admitted a new custom callback"


@pytest.mark.parametrize("mode", ["repeated_task_cancel", "client_close"])
def test_cancelled_count_handoff_never_starts_async_custom_callback(mode):
    import asyncio, threading
    produced, release = threading.Event(), threading.Event()
    counter, token = LengthCounter(), tf.CancellationToken()
    async def check():
        async with paused_count_client(counter, produced, release) as client:
            task = asyncio.create_task(client.afit("hello", target_tokens=10, cancel=token))
            try:
                async with asyncio.timeout(10):
                    while not produced.is_set(): await asyncio.sleep(0)
                if mode == "repeated_task_cancel":
                    task.cancel()
                    async with asyncio.timeout(10):
                        while not token.cancelled: await asyncio.sleep(0)
                    task.cancel()
                    await asyncio.sleep(0)
                    assert not task.done(), "repeated cancellation abandoned the shielded runner"
                    expected = asyncio.CancelledError
                else:
                    await asyncio.to_thread(client.close)
                    assert not token.cancelled, "close must be observed through the native session, not only the external token"
                    expected = tf.CancelledError
            finally:
                release.set()
            with pytest.raises(expected): await task
        assert counter.calls == [], "cancelled handoff admitted a new custom callback"
    asyncio.run(check())
