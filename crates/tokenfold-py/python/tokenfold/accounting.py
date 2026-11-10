"""Accounting profiles are local evidence, never provider billing guarantees."""
from dataclasses import dataclass
from typing import Protocol, Literal
from . import _native
from .types import RenderedRequest, CountBasis
from .errors import AccountingError, ConfigError

@dataclass(frozen=True, slots=True)
class TokenCount:
    tokens: int | None
    basis: CountBasis = "exact"
    def __post_init__(self) -> None:
        if self.basis not in ("exact", "estimated", "unavailable") or ((self.basis == "unavailable") != (self.tokens is None)) or (self.tokens is not None and (type(self.tokens) is not int or not 0 <= self.tokens <= 2**53 - 1)):
            raise AccountingError("invalid token count")

class TokenCounter(Protocol):
    @property
    def id(self) -> str: ...
    def count(self, request: RenderedRequest) -> TokenCount: ...

@dataclass(frozen=True, slots=True)
class EncodingCounter:
    encoding: Literal["o200k_base", "cl100k_base"] = "o200k_base"
    def __post_init__(self) -> None:
        if type(self.encoding) is not str:
            raise ConfigError("invalid encoding")
        _native.counter_id("encoding", self.encoding)
    @property
    def id(self) -> str:
        return f"encoding:{self.encoding}:v1"
    def count(self, request: RenderedRequest) -> TokenCount:
        import json
        return TokenCount(**json.loads(_native.count("encoding", self.encoding, request.format, request.scope, request._payload_json)))

@dataclass(frozen=True, slots=True)
class ProviderCounter:
    model: str
    def __post_init__(self) -> None:
        if type(self.model) is not str:
            raise ConfigError("invalid provider profile")
        _native.counter_id("openai", self.model)
    @classmethod
    def openai(cls, *, model: str) -> "ProviderCounter":
        return cls(model)
    @property
    def id(self) -> str:
        return f"openai_chat:{self.model}:local-text-v1"
    def count(self, request: RenderedRequest) -> TokenCount:
        import json
        return TokenCount(**json.loads(_native.count("openai", self.model, request.format, request.scope, request._payload_json)))
