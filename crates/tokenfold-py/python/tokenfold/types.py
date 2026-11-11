"""Owned input snapshots and immutable fit evidence."""
from __future__ import annotations
from dataclasses import asdict, dataclass
import json
import math
from typing import Generic, Literal, TypeAlias, TypeVar, cast, Sequence
from . import _native
from .errors import InvalidInputError, BudgetUnmetError, BudgetIndeterminateError

JsonValue: TypeAlias = None | bool | int | float | str | list["JsonValue"] | dict[str, "JsonValue"]
JsonObject: TypeAlias = dict[str, JsonValue]
BudgetMode: TypeAlias = Literal["strict", "best_effort"]
BudgetStatus: TypeAlias = Literal["met", "unmet", "indeterminate"]
CountBasis: TypeAlias = Literal["exact", "estimated", "unavailable"]
FitFormat: TypeAlias = Literal["text", "json_data", "context", "openai_chat", "anthropic_messages"]
AccountingScope: TypeAlias = Literal["rendered_text", "provider_request"]
FitReason: TypeAlias = Literal["selection_disabled", "runtime_unavailable", "no_optional_groups", "required_over_budget", "unsupported_selection", "selection_limit", "scorer_failure", "scorer_timeout", "scorer_queue_timeout", "scorer_response_invalid", "no_valid_fit", "no_gain", "unknown_accounting", "estimated_fit"]


def _json(value: object) -> str:
    """Reject coercion and cycles before JSON encoding; source bounds are native-owned."""
    active: set[int] = set()
    def walk(item: object, depth: int) -> None:
        if depth > 64:
            raise InvalidInputError("JSON depth exceeds 64")
        if type(item) in (str, type(None), bool):
            return
        if isinstance(item, (int, float)) and type(item) in (int, float):
            if not math.isfinite(item) or (int(item) == item and abs(item) > 2**53 - 1):
                raise InvalidInputError("unsafe JSON number")
            return
        if type(item) not in (list, dict) or id(item) in active:
            raise InvalidInputError("invalid JSON value or cycle")
        active.add(id(item))
        if isinstance(item, dict):
            for key, child in item.items():
                if type(key) is not str:
                    raise InvalidInputError("JSON keys must be strings")
                walk(child, depth + 1)
        elif isinstance(item, list):
            for child in item:
                walk(child, depth + 1)
        active.remove(id(item))
    try:
        walk(value, 0)
        source = json.dumps(value, ensure_ascii=False, allow_nan=False, separators=(",", ":"))
        source.encode("utf-8")
        return source
    except (ValueError, TypeError, OverflowError, UnicodeError, RecursionError):
        raise InvalidInputError("invalid JSON value") from None


@dataclass(frozen=True, slots=True, init=False)
class Data:
    _source: str
    representation: Literal["json", "folded"]
    def __init__(self, value: JsonValue, representation: Literal["json", "folded"] = "json") -> None:
        if representation not in ("json", "folded"):
            raise InvalidInputError("invalid data representation")
        object.__setattr__(self, "_source", _json(value))
        object.__setattr__(self, "representation", representation)
    @property
    def value(self) -> JsonValue:
        return json.loads(self._source)

@dataclass(frozen=True, slots=True)
class Group:
    id: str
    content: str | Data
    required: bool = False

@dataclass(frozen=True, slots=True, init=False)
class Context:
    query: str
    groups: tuple[Group, ...]
    def __init__(self, query: str, groups: Sequence[Group]) -> None:
        object.__setattr__(self, "query", query)
        object.__setattr__(self, "groups", tuple(groups))

@dataclass(frozen=True, slots=True)
class MessageGroup:
    id: str
    indices: tuple[int, ...]
    required: bool = False
    def __post_init__(self) -> None:
        object.__setattr__(self, "indices", tuple(self.indices))

@dataclass(frozen=True, slots=True, init=False)
class Request:
    _source: str
    format: Literal["openai_chat", "anthropic_messages"]
    query: str | None
    groups: tuple[MessageGroup, ...]
    def __init__(self, payload: JsonObject, format: Literal["openai_chat", "anthropic_messages"], query: str | None = None, groups: Sequence[MessageGroup] = ()) -> None:
        if type(payload) is not dict or format not in ("openai_chat", "anthropic_messages"):
            raise InvalidInputError("invalid provider request")
        object.__setattr__(self, "_source", _json(payload))
        object.__setattr__(self, "format", format)
        object.__setattr__(self, "query", query)
        object.__setattr__(self, "groups", tuple(groups))
    @classmethod
    def openai(cls, payload: JsonObject, query: str | None = None, groups: Sequence[MessageGroup] = ()) -> Request:
        return cls(payload, "openai_chat", query, groups)
    @classmethod
    def anthropic(cls, payload: JsonObject, query: str | None = None, groups: Sequence[MessageGroup] = ()) -> Request:
        return cls(payload, "anthropic_messages", query, groups)
    @property
    def payload(self) -> JsonObject:
        return json.loads(self._source)

@dataclass(frozen=True, slots=True)
class RenderedRequest:
    format: FitFormat
    _payload_json: str
    scope: AccountingScope
    @property
    def payload(self) -> str | JsonObject:
        return json.loads(self._payload_json)

@dataclass(frozen=True, slots=True)
class Accounting:
    counter_id: str
    scope: AccountingScope
    original_basis: CountBasis
    baseline_basis: CountBasis
    final_basis: CountBasis

@dataclass(frozen=True, slots=True)
class ScorerInfo:
    revision: str
    manifest_hash: str
    runtime_id: str
    device: str
    precision: str
    qualification: Literal["unqualified"]

@dataclass(frozen=True, slots=True)
class FitWarning:
    code: str
    message: str

@dataclass(frozen=True, slots=True)
class TimingsMs:
    total: float | None
    core: float | None
    accounting: float | None
    score_queue: float | None
    startup: float | None
    scoring: float | None

@dataclass(frozen=True, slots=True)
class Receipt:
    kind: Literal["tokenfold.fit"]
    schema_version: Literal["1.0"]
    format: FitFormat
    budget_mode: BudgetMode
    budget_status: BudgetStatus
    reason: FitReason | None
    target_tokens: int
    original_tokens: int | None
    baseline_tokens: int | None
    final_tokens: int | None
    accounting: Accounting
    core_transforms: tuple[str, ...]
    scorer_attempted: bool
    scorer_succeeded: bool
    selection_applied: bool
    scorer: ScorerInfo | None
    groups_kept: tuple[str, ...]
    groups_dropped: tuple[str, ...]
    warnings: tuple[FitWarning, ...]
    timings_ms: TimingsMs
    def to_json(self) -> str:
        return _native.receipt_json(json.dumps(asdict(self), ensure_ascii=False, separators=(",", ":")))
    @classmethod
    def from_json(cls, source: str) -> Receipt:
        fields = json.loads(_native.receipt_json(source))
        fields["accounting"] = Accounting(**fields["accounting"])
        fields["timings_ms"] = TimingsMs(**fields["timings_ms"])
        fields["scorer"] = ScorerInfo(**fields["scorer"]) if fields["scorer"] else None
        fields["warnings"] = tuple(FitWarning(**w) for w in fields["warnings"])
        for key in ("core_transforms", "groups_kept", "groups_dropped"):
            fields[key] = tuple(fields[key])
        return cls(**fields)

P = TypeVar("P", str, JsonObject)
@dataclass(frozen=True, slots=True)
class FitResult(Generic[P]):
    _payload_json: str
    receipt: Receipt
    @property
    def payload(self) -> P:
        return cast(P, json.loads(self._payload_json))
    def require_met(self) -> P:
        if self.budget_status == "unmet":
            raise BudgetUnmetError(self.receipt)
        if self.budget_status != "met":
            raise BudgetIndeterminateError(self.receipt)
        return self.payload
    def require_fit(self) -> P:
        if self.budget_mode == "best_effort" and self.estimated_fit is True:
            return self.payload
        return self.require_met()
    @property
    def budget_mode(self) -> BudgetMode: return self.receipt.budget_mode
    @property
    def budget_status(self) -> BudgetStatus: return self.receipt.budget_status
    @property
    def reason(self) -> FitReason | None: return self.receipt.reason
    @property
    def target_tokens(self) -> int: return self.receipt.target_tokens
    @property
    def original_tokens(self) -> int | None: return self.receipt.original_tokens
    @property
    def baseline_tokens(self) -> int | None: return self.receipt.baseline_tokens
    @property
    def final_tokens(self) -> int | None: return self.receipt.final_tokens
    @property
    def groups_kept(self) -> tuple[str, ...]: return self.receipt.groups_kept
    @property
    def groups_dropped(self) -> tuple[str, ...]: return self.receipt.groups_dropped
    @property
    def selection_applied(self) -> bool: return self.receipt.selection_applied
    def _delta(self, value: int | None, basis: CountBasis) -> int | None:
        if value is None or self.final_tokens is None or basis != self.receipt.accounting.final_basis:
            return None
        return value - self.final_tokens
    @property
    def tokens_saved(self) -> int | None: return self._delta(self.original_tokens, self.receipt.accounting.original_basis)
    @property
    def selection_tokens_saved(self) -> int | None: return self._delta(self.baseline_tokens, self.receipt.accounting.baseline_basis)
    @property
    def estimated_fit(self) -> bool | None:
        return self.final_tokens <= self.target_tokens if self.receipt.accounting.final_basis == "estimated" and self.final_tokens is not None else None
