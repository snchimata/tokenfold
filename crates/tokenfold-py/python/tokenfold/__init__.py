"""Offline native request fitting; experimental scoring requires explicit approval."""
class InstallationError(ImportError):
    pass
try:
    from . import _native
except ImportError:
    raise InstallationError("Install a supported tokenfold wheel for Python >=3.12; no fallback backend is used") from None
from .types import (Data, Context, Group, Request, MessageGroup, FitResult, Receipt,
                    RenderedRequest, JsonObject, JsonValue, Accounting, ScorerInfo,
                    FitWarning, TimingsMs, BudgetMode, BudgetStatus, CountBasis,
                    FitFormat, AccountingScope, FitReason)
from .accounting import TokenCount, EncodingCounter, ProviderCounter, TokenCounter
from .client import fit, Tokenfold, SelectRuntime, CancellationToken
from .errors import (TokenFoldError, InvalidInputError, SafetyError, ConfigError,
                     InternalError, AccountingError, BusyError, ClosedError,
                     ScorerError, CancelledError, BudgetUnmetError,
                     BudgetIndeterminateError)
__all__ = ["fit", "Tokenfold", "Data", "Context", "Group", "Request", "MessageGroup",
           "FitResult", "Receipt", "TokenCount", "EncodingCounter", "ProviderCounter",
           "TokenCounter", "SelectRuntime", "CancellationToken", "RenderedRequest",
           "JsonObject", "JsonValue", "Accounting", "ScorerInfo", "FitWarning", "TimingsMs",
           "BudgetMode", "BudgetStatus", "CountBasis", "FitFormat", "AccountingScope", "FitReason",
           "TokenFoldError", "InvalidInputError", "SafetyError", "ConfigError", "InternalError",
           "AccountingError", "BusyError", "ClosedError", "ScorerError", "CancelledError",
           "BudgetUnmetError", "BudgetIndeterminateError", "InstallationError"]
