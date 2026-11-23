"""Errors contain bounded diagnostics, never input or callback exception text."""
from ._native import (TokenFoldError, InvalidInputError, SafetyError, ConfigError,
                      InternalError, AccountingError, BusyError, ClosedError,
                      ScorerError, CancelledError)
from typing import TYPE_CHECKING
if TYPE_CHECKING:
    from .types import Receipt

class BudgetUnmetError(TokenFoldError):
    def __init__(self, receipt: "Receipt") -> None:
        super().__init__("budget unmet")
        self.receipt = receipt

class BudgetIndeterminateError(TokenFoldError):
    def __init__(self, receipt: "Receipt") -> None:
        super().__init__("budget indeterminate")
        self.receipt = receipt
