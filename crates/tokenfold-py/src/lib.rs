//! Python binding for `tokenfold-core`, per `INTERFACES.md` §5 ("Python Binding API").
//!
//! Naming convention (INTERFACES.md §5.4): Python-facing enum variant names use
//! `ALL_CAPS` (e.g. `CompressionMode.BALANCED`), while the underlying Rust enums
//! (`tokenfold_core::CompressionMode`, etc.) keep Rust's `PascalCase` convention
//! (`CompressionMode::Balanced`). The `#[pyo3(name = "...")]` attributes below are what
//! does that renaming at the FFI boundary.
//!
//! pyo3 0.22's `#[pyfunction]`/`#[pymethods]`/`create_exception!` macro expansions predate
//! this workspace's `edition2024`: they emit calls to macro-internal unsafe functions
//! without wrapping them in `unsafe {}` (edition2024's `unsafe_op_in_unsafe_fn` lint) and
//! reference a `gil-refs` cfg this crate never declares (rustc's `unexpected_cfgs` lint).
//! Both are pyo3-generated code this crate doesn't control, not real issues. Its
//! `#[pyfunction]` expansion also triggers `clippy::useless_conversion` on functions
//! returning `PyResult<T>` (the generated `?`-based error conversion), for the same reason.
#![allow(unsafe_op_in_unsafe_fn, unexpected_cfgs)]
#![allow(clippy::useless_conversion)]

use pyo3::IntoPyObjectExt;
use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyOSError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList};

use tokenfold_core::report::TransformStatus;
use tokenfold_core::{
    CompressionInput, CompressionMode as CoreMode, CompressionOutput as CoreOutput,
    CompressionPolicy as CorePolicy, InputFormat as CoreFormat, Status as CoreStatus,
    TokenFoldError as CoreError,
};

// ---------------------------------------------------------------------------------------
// Error hierarchy (INTERFACES.md §5.5, mapped exactly per roadmap.md F-003's error
// taxonomy table).
// ---------------------------------------------------------------------------------------

create_exception!(tokenfold, TokenFoldError, PyException);
create_exception!(tokenfold, InvalidInputError, TokenFoldError);
create_exception!(tokenfold, SafetyError, TokenFoldError);
create_exception!(tokenfold, EstimatorError, TokenFoldError);
create_exception!(tokenfold, ConfigError, TokenFoldError);
create_exception!(tokenfold, InternalError, TokenFoldError);

/// Maps `tokenfold_core::TokenFoldError` to the Python exception hierarchy above. `Io`
/// maps to the builtin `OSError`, not `InternalError` -- see roadmap.md F-003's table.
fn map_err(err: CoreError) -> PyErr {
    match err {
        CoreError::InvalidInput(msg) => InvalidInputError::new_err(msg),
        CoreError::SafetyViolation(msg) | CoreError::RedactionFailed(msg) => {
            SafetyError::new_err(msg)
        }
        CoreError::EstimatorError(msg) => EstimatorError::new_err(msg),
        CoreError::ConfigError(msg) => ConfigError::new_err(msg),
        CoreError::InternalError(msg) => InternalError::new_err(msg),
        CoreError::Io(e) => PyOSError::new_err(e.to_string()),
    }
}

// ---------------------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------------------

#[pyclass(name = "CompressionMode", eq, from_py_object)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PyCompressionMode {
    #[pyo3(name = "CONSERVATIVE")]
    Conservative,
    #[pyo3(name = "BALANCED")]
    Balanced,
    #[pyo3(name = "AGGRESSIVE")]
    Aggressive,
}

impl From<PyCompressionMode> for CoreMode {
    fn from(m: PyCompressionMode) -> Self {
        match m {
            PyCompressionMode::Conservative => CoreMode::Conservative,
            PyCompressionMode::Balanced => CoreMode::Balanced,
            PyCompressionMode::Aggressive => CoreMode::Aggressive,
        }
    }
}

impl From<CoreMode> for PyCompressionMode {
    fn from(m: CoreMode) -> Self {
        match m {
            CoreMode::Conservative => PyCompressionMode::Conservative,
            CoreMode::Balanced => PyCompressionMode::Balanced,
            CoreMode::Aggressive => PyCompressionMode::Aggressive,
        }
    }
}

fn parse_mode_str(s: &str) -> PyResult<CoreMode> {
    match s.to_ascii_uppercase().as_str() {
        "CONSERVATIVE" => Ok(CoreMode::Conservative),
        "BALANCED" => Ok(CoreMode::Balanced),
        "AGGRESSIVE" => Ok(CoreMode::Aggressive),
        other => Err(ConfigError::new_err(format!(
            "unknown CompressionMode: {other:?}"
        ))),
    }
}

#[pyclass(name = "InputFormat", eq, from_py_object)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PyInputFormat {
    #[pyo3(name = "AUTO")]
    Auto,
    #[pyo3(name = "OPENAI_JSON")]
    OpenaiJson,
    #[pyo3(name = "ANTHROPIC_JSON")]
    AnthropicJson,
    #[pyo3(name = "JSON")]
    Json,
    #[pyo3(name = "PLAIN_TEXT")]
    PlainText,
    #[pyo3(name = "COMMAND_OUTPUT")]
    CommandOutput,
    #[pyo3(name = "GIT_DIFF")]
    GitDiff,
}

impl From<PyInputFormat> for CoreFormat {
    fn from(f: PyInputFormat) -> Self {
        match f {
            PyInputFormat::Auto => CoreFormat::Auto,
            PyInputFormat::OpenaiJson => CoreFormat::OpenAiJson,
            PyInputFormat::AnthropicJson => CoreFormat::AnthropicJson,
            PyInputFormat::Json => CoreFormat::Json,
            PyInputFormat::PlainText => CoreFormat::PlainText,
            PyInputFormat::CommandOutput => CoreFormat::CommandOutput,
            PyInputFormat::GitDiff => CoreFormat::GitDiff,
        }
    }
}

fn parse_format_str(s: &str) -> PyResult<CoreFormat> {
    match s.to_ascii_uppercase().as_str() {
        "AUTO" => Ok(CoreFormat::Auto),
        "OPENAI_JSON" => Ok(CoreFormat::OpenAiJson),
        "ANTHROPIC_JSON" => Ok(CoreFormat::AnthropicJson),
        "JSON" => Ok(CoreFormat::Json),
        "PLAIN_TEXT" => Ok(CoreFormat::PlainText),
        "COMMAND_OUTPUT" => Ok(CoreFormat::CommandOutput),
        "GIT_DIFF" => Ok(CoreFormat::GitDiff),
        other => Err(ConfigError::new_err(format!(
            "unknown InputFormat: {other:?}"
        ))),
    }
}

#[pyclass(name = "Status", eq, from_py_object)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PyStatus {
    #[pyo3(name = "COMPRESSED")]
    Compressed,
    #[pyo3(name = "PASSTHROUGH")]
    Passthrough,
    #[pyo3(name = "BEST_EFFORT")]
    BestEffort,
    #[pyo3(name = "UNREACHABLE_TARGET")]
    UnreachableTarget,
}

impl From<CoreStatus> for PyStatus {
    fn from(s: CoreStatus) -> Self {
        match s {
            CoreStatus::Compressed => PyStatus::Compressed,
            CoreStatus::Passthrough => PyStatus::Passthrough,
            CoreStatus::BestEffort => PyStatus::BestEffort,
            CoreStatus::UnreachableTarget => PyStatus::UnreachableTarget,
        }
    }
}

/// Accepts either the `CompressionMode` enum or a (case-insensitive) string, per
/// INTERFACES.md §5.1's `mode: CompressionMode | str` signature.
#[derive(FromPyObject)]
enum ModeArg {
    Enum(PyCompressionMode),
    Str(String),
}

impl ModeArg {
    fn resolve(self) -> PyResult<CoreMode> {
        match self {
            ModeArg::Enum(m) => Ok(m.into()),
            ModeArg::Str(s) => parse_mode_str(&s),
        }
    }
}

/// Accepts either the `InputFormat` enum or a (case-insensitive) string, per
/// INTERFACES.md §5.1's `format: InputFormat | str` signature.
#[derive(FromPyObject)]
enum FormatArg {
    Enum(PyInputFormat),
    Str(String),
}

impl FormatArg {
    fn resolve(self) -> PyResult<CoreFormat> {
        match self {
            FormatArg::Enum(f) => Ok(f.into()),
            FormatArg::Str(s) => parse_format_str(&s),
        }
    }
}

/// `str` input is UTF-8 encoded to bytes; `bytes` input is used as-is (INTERFACES.md §5.1).
#[derive(FromPyObject)]
enum PayloadArg {
    Bytes(Vec<u8>),
    Str(String),
}

impl PayloadArg {
    fn into_bytes(self) -> Vec<u8> {
        match self {
            PayloadArg::Bytes(b) => b,
            PayloadArg::Str(s) => s.into_bytes(),
        }
    }
}

// ---------------------------------------------------------------------------------------
// CompressionPolicy (INTERFACES.md §5.3: "optional convenience dataclass mirroring the
// Rust policy")
// ---------------------------------------------------------------------------------------

#[pyclass(name = "CompressionPolicy", from_py_object)]
#[derive(Clone)]
pub struct PyCompressionPolicy(CorePolicy);

#[pymethods]
impl PyCompressionPolicy {
    #[new]
    #[pyo3(signature = (
        target_tokens=None,
        mode=None,
        disable=None,
        reserve_output_tokens=None,
        preserve_latest_user_message=None,
        unsafe_disable_redaction=false,
        experimental=false,
        enable=None,
        store_originals=false,
        retrieval_namespace=None,
        retrieval_ttl_seconds=None,
        retrieval_backend=None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        target_tokens: Option<usize>,
        mode: Option<ModeArg>,
        disable: Option<Vec<String>>,
        reserve_output_tokens: Option<usize>,
        preserve_latest_user_message: Option<bool>,
        unsafe_disable_redaction: bool,
        experimental: bool,
        enable: Option<Vec<String>>,
        store_originals: bool,
        retrieval_namespace: Option<String>,
        retrieval_ttl_seconds: Option<u64>,
        retrieval_backend: Option<String>,
    ) -> PyResult<Self> {
        let mut builder = CorePolicy::builder();
        if let Some(t) = target_tokens {
            builder = builder.target_tokens(t);
        }
        if let Some(m) = mode {
            builder = builder.mode(m.resolve()?);
        }
        for id in disable.unwrap_or_default() {
            builder = builder.disable(id);
        }
        if let Some(r) = reserve_output_tokens {
            builder = builder.reserve_output_tokens(r);
        }
        if let Some(p) = preserve_latest_user_message {
            builder = builder.preserve_latest_user_message(p);
        }
        builder = builder.unsafe_disable_redaction(unsafe_disable_redaction);
        builder = builder.experimental(experimental);
        for id in enable.unwrap_or_default() {
            builder = builder.enable(id);
        }
        builder = builder.store_originals(store_originals);
        if let Some(ns) = retrieval_namespace {
            builder = builder.retrieval_namespace(ns);
        }
        builder = builder.retrieval_ttl_seconds(retrieval_ttl_seconds);
        if let Some(backend) = retrieval_backend {
            builder = builder.retrieval_backend(backend);
        }
        Ok(Self(builder.build().map_err(map_err)?))
    }

    #[getter]
    fn target_tokens(&self) -> Option<usize> {
        self.0.target_tokens
    }

    #[getter]
    fn mode(&self) -> PyCompressionMode {
        self.0.mode.into()
    }

    #[getter]
    fn disable(&self) -> Vec<String> {
        self.0.disabled.clone()
    }

    #[getter]
    fn store_originals(&self) -> bool {
        self.0.store_originals
    }
}

// ---------------------------------------------------------------------------------------
// CompressionReport / EstimatorInfo (INTERFACES.md §5.3)
// ---------------------------------------------------------------------------------------

#[pyclass(name = "EstimatorInfo", from_py_object)]
#[derive(Clone)]
pub struct PyEstimatorInfo {
    #[pyo3(get)]
    backend: String,
    #[pyo3(get)]
    model: Option<String>,
    #[pyo3(get)]
    is_exact: bool,
}

#[pyclass(name = "CompressionReport")]
pub struct PyCompressionReport {
    #[pyo3(get)]
    schema_version: String,
    #[pyo3(get)]
    original_tokens: usize,
    #[pyo3(get)]
    compressed_tokens: usize,
    #[pyo3(get)]
    saved_tokens: usize,
    #[pyo3(get)]
    savings_ratio: f64,
    #[pyo3(get)]
    savings_pct: f64,
    #[pyo3(get)]
    estimator: Py<PyEstimatorInfo>,
    #[pyo3(get)]
    status: PyStatus,
    #[pyo3(get)]
    mode: String,
    #[pyo3(get)]
    format: String,
    #[pyo3(get)]
    task_scope: String,
    #[pyo3(get)]
    warnings: Vec<String>,
    /// Full report, structurally converted to a plain Python dict (INTERFACES.md §5.3
    /// only requires `saved_tokens`/`estimator`/`status` as first-class attributes;
    /// everything else -- `quality`, `budget`, `cache`, `retrieval`, `transforms`, etc. --
    /// is available here rather than modeled as another dozen pyclasses).
    #[pyo3(get)]
    raw: Py<PyAny>,
}

fn report_to_py(
    py: Python<'_>,
    report: &tokenfold_core::report::CompressionReport,
) -> PyResult<Py<PyCompressionReport>> {
    let estimator = Py::new(
        py,
        PyEstimatorInfo {
            backend: report.estimator.backend.clone(),
            model: report.estimator.model.clone(),
            is_exact: report.estimator.is_exact,
        },
    )?;
    let raw_value =
        serde_json::to_value(report).map_err(|e| InternalError::new_err(e.to_string()))?;
    let raw = json_to_py(py, &raw_value)?;
    Py::new(
        py,
        PyCompressionReport {
            schema_version: report.schema_version.clone(),
            original_tokens: report.original_tokens,
            compressed_tokens: report.compressed_tokens,
            saved_tokens: report.saved_tokens,
            savings_ratio: report.savings_ratio,
            savings_pct: report.savings_pct,
            estimator,
            status: report.status.clone().into(),
            mode: report.mode.clone(),
            format: report.format.clone(),
            task_scope: report.task_scope.clone(),
            warnings: report.warnings.iter().map(|w| w.message.clone()).collect(),
            raw,
        },
    )
}

// ---------------------------------------------------------------------------------------
// CompressionResult (INTERFACES.md §5.3)
// ---------------------------------------------------------------------------------------

#[pyclass(name = "CompressionResult")]
pub struct PyCompressionResult {
    #[pyo3(get)]
    payload: Py<PyBytes>,
    #[pyo3(get)]
    report: Py<PyCompressionReport>,
}

#[pymethods]
impl PyCompressionResult {
    fn saved_pct(&self, py: Python<'_>) -> f64 {
        self.report.borrow(py).savings_pct
    }

    fn is_over_budget(&self, py: Python<'_>) -> bool {
        matches!(
            self.report.borrow(py).status,
            PyStatus::BestEffort | PyStatus::UnreachableTarget
        )
    }
}

fn output_to_result(
    py: Python<'_>,
    out: CoreOutput,
    payload_override: Option<&[u8]>,
) -> PyResult<PyCompressionResult> {
    let payload_bytes = payload_override.unwrap_or(&out.bytes);
    let payload = PyBytes::new(py, payload_bytes).unbind();
    let report = report_to_py(py, &out.report)?;
    Ok(PyCompressionResult { payload, report })
}

// ---------------------------------------------------------------------------------------
// JSON <-> Python conversion helpers (no `pythonize` dependency; both directions are a
// handful of lines and this is the only place either is needed).
// ---------------------------------------------------------------------------------------

fn json_to_py(py: Python<'_>, value: &serde_json::Value) -> PyResult<Py<PyAny>> {
    use serde_json::Value;
    match value {
        Value::Null => Ok(py.None()),
        Value::Bool(b) => (*b).into_py_any(py),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.into_py_any(py)
            } else if let Some(u) = n.as_u64() {
                u.into_py_any(py)
            } else {
                n.as_f64().unwrap_or(0.0).into_py_any(py)
            }
        }
        Value::String(s) => s.into_py_any(py),
        Value::Array(items) => {
            let list = PyList::empty(py);
            for item in items {
                list.append(json_to_py(py, item)?)?;
            }
            list.into_py_any(py)
        }
        Value::Object(map) => {
            let dict = PyDict::new(py);
            for (k, v) in map {
                dict.set_item(k, json_to_py(py, v)?)?;
            }
            dict.into_py_any(py)
        }
    }
}

fn py_to_json(value: &Bound<'_, PyAny>) -> PyResult<serde_json::Value> {
    if value.is_none() {
        Ok(serde_json::Value::Null)
    } else if let Ok(b) = value.extract::<bool>() {
        Ok(serde_json::Value::Bool(b))
    } else if let Ok(i) = value.extract::<i64>() {
        Ok(serde_json::Value::Number(i.into()))
    } else if let Ok(f) = value.extract::<f64>() {
        Ok(serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null))
    } else if let Ok(s) = value.extract::<String>() {
        Ok(serde_json::Value::String(s))
    } else if let Ok(list) = value.cast::<PyList>() {
        let mut arr = Vec::with_capacity(list.len());
        for item in list.iter() {
            arr.push(py_to_json(&item)?);
        }
        Ok(serde_json::Value::Array(arr))
    } else if let Ok(dict) = value.cast::<PyDict>() {
        let mut map = serde_json::Map::new();
        for (k, v) in dict.iter() {
            let key: String = k.extract()?;
            map.insert(key, py_to_json(&v)?);
        }
        Ok(serde_json::Value::Object(map))
    } else {
        Err(InvalidInputError::new_err(format!(
            "unsupported message value type: {}",
            value.get_type().name()?
        )))
    }
}

// ---------------------------------------------------------------------------------------
// Policy resolution: merges an optional `CompressionPolicy` with per-call keyword
// arguments, explicit keyword arguments winning (INTERFACES.md §5.3 "explicit keyword
// arguments win, matching CLI precedence rules").
// ---------------------------------------------------------------------------------------

fn effective_policy(
    policy: Option<&PyCompressionPolicy>,
    mode: Option<ModeArg>,
    target_tokens: Option<usize>,
    disable: Option<Vec<String>>,
) -> PyResult<CorePolicy> {
    let base = policy.map(|p| &p.0);
    let mut builder = CorePolicy::builder();

    let resolved_mode = match mode {
        Some(m) => m.resolve()?,
        None => base.map(|p| p.mode).unwrap_or(CoreMode::Balanced),
    };
    builder = builder.mode(resolved_mode);

    let resolved_target = target_tokens.or_else(|| base.and_then(|p| p.target_tokens));
    if let Some(t) = resolved_target {
        builder = builder.target_tokens(t);
    }

    let resolved_disable =
        disable.unwrap_or_else(|| base.map(|p| p.disabled.clone()).unwrap_or_default());
    for id in resolved_disable {
        builder = builder.disable(id);
    }

    if let Some(p) = base {
        builder = builder.reserve_output_tokens(p.reserve_output_tokens);
        builder = builder.preserve_latest_user_message(p.preserve_latest_user_message);
        builder = builder.unsafe_disable_redaction(p.unsafe_disable_redaction);
        builder = builder.experimental(p.experimental);
        for id in &p.enable {
            builder = builder.enable(id.clone());
        }
        builder = builder.store_originals(p.store_originals);
        builder = builder.retrieval_namespace(p.retrieval_namespace.clone());
        builder = builder.retrieval_ttl_seconds(p.retrieval_ttl_seconds);
        builder = builder.retrieval_backend(p.retrieval_backend.clone());
        builder = builder.retrieval_store_path(p.retrieval_store_path.clone());
        builder = builder.task_scope(p.task_scope);
        if let Some(cb) = p.cache_boundary {
            builder = builder.cache_boundary(cb);
        }
    }
    builder.build().map_err(map_err)
}

/// F-002's "fails closed for budget decisions OR proceeds with `--allow-heuristic-budget`"
/// criterion, implemented at this binding's boundary: `tokenfold_core::compress` doesn't
/// itself gate on this (its default build always has the `tiktoken` feature on, so this is
/// effectively a defense against a non-default build), but the Python signature documents
/// the parameter, so it's honored here.
fn check_estimator_budget_gate(
    report: &tokenfold_core::report::CompressionReport,
    target_tokens: Option<usize>,
    allow_heuristic_budget: bool,
) -> PyResult<()> {
    if target_tokens.is_some() && !report.estimator.is_exact && !allow_heuristic_budget {
        return Err(EstimatorError::new_err(
            "exact token estimator unavailable for a budget-constrained call; pass \
             allow_heuristic_budget=True to opt into the heuristic estimator"
                .to_string(),
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_compress(
    py: Python<'_>,
    format: CoreFormat,
    payload: PayloadArg,
    policy: Option<&PyCompressionPolicy>,
    mode: Option<ModeArg>,
    target_tokens: Option<usize>,
    disable: Option<Vec<String>>,
    allow_heuristic_budget: bool,
    dry_run: bool,
) -> PyResult<PyCompressionResult> {
    let bytes = payload.into_bytes();
    let resolved_policy = effective_policy(policy, mode, target_tokens, disable)?;
    let input = CompressionInput {
        format,
        bytes: bytes.clone(),
    };
    let out = tokenfold_core::compress(input, &resolved_policy).map_err(map_err)?;
    check_estimator_budget_gate(
        &out.report,
        resolved_policy.target_tokens,
        allow_heuristic_budget,
    )?;
    let payload_override = if dry_run {
        Some(bytes.as_slice())
    } else {
        None
    };
    output_to_result(py, out, payload_override)
}

// ---------------------------------------------------------------------------------------
// Public functions (INTERFACES.md §5.1)
// ---------------------------------------------------------------------------------------

#[pyfunction]
#[pyo3(signature = (payload, *, policy=None, format=None, mode=None, target_tokens=None, disable=None, allow_heuristic_budget=false))]
#[allow(clippy::too_many_arguments)]
fn compress(
    py: Python<'_>,
    payload: PayloadArg,
    policy: Option<PyRef<'_, PyCompressionPolicy>>,
    format: Option<FormatArg>,
    mode: Option<ModeArg>,
    target_tokens: Option<usize>,
    disable: Option<Vec<String>>,
    allow_heuristic_budget: bool,
) -> PyResult<PyCompressionResult> {
    let resolved_format = format
        .map(FormatArg::resolve)
        .transpose()?
        .unwrap_or(CoreFormat::Auto);
    run_compress(
        py,
        resolved_format,
        payload,
        policy.as_deref(),
        mode,
        target_tokens,
        disable,
        allow_heuristic_budget,
        false,
    )
}

#[pyfunction]
#[pyo3(signature = (payload, *, policy=None, format=None, mode=None, target_tokens=None))]
fn inspect(
    py: Python<'_>,
    payload: PayloadArg,
    policy: Option<PyRef<'_, PyCompressionPolicy>>,
    format: Option<FormatArg>,
    mode: Option<ModeArg>,
    target_tokens: Option<usize>,
) -> PyResult<PyCompressionResult> {
    let resolved_format = format
        .map(FormatArg::resolve)
        .transpose()?
        .unwrap_or(CoreFormat::Auto);
    run_compress(
        py,
        resolved_format,
        payload,
        policy.as_deref(),
        mode,
        target_tokens,
        None,
        // inspect() never applies compression, so a fail-closed budget gate would only get
        // in the way of a dry-run preview; always let it through.
        true,
        true,
    )
}

#[pyfunction]
#[pyo3(signature = (payload, *, policy=None, mode=None, target_tokens=None, disable=None, allow_heuristic_budget=false))]
#[allow(clippy::too_many_arguments)]
fn compress_openai_payload(
    py: Python<'_>,
    payload: PayloadArg,
    policy: Option<PyRef<'_, PyCompressionPolicy>>,
    mode: Option<ModeArg>,
    target_tokens: Option<usize>,
    disable: Option<Vec<String>>,
    allow_heuristic_budget: bool,
) -> PyResult<PyCompressionResult> {
    run_compress(
        py,
        CoreFormat::OpenAiJson,
        payload,
        policy.as_deref(),
        mode,
        target_tokens,
        disable,
        allow_heuristic_budget,
        false,
    )
}

#[pyfunction]
#[pyo3(signature = (payload, *, policy=None, mode=None, target_tokens=None, disable=None, allow_heuristic_budget=false))]
#[allow(clippy::too_many_arguments)]
fn compress_anthropic_payload(
    py: Python<'_>,
    payload: PayloadArg,
    policy: Option<PyRef<'_, PyCompressionPolicy>>,
    mode: Option<ModeArg>,
    target_tokens: Option<usize>,
    disable: Option<Vec<String>>,
    allow_heuristic_budget: bool,
) -> PyResult<PyCompressionResult> {
    run_compress(
        py,
        CoreFormat::AnthropicJson,
        payload,
        policy.as_deref(),
        mode,
        target_tokens,
        disable,
        allow_heuristic_budget,
        false,
    )
}

// ---------------------------------------------------------------------------------------
// compress_messages (INTERFACES.md §5.2)
// ---------------------------------------------------------------------------------------

#[pyclass(name = "MessagesCompressionResult")]
pub struct PyMessagesCompressionResult {
    #[pyo3(get)]
    payload: Py<PyBytes>,
    #[pyo3(get)]
    report: Py<PyCompressionReport>,
    #[pyo3(get)]
    messages: Py<PyAny>,
    #[pyo3(get)]
    tokens_before: usize,
    #[pyo3(get)]
    tokens_after: usize,
    #[pyo3(get)]
    tokens_saved: usize,
    #[pyo3(get)]
    savings_pct: f64,
    #[pyo3(get)]
    transforms_applied: Vec<String>,
    #[pyo3(get)]
    retrieval_hashes: Vec<String>,
}

#[pyfunction]
#[pyo3(signature = (messages, *, model="gpt-4.1", token_budget=None, mode=None))]
fn compress_messages(
    py: Python<'_>,
    messages: &Bound<'_, PyList>,
    // `model` is accepted for API compatibility with INTERFACES.md §5.2, but
    // `tokenfold_core::compress` doesn't yet route estimator choice by model name (it
    // always tries `o200k_base` when the `tiktoken` feature is compiled in) -- recorded
    // honestly rather than faking model-specific tokenization.
    model: &str,
    token_budget: Option<usize>,
    mode: Option<ModeArg>,
) -> PyResult<PyMessagesCompressionResult> {
    let _ = model;
    let mut messages_json = Vec::with_capacity(messages.len());
    for item in messages.iter() {
        messages_json.push(py_to_json(&item)?);
    }
    let payload_value = serde_json::json!({ "messages": messages_json });
    let payload_bytes = serde_json::to_vec(&payload_value)
        .map_err(|e| InvalidInputError::new_err(e.to_string()))?;

    let resolved_mode = match mode {
        Some(m) => m.resolve()?,
        None => CoreMode::Balanced,
    };
    let mut builder = CorePolicy::builder().mode(resolved_mode);
    if let Some(budget) = token_budget {
        builder = builder.target_tokens(budget);
    }
    let policy = builder.build().map_err(map_err)?;

    let input = CompressionInput {
        format: CoreFormat::OpenAiJson,
        bytes: payload_bytes,
    };
    let out = tokenfold_core::compress(input, &policy).map_err(map_err)?;

    let compressed_value: serde_json::Value = serde_json::from_slice(&out.bytes).map_err(|e| {
        InternalError::new_err(format!("compressed payload was not valid JSON: {e}"))
    })?;
    let messages_value = compressed_value
        .get("messages")
        .cloned()
        .unwrap_or(serde_json::Value::Array(Vec::new()));
    let messages_out = json_to_py(py, &messages_value)?;

    let transforms_applied = out
        .report
        .transforms
        .iter()
        .filter(|t| t.status == TransformStatus::Applied)
        .map(|t| t.id.clone())
        .collect();

    let tokens_before = out.report.original_tokens;
    let tokens_after = out.report.compressed_tokens;
    let tokens_saved = out.report.saved_tokens;
    let savings_pct = out.report.savings_pct;
    let report = report_to_py(py, &out.report)?;
    let payload = PyBytes::new(py, &out.bytes).unbind();

    Ok(PyMessagesCompressionResult {
        payload,
        report,
        messages: messages_out,
        tokens_before,
        tokens_after,
        tokens_saved,
        savings_pct,
        transforms_applied,
        // RetrievalReport carries no per-entry content hash yet (see INTERFACES.md §4's
        // "Implementation status (v0.2)" note on `tokenfold_retrieve`), so this is always
        // empty rather than fabricated.
        retrieval_hashes: Vec::new(),
    })
}

// ---------------------------------------------------------------------------------------
// Module registration
// ---------------------------------------------------------------------------------------

#[pymodule]
fn tokenfold(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyCompressionMode>()?;
    m.add_class::<PyInputFormat>()?;
    m.add_class::<PyStatus>()?;
    m.add_class::<PyCompressionPolicy>()?;
    m.add_class::<PyEstimatorInfo>()?;
    m.add_class::<PyCompressionReport>()?;
    m.add_class::<PyCompressionResult>()?;
    m.add_class::<PyMessagesCompressionResult>()?;

    m.add("TokenFoldError", py.get_type::<TokenFoldError>())?;
    m.add("InvalidInputError", py.get_type::<InvalidInputError>())?;
    m.add("SafetyError", py.get_type::<SafetyError>())?;
    m.add("EstimatorError", py.get_type::<EstimatorError>())?;
    m.add("ConfigError", py.get_type::<ConfigError>())?;
    m.add("InternalError", py.get_type::<InternalError>())?;

    m.add_function(wrap_pyfunction!(compress, m)?)?;
    m.add_function(wrap_pyfunction!(inspect, m)?)?;
    m.add_function(wrap_pyfunction!(compress_openai_payload, m)?)?;
    m.add_function(wrap_pyfunction!(compress_anthropic_payload, m)?)?;
    m.add_function(wrap_pyfunction!(compress_messages, m)?)?;
    Ok(())
}
