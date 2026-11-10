"""Lifecycle and host callbacks; native Rust owns every fitting decision."""
from __future__ import annotations
import asyncio
from dataclasses import dataclass
import json
from os import PathLike, fspath
from typing import overload
from . import _native
from .accounting import EncodingCounter, ProviderCounter, TokenCount, TokenCounter
from .types import (Data, Group, Context, Request, MessageGroup, FitResult, Receipt,
                    JsonObject, BudgetMode, RenderedRequest, _json)
from .errors import ConfigError, InvalidInputError, AccountingError

CancellationToken = _native.CancellationToken
Input = str | Data | Context | Request

@dataclass(frozen=True, slots=True)
class SelectRuntime:
    config: str | PathLike[str]
    experimental: bool = False
    def __post_init__(self) -> None:
        if self.experimental is not True:
            raise ConfigError("Select requires explicit experimental=True")
        try:
            path = fspath(self.config)
            if type(path) is not str:
                raise TypeError
            object.__setattr__(self, "config", path)
        except Exception:
            raise ConfigError("invalid Select configuration path") from None


def _integer(value: object, lo: int, hi: int, error: type[Exception]) -> None:
    if type(value) is not int or not lo <= value <= hi:
        raise error("invalid integer option")


def _source(request: Input) -> str:
    def content(value: str | Data) -> JsonObject:
        if type(value) is str:
            return {"kind": "text", "text": value}
        if isinstance(value, Data):
            return {"kind": "json_data", "value": value.value, "representation": value.representation}
        raise InvalidInputError("invalid group content")
    def group(value: Group) -> JsonObject:
        if not isinstance(value, Group) or type(value.id) is not str or type(value.required) is not bool:
            raise InvalidInputError("invalid group")
        return {"id": value.id, "content": content(value.content), "required": value.required}
    def message_group(value: MessageGroup) -> JsonObject:
        if not isinstance(value, MessageGroup) or type(value.id) is not str or type(value.required) is not bool:
            raise InvalidInputError("invalid message group")
        for index in value.indices:
            _integer(index, 0, 2**53 - 1, InvalidInputError)
        return {"id": value.id, "indices": list(value.indices), "required": value.required}
    if isinstance(request, (str, Data)):
        value = content(request)
    elif isinstance(request, Context):
        if type(request.query) is not str:
            raise InvalidInputError("invalid context query")
        value = {"kind": "context", "query": request.query, "groups": [group(g) for g in request.groups]}
    elif isinstance(request, Request):
        if request.query is not None and type(request.query) is not str:
            raise InvalidInputError("invalid scoring query")
        value = {"kind": request.format, "payload": request.payload, "query": request.query, "groups": [message_group(g) for g in request.groups]}
    else:
        raise InvalidInputError("expected text, Data, Context or Request")
    try:
        # Input values were validated individually; do not charge this private envelope's depth.
        source = json.dumps(value, ensure_ascii=False, allow_nan=False, separators=(",", ":"))
        source.encode("utf-8")
        return source
    except (ValueError, TypeError, UnicodeError):
        raise InvalidInputError("invalid fit input") from None

class Tokenfold:
    def __init__(self, *, counter: TokenCounter | None = None, select: SelectRuntime | None = None, max_pending_scores: int = 32, score_wait_timeout_ms: int = 5000) -> None:
        _integer(max_pending_scores, 0, 128, ConfigError)
        _integer(score_wait_timeout_ms, 1, 120000, ConfigError)
        if select is not None and not isinstance(select, SelectRuntime):
            raise ConfigError("expected SelectRuntime")
        self._counter = counter if counter is not None else EncodingCounter()
        self._native = _native._Client(fspath(select.config) if select else None, max_pending_scores, score_wait_timeout_ms)
    def __enter__(self) -> Tokenfold:
        return self
    def __exit__(self, *args: object) -> None:
        self.close()
    async def __aenter__(self) -> Tokenfold:
        return self
    async def __aexit__(self, *args: object) -> None:
        await asyncio.to_thread(self.close)
    def warmup(self) -> None:
        self._native.warmup()
    def close(self) -> None:
        self._native.close()
    def _begin(self, request: Input, target_tokens: int, budget_mode: BudgetMode, allow_selection: bool, cancel: CancellationToken) -> _native._Fit:
        _integer(target_tokens, 0, 2**53 - 1, InvalidInputError)
        if budget_mode not in ("strict", "best_effort") or type(allow_selection) is not bool:
            raise ConfigError("invalid fit options")
        if type(self._counter) is EncodingCounter:
            kind, value = "encoding", self._counter.encoding
        elif type(self._counter) is ProviderCounter:
            kind, value = "openai", self._counter.model
        else:
            kind, value = "custom", self._counter_id()
        return self._native.begin(_source(request), target_tokens, budget_mode, allow_selection, kind, value, cancel)
    def _counter_id(self) -> str:
        try:
            value = self._counter.id
            if type(value) is not str:
                raise ValueError
            return value
        except Exception:
            raise AccountingError("invalid counter ID") from None
    def _step(self, session: _native._Fit, action: dict | None = None) -> dict:
        if action is not None:
            session.check_cancelled()
            before = self._counter_id()
            request = RenderedRequest(action["format"], json.dumps(action["payload"], ensure_ascii=False, separators=(",", ":")), action["scope"])
            session.check_cancelled()
            try:
                count = self._counter.count(request)
                after = self._counter_id()
                if not isinstance(count, TokenCount) or after != before:
                    raise ValueError
            except Exception:
                raise AccountingError("custom counter failed") from None
            return json.loads(session.step(json.dumps({"tokens": count.tokens, "basis": count.basis}), after))
        return json.loads(session.step())
    @staticmethod
    def _result(action: dict) -> FitResult[str] | FitResult[JsonObject]:
        return FitResult(json.dumps(action["payload"], ensure_ascii=False, separators=(",", ":")), Receipt.from_json(action["receipt"]))
    @overload
    def fit(self, request: str | Data | Context, *, target_tokens: int, allow_selection: bool = False, budget_mode: BudgetMode = "strict", cancel: CancellationToken | None = None) -> FitResult[str]: ...
    @overload
    def fit(self, request: Request, *, target_tokens: int, allow_selection: bool = False, budget_mode: BudgetMode = "strict", cancel: CancellationToken | None = None) -> FitResult[JsonObject]: ...
    def fit(self, request: Input, *, target_tokens: int, allow_selection: bool = False, budget_mode: BudgetMode = "strict", cancel: CancellationToken | None = None) -> FitResult[str] | FitResult[JsonObject]:
        token = cancel if cancel is not None else CancellationToken()
        if not isinstance(token, CancellationToken):
            raise InvalidInputError("invalid cancellation token")
        session = self._begin(request, target_tokens, budget_mode, allow_selection, token)
        action = self._step(session)
        while action["action"] != "done":
            if action["action"] == "score":
                session.wait_score()
                action = self._step(session)
            else:
                action = self._step(session, action)
        return self._result(action)
    @overload
    async def afit(self, request: str | Data | Context, *, target_tokens: int, allow_selection: bool = False, budget_mode: BudgetMode = "strict", cancel: CancellationToken | None = None) -> FitResult[str]: ...
    @overload
    async def afit(self, request: Request, *, target_tokens: int, allow_selection: bool = False, budget_mode: BudgetMode = "strict", cancel: CancellationToken | None = None) -> FitResult[JsonObject]: ...
    async def afit(self, request: Input, *, target_tokens: int, allow_selection: bool = False, budget_mode: BudgetMode = "strict", cancel: CancellationToken | None = None) -> FitResult[str] | FitResult[JsonObject]:
        token = cancel if cancel is not None else CancellationToken()
        if not isinstance(token, CancellationToken):
            raise InvalidInputError("invalid cancellation token")
        async def run() -> FitResult[str] | FitResult[JsonObject]:
            session = await asyncio.to_thread(self._begin, request, target_tokens, budget_mode, allow_selection, token)
            action = await asyncio.to_thread(self._step, session)
            while action["action"] != "done":
                if action["action"] == "score":
                    loop = asyncio.get_running_loop()
                    ready = loop.create_future()
                    session.on_score(lambda: loop.call_soon_threadsafe(ready.set_result, None))
                    await ready
                    action = await asyncio.to_thread(self._step, session)
                else:
                    action = await asyncio.to_thread(self._step, session, action)
            return self._result(action)
        running = asyncio.create_task(run())
        try:
            # Shield the entire operation, not just queued work: running native cleanup must finish.
            return await asyncio.shield(running)
        except asyncio.CancelledError:
            token.cancel()
            while not running.done():
                try:
                    await asyncio.shield(running)
                except asyncio.CancelledError:
                    continue
                except Exception:
                    break
            if running.done() and not running.cancelled():
                running.exception()
            raise

@overload
def fit(request: str | Data | Context, *, target_tokens: int, counter: TokenCounter | None = None, budget_mode: BudgetMode = "strict") -> FitResult[str]: ...
@overload
def fit(request: Request, *, target_tokens: int, counter: TokenCounter | None = None, budget_mode: BudgetMode = "strict") -> FitResult[JsonObject]: ...
def fit(request: Input, *, target_tokens: int, counter: TokenCounter | None = None, budget_mode: BudgetMode = "strict") -> FitResult[str] | FitResult[JsonObject]:
    with Tokenfold(counter=counter) as client:
        return client.fit(request, target_tokens=target_tokens, budget_mode=budget_mode)
