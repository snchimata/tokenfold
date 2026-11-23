"""Historical compression APIs and report semantics, deliberately separate from fit."""
from ._native import (compress, inspect, decode, retrieve, parse_report,
    Preset, InputFormat, Encoding, Status, PruningPolicy, EstimatorInfo,
    CompressionReport, CompressionResult, TokenFoldError, InvalidInputError,
    SafetyError, EstimatorError, ConfigError, InternalError, RetrievalError,
    BudgetUnmetError)
