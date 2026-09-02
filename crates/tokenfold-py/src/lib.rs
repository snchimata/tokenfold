//! Python binding for `tokenfold-core`.
//!
//! Naming convention: Python-facing enum variant names use
//! `ALL_CAPS` (e.g. `Preset.BALANCED`), while the underlying Rust enums
//! (`tokenfold_core::Preset`, etc.) keep Rust's `PascalCase` convention
//! (`Preset::Balanced`). The `#[pyo3(name = "...")]` attributes below are what
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

use std::path::PathBuf;

use pyo3::IntoPyObjectExt;
use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyOSError, PyUnicodeDecodeError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList};

use tokenfold_core::retrieval_store::{RetrievalOutcome, RetrievalStore};
use tokenfold_core::{
    CompressionInput, CompressionOutput as CoreOutput, CompressionPolicy as CorePolicy,
    InputFormat as CoreFormat, LossyPath as CoreLossyPath, Preset as CorePreset,
    Status as CoreStatus, TokenFoldError as CoreError,
};

// ---------------------------------------------------------------------------------------
// Error hierarchy: `TokenFoldError` is the catch-all base; each subclass below mirrors one
// `tokenfold_core::TokenFoldError` variant, so `except TokenFoldError:` catches everything
// while callers can still handle a specific failure.
// ---------------------------------------------------------------------------------------

create_exception!(tokenfold, TokenFoldError, PyException);
create_exception!(tokenfold, InvalidInputError, TokenFoldError);
create_exception!(tokenfold, SafetyError, TokenFoldError);
create_exception!(tokenfold, EstimatorError, TokenFoldError);
create_exception!(tokenfold, ConfigError, TokenFoldError);
create_exception!(tokenfold, InternalError, TokenFoldError);
// Raised by retrieve() when a hash is not found in the given namespace, or was found but its
// TTL has elapsed -- the two non-`Found` arms of `retrieval_store::RetrievalOutcome`. A plain
// `//` comment, not `///`: `create_exception!` doesn't attach outer doc comments to the item
// it generates, so a doc comment here would just be a `-D warnings`-tripping dead comment.
create_exception!(tokenfold, RetrievalError, TokenFoldError);
create_exception!(tokenfold, BudgetUnmetError, TokenFoldError);

/// Maps `tokenfold_core::TokenFoldError` to the Python exception hierarchy above. `Io`
/// maps to the builtin `OSError`, not `InternalError`: an I/O failure is the caller's
/// environment misbehaving, not an unexpected panic out of the Rust core.
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

#[pyclass(name = "Preset", eq, from_py_object)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PyPreset {
    #[pyo3(name = "CONSERVATIVE")]
    Conservative,
    #[pyo3(name = "BALANCED")]
    Balanced,
    #[pyo3(name = "AGGRESSIVE")]
    Aggressive,
}

impl From<PyPreset> for CorePreset {
    fn from(m: PyPreset) -> Self {
        match m {
            PyPreset::Conservative => CorePreset::Conservative,
            PyPreset::Balanced => CorePreset::Balanced,
            PyPreset::Aggressive => CorePreset::Aggressive,
        }
    }
}

impl From<CorePreset> for PyPreset {
    fn from(m: CorePreset) -> Self {
        match m {
            CorePreset::Conservative => PyPreset::Conservative,
            CorePreset::Balanced => PyPreset::Balanced,
            CorePreset::Aggressive => PyPreset::Aggressive,
        }
    }
}

fn parse_preset_str(s: &str) -> PyResult<CorePreset> {
    match s.to_ascii_uppercase().as_str() {
        "CONSERVATIVE" => Ok(CorePreset::Conservative),
        "BALANCED" => Ok(CorePreset::Balanced),
        "AGGRESSIVE" => Ok(CorePreset::Aggressive),
        other => Err(ConfigError::new_err(format!("unknown Preset: {other:?}"))),
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

#[pyclass(name = "Encoding", eq, from_py_object)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PyEncoding {
    #[pyo3(name = "JSON")]
    Json,
    #[pyo3(name = "TOON")]
    Toon,
}

impl From<PyEncoding> for tokenfold_core::OutputEncoding {
    fn from(value: PyEncoding) -> Self {
        match value {
            PyEncoding::Json => Self::Json,
            PyEncoding::Toon => Self::Toon,
        }
    }
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
}

impl From<CoreStatus> for PyStatus {
    fn from(s: CoreStatus) -> Self {
        match s {
            CoreStatus::Compressed => PyStatus::Compressed,
            CoreStatus::Passthrough => PyStatus::Passthrough,
        }
    }
}

/// Selection backend for opt-in lossy JSON array-item pruning -- see `CompressionPolicy.lossy`.
/// `HEURISTIC` is the only Phase 1 implementation; deliberately a single-variant enum rather
/// than a bare `bool`, matching `tokenfold_core::budget::LossyPath`'s own rationale (this
/// names an algorithm, not a toggle) and the CLI's `--lossy heuristic` shape.
#[pyclass(name = "LossyPath", eq, from_py_object)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PyLossyPath {
    #[pyo3(name = "HEURISTIC")]
    Heuristic,
}

impl From<PyLossyPath> for CoreLossyPath {
    fn from(p: PyLossyPath) -> Self {
        match p {
            PyLossyPath::Heuristic => CoreLossyPath::Heuristic,
        }
    }
}

impl From<CoreLossyPath> for PyLossyPath {
    fn from(p: CoreLossyPath) -> Self {
        match p {
            CoreLossyPath::Heuristic => PyLossyPath::Heuristic,
        }
    }
}

fn parse_lossy_str(s: &str) -> PyResult<CoreLossyPath> {
    match s.to_ascii_uppercase().as_str() {
        "HEURISTIC" => Ok(CoreLossyPath::Heuristic),
        other => Err(ConfigError::new_err(format!(
            "unknown LossyPath: {other:?}"
        ))),
    }
}

/// Accepts either the `Preset` enum or a (case-insensitive) string, matching the
/// public `preset: Preset | str` signature.
#[derive(FromPyObject)]
enum PresetArg {
    Enum(PyPreset),
    Str(String),
}

impl PresetArg {
    fn resolve(self) -> PyResult<CorePreset> {
        match self {
            PresetArg::Enum(m) => Ok(m.into()),
            PresetArg::Str(s) => parse_preset_str(&s),
        }
    }
}

/// Accepts either the `InputFormat` enum or a (case-insensitive) string, matching the
/// public `format: InputFormat | str` signature.
#[derive(FromPyObject)]
enum FormatArg {
    Enum(PyInputFormat),
    Str(String),
}

#[derive(FromPyObject)]
enum EncodingArg {
    Enum(PyEncoding),
    Str(String),
}

impl EncodingArg {
    fn resolve(self) -> PyResult<tokenfold_core::OutputEncoding> {
        match self {
            Self::Enum(value) => Ok(value.into()),
            Self::Str(value) if value.eq_ignore_ascii_case("json") => {
                Ok(tokenfold_core::OutputEncoding::Json)
            }
            Self::Str(value) if value.eq_ignore_ascii_case("toon") => {
                Ok(tokenfold_core::OutputEncoding::Toon)
            }
            Self::Str(value) => Err(ConfigError::new_err(format!("unknown Encoding: {value:?}"))),
        }
    }
}

impl FormatArg {
    fn resolve(self) -> PyResult<CoreFormat> {
        match self {
            FormatArg::Enum(f) => Ok(f.into()),
            FormatArg::Str(s) => parse_format_str(&s),
        }
    }
}

/// Accepts either the `LossyPath` enum or a (case-insensitive) string, matching the public
/// `lossy: LossyPath | str | None` signature.
#[derive(FromPyObject)]
enum LossyArg {
    Enum(PyLossyPath),
    Str(String),
}

impl LossyArg {
    fn resolve(self) -> PyResult<CoreLossyPath> {
        match self {
            LossyArg::Enum(p) => Ok(p.into()),
            LossyArg::Str(s) => parse_lossy_str(&s),
        }
    }
}

/// `str` input is UTF-8 encoded to bytes; `bytes` input is used as-is. The core API is
/// bytes-first, so `CompressionResult.payload` always comes back as `bytes`.
#[derive(FromPyObject)]
enum PayloadArg {
    Bytes(Vec<u8>),
    Str(String),
}

#[pyclass(name = "PruningPolicy", from_py_object)]
#[derive(Clone)]
pub struct PyPruningPolicy {
    keep_ratio: Option<f64>,
    preserve_paths: Vec<String>,
    retrieval_store: Option<PathBuf>,
    retrieval_namespace: Option<String>,
}

#[pymethods]
impl PyPruningPolicy {
    #[new]
    #[pyo3(signature = (keep_ratio=None, preserve_paths=None, retrieval_store=None, retrieval_namespace=None))]
    fn new(
        keep_ratio: Option<f64>,
        preserve_paths: Option<Vec<String>>,
        retrieval_store: Option<PathBuf>,
        retrieval_namespace: Option<String>,
    ) -> PyResult<Self> {
        if keep_ratio.is_some_and(|ratio| !(0.0 < ratio && ratio <= 1.0)) {
            return Err(ConfigError::new_err(
                "keep_ratio must be greater than 0 and at most 1",
            ));
        }
        Ok(Self {
            keep_ratio,
            preserve_paths: preserve_paths.unwrap_or_default(),
            retrieval_store,
            retrieval_namespace,
        })
    }

    #[getter]
    fn keep_ratio(&self) -> Option<f64> {
        self.keep_ratio
    }
    #[getter]
    fn preserve_paths(&self) -> Vec<String> {
        self.preserve_paths.clone()
    }
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
// CompressionPolicy: an optional convenience type mirroring the Rust policy, so callers can
// build one policy object once instead of repeating keyword arguments per call.
// ---------------------------------------------------------------------------------------

#[pyclass(name = "CompressionPolicy", from_py_object)]
#[derive(Clone)]
pub struct PyCompressionPolicy(CorePolicy);

#[pymethods]
impl PyCompressionPolicy {
    #[new]
    #[pyo3(signature = (
        target_tokens=None,
        preset=None,
        disable=None,
        reserve_output_tokens=None,
        preserve_latest_user_message=None,
        experimental=false,
        enable=None,
        store_originals=false,
        retrieval_namespace=None,
        retrieval_ttl_seconds=None,
        retrieval_backend=None,
        retrieval_store_path=None,
        lossy=None,
        lossy_ratio=None,
        lossy_preserve=None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        target_tokens: Option<usize>,
        preset: Option<PresetArg>,
        disable: Option<Vec<String>>,
        reserve_output_tokens: Option<usize>,
        preserve_latest_user_message: Option<bool>,
        experimental: bool,
        enable: Option<Vec<String>>,
        store_originals: bool,
        retrieval_namespace: Option<String>,
        retrieval_ttl_seconds: Option<u64>,
        retrieval_backend: Option<String>,
        retrieval_store_path: Option<PathBuf>,
        lossy: Option<LossyArg>,
        lossy_ratio: Option<f64>,
        lossy_preserve: Option<Vec<String>>,
    ) -> PyResult<Self> {
        let mut builder = CorePolicy::builder();
        if let Some(t) = target_tokens {
            builder = builder.target_tokens(t);
        }
        if let Some(m) = preset {
            builder = builder.preset(m.resolve()?);
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
        builder = builder.retrieval_store_path(retrieval_store_path);
        if let Some(l) = lossy {
            builder = builder.lossy(l.resolve()?);
        }
        if let Some(r) = lossy_ratio {
            builder = builder.lossy_ratio(r);
        }
        for path in lossy_preserve.unwrap_or_default() {
            builder = builder.lossy_preserve(path);
        }
        Ok(Self(builder.build().map_err(map_err)?))
    }

    #[getter]
    fn target_tokens(&self) -> Option<usize> {
        self.0.target_tokens
    }

    #[getter]
    fn preset(&self) -> PyPreset {
        self.0.preset.into()
    }

    #[getter]
    fn disable(&self) -> Vec<String> {
        self.0.disabled.clone()
    }

    #[getter]
    fn store_originals(&self) -> bool {
        self.0.store_originals
    }

    // Returned as `str`, not `PathBuf`: the storage type is `Option<PathBuf>`, but the
    // return-direction Python conversion for `PathBuf` is a needless risk to take on for a
    // getter when `str` is unambiguous and just as usable from the caller's side.
    #[getter]
    fn retrieval_store_path(&self) -> Option<String> {
        self.0
            .retrieval_store_path
            .as_ref()
            .map(|p| p.display().to_string())
    }

    #[getter]
    fn lossy(&self) -> Option<PyLossyPath> {
        self.0.lossy.map(PyLossyPath::from)
    }

    #[getter]
    fn lossy_ratio(&self) -> f64 {
        self.0.lossy_ratio
    }

    #[getter]
    fn lossy_preserve(&self) -> Vec<String> {
        self.0.lossy_preserve.clone()
    }
}

// ---------------------------------------------------------------------------------------
// CompressionReport / EstimatorInfo
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
    preset: String,
    #[pyo3(get)]
    format: String,
    #[pyo3(get)]
    task_scope: String,
    #[pyo3(get)]
    warnings: Vec<String>,
    /// Full report, structurally converted to a plain Python dict. The fields above are
    /// the subset promoted to first-class attributes;
    /// everything else -- `quality`, `budget`, `cache`, `retrieval`, `transforms`, etc. --
    /// is available here rather than modeled as another dozen pyclasses.
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
            preset: report.preset.clone(),
            format: report.format.clone(),
            task_scope: report.task_scope.clone(),
            warnings: report.warnings.iter().map(|w| w.message.clone()).collect(),
            raw,
        },
    )
}

// ---------------------------------------------------------------------------------------
// CompressionResult: a named return type rather than a `(payload, report)` tuple, so adding
// a field later doesn't break callers that unpack.
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
    #[getter]
    fn text(&self, py: Python<'_>) -> PyResult<String> {
        let bytes = self.payload.bind(py).as_bytes();
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|e| PyUnicodeDecodeError::new_err(e.to_string()))
    }

    fn saved_pct(&self, py: Python<'_>) -> f64 {
        self.report.borrow(py).savings_pct
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
// arguments, explicit keyword arguments winning -- the same precedence the CLI applies
// when a flag and a config file disagree.
// ---------------------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn effective_policy(
    policy: Option<&PyCompressionPolicy>,
    preset: Option<PresetArg>,
    target_tokens: Option<usize>,
    disable: Option<Vec<String>>,
    lossy: Option<LossyArg>,
    lossy_ratio: Option<f64>,
    lossy_preserve: Option<Vec<String>>,
    preview: bool,
) -> PyResult<CorePolicy> {
    let base = policy.map(|p| &p.0);
    let mut builder = CorePolicy::builder().preview(preview);

    let resolved_mode = match preset {
        Some(m) => m.resolve()?,
        None => base.map(|p| p.preset).unwrap_or(CorePreset::Balanced),
    };
    builder = builder.preset(resolved_mode);

    let resolved_target = target_tokens.or_else(|| base.and_then(|p| p.target_tokens));
    if let Some(t) = resolved_target {
        builder = builder.target_tokens(t);
    }

    let resolved_disable =
        disable.unwrap_or_else(|| base.map(|p| p.disabled.clone()).unwrap_or_default());
    for id in resolved_disable {
        builder = builder.disable(id);
    }

    let resolved_lossy = match lossy {
        Some(l) => Some(l.resolve()?),
        None => base.and_then(|p| p.lossy),
    };
    if let Some(l) = resolved_lossy {
        builder = builder.lossy(l);
    }
    let resolved_lossy_ratio = lossy_ratio.or_else(|| base.map(|p| p.lossy_ratio));
    if let Some(r) = resolved_lossy_ratio {
        builder = builder.lossy_ratio(r);
    }
    let resolved_lossy_preserve = lossy_preserve
        .unwrap_or_else(|| base.map(|p| p.lossy_preserve.clone()).unwrap_or_default());
    for path in resolved_lossy_preserve {
        builder = builder.lossy_preserve(path);
    }

    if let Some(p) = base {
        builder = builder.reserve_output_tokens(p.reserve_output_tokens);
        builder = builder.preserve_latest_user_message(p.preserve_latest_user_message);
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
    }
    builder.build().map_err(map_err)
}

/// A budget-constrained call must fail closed when only an inexact (heuristic) token
/// estimator is available, unless the caller opts in with `allow_heuristic_budget=True`.
/// That rule is enforced at this binding's boundary: `tokenfold_core::compress` doesn't
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
    preset: Option<PresetArg>,
    target_tokens: Option<usize>,
    disable: Option<Vec<String>>,
    lossy: Option<LossyArg>,
    lossy_ratio: Option<f64>,
    lossy_preserve: Option<Vec<String>>,
    allow_heuristic_budget: bool,
    dry_run: bool,
    encoding: Option<EncodingArg>,
    retrieval_store: Option<PathBuf>,
    retrieval_namespace: Option<String>,
    require_target: bool,
) -> PyResult<PyCompressionResult> {
    let bytes = payload.into_bytes();
    // `dry_run` (i.e. `inspect()`) must be side-effect-free for real, not merely in what it
    // RETURNS. Substituting the original payload back into the result afterwards -- the old
    // behavior -- left the underlying core run using the ordinary persistence policy, so an
    // `inspect()` against a `store_originals=True` policy still wrote the full payload to the
    // retrieval store. `preview` is the switch that actually stops the write, in core, before it
    // happens; the payload substitution below stays as the presentation half of the same
    // contract.
    let mut resolved_policy = effective_policy(
        policy,
        preset,
        target_tokens,
        disable,
        lossy,
        lossy_ratio,
        lossy_preserve,
        dry_run,
    )?;
    if let Some(encoding) = encoding {
        resolved_policy.encoding = encoding.resolve()?;
    }
    if retrieval_store.is_some() {
        resolved_policy.retrieval_store_path = retrieval_store;
    }
    if let Some(namespace) = retrieval_namespace {
        resolved_policy.retrieval_namespace = namespace;
    }
    if resolved_policy.lossy.is_some() {
        resolved_policy.pruning = Some(tokenfold_core::PruningPolicy {
            keep_ratio: Some(resolved_policy.lossy_ratio),
            preserve_paths: resolved_policy.lossy_preserve.clone(),
            retrieval_store: resolved_policy.retrieval_store_path.clone(),
            retrieval_namespace: Some(resolved_policy.retrieval_namespace.clone()),
        });
    }
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
    if require_target
        && out.report.budget.as_ref().is_some_and(|budget| {
            matches!(
                budget.status,
                tokenfold_core::report::BudgetStatus::BestEffort
                    | tokenfold_core::report::BudgetStatus::Unreachable
            )
        })
    {
        let receipt = report_to_py(py, &out.report)?;
        let error = BudgetUnmetError::new_err(format!(
            "token budget unmet: achieved {} tokens",
            out.report.compressed_tokens
        ));
        error.value(py).setattr("receipt", receipt)?;
        return Err(error);
    }
    let payload_override = if dry_run {
        Some(bytes.as_slice())
    } else {
        None
    };
    output_to_result(py, out, payload_override)
}

// ---------------------------------------------------------------------------------------
// Public functions
// ---------------------------------------------------------------------------------------

#[pyfunction]
#[pyo3(signature = (payload, *, format=None, preset=None, target_tokens=None, require_target=false, encoding=None, pruning=None))]
#[allow(clippy::too_many_arguments)]
fn compress(
    py: Python<'_>,
    payload: PayloadArg,
    format: Option<FormatArg>,
    preset: Option<PresetArg>,
    target_tokens: Option<usize>,
    require_target: bool,
    encoding: Option<EncodingArg>,
    pruning: Option<PyRef<'_, PyPruningPolicy>>,
) -> PyResult<PyCompressionResult> {
    if require_target && target_tokens.is_none() {
        return Err(ConfigError::new_err(
            "require_target requires target_tokens",
        ));
    }
    if pruning.is_some()
        && target_tokens.is_none()
        && pruning.as_ref().and_then(|p| p.keep_ratio).is_none()
    {
        return Err(ConfigError::new_err(
            "pruning requires target_tokens or keep_ratio",
        ));
    }
    let resolved_format = format
        .map(FormatArg::resolve)
        .transpose()?
        .unwrap_or(CoreFormat::Auto);
    let (ratio, preserve, store, namespace) =
        pruning.as_ref().map_or((None, None, None, None), |p| {
            (
                p.keep_ratio.or(Some(0.0)),
                Some(p.preserve_paths.clone()),
                p.retrieval_store.clone(),
                p.retrieval_namespace.clone(),
            )
        });
    run_compress(
        py,
        resolved_format,
        payload,
        None,
        preset,
        target_tokens,
        None,
        pruning.map(|_| LossyArg::Str("heuristic".to_string())),
        ratio,
        preserve,
        false,
        false,
        encoding,
        store,
        namespace,
        require_target,
    )
}

#[pyfunction]
#[pyo3(signature = (payload, *, format=None, preset=None, target_tokens=None, require_target=false, encoding=None, pruning=None))]
#[allow(clippy::too_many_arguments)]
fn inspect(
    py: Python<'_>,
    payload: PayloadArg,
    format: Option<FormatArg>,
    preset: Option<PresetArg>,
    target_tokens: Option<usize>,
    require_target: bool,
    encoding: Option<EncodingArg>,
    pruning: Option<PyRef<'_, PyPruningPolicy>>,
) -> PyResult<Py<PyCompressionReport>> {
    if require_target && target_tokens.is_none() {
        return Err(ConfigError::new_err(
            "require_target requires target_tokens",
        ));
    }
    if pruning.is_some()
        && target_tokens.is_none()
        && pruning.as_ref().and_then(|p| p.keep_ratio).is_none()
    {
        return Err(ConfigError::new_err(
            "pruning requires target_tokens or keep_ratio",
        ));
    }
    let resolved_format = format
        .map(FormatArg::resolve)
        .transpose()?
        .unwrap_or(CoreFormat::Auto);
    let (ratio, preserve, store, namespace) =
        pruning.as_ref().map_or((None, None, None, None), |p| {
            (
                p.keep_ratio.or(Some(0.0)),
                Some(p.preserve_paths.clone()),
                p.retrieval_store.clone(),
                p.retrieval_namespace.clone(),
            )
        });
    let result = run_compress(
        py,
        resolved_format,
        payload,
        None,
        preset,
        target_tokens,
        None,
        pruning.map(|_| LossyArg::Str("heuristic".to_string())),
        ratio,
        preserve,
        true,
        true,
        encoding,
        store,
        namespace,
        require_target,
    )?;
    Ok(result.report)
}
// ---------------------------------------------------------------------------------------
// retrieve: restores a payload previously persisted by `store_originals=True` or `lossy`,
// mirroring `tokenfold retrieve`: accepts a raw hash, legacy text marker, or serialized JSON
// `$tf_ref` marker through the `hash` argument. CompressionReport paths remain CLI-only.
// ---------------------------------------------------------------------------------------

#[pyfunction]
#[pyo3(signature = (payload, *, from_format="auto"))]
fn decode(py: Python<'_>, payload: PayloadArg, from_format: &str) -> PyResult<Py<PyBytes>> {
    let from = match from_format.to_ascii_lowercase().as_str() {
        "auto" => tokenfold_core::DecodeFormat::Auto,
        "json" => tokenfold_core::DecodeFormat::Json,
        "toon" => tokenfold_core::DecodeFormat::Toon,
        "text" => tokenfold_core::DecodeFormat::Text,
        value => {
            return Err(ConfigError::new_err(format!(
                "unknown decode format: {value:?}"
            )));
        }
    };
    let decoded = tokenfold_core::decode(&payload.into_bytes(), from).map_err(map_err)?;
    Ok(PyBytes::new(py, &decoded).unbind())
}

/// Restore bytes by raw SHA-256 hash, legacy text marker, or serialized JSON `$tf_ref` marker.
/// An explicit `namespace` overrides a namespace embedded in a marker.
#[pyfunction]
#[pyo3(signature = (reference, *, retrieval_store=None, namespace=None))]
fn retrieve(
    py: Python<'_>,
    reference: &Bound<'_, PyAny>,
    retrieval_store: Option<PathBuf>,
    namespace: Option<String>,
) -> PyResult<Py<PyBytes>> {
    let reference = match reference.extract::<String>() {
        Ok(value) => value,
        Err(_) => serde_json::to_string(&py_to_json(reference)?)
            .map_err(|error| InvalidInputError::new_err(error.to_string()))?,
    };
    let reference =
        tokenfold_core::retrieval_store::parse_retrieval_reference(&reference).map_err(map_err)?;
    let resolved_namespace = namespace
        .or(reference.namespace)
        .unwrap_or_else(|| "default".to_string());

    // Hash algorithm is hardcoded "sha256" here for the same reason every call site in
    // tokenfold-core is: it is the only implemented option (`RetrievalStore::open` rejects
    // "blake3" as a documented, not-yet-built scope cut).
    let store = RetrievalStore::open("filesystem", "sha256", retrieval_store).map_err(map_err)?;

    match store.retrieve(&reference.hash, &resolved_namespace) {
        RetrievalOutcome::Found(bytes) => Ok(PyBytes::new(py, &bytes).unbind()),
        RetrievalOutcome::Missing => Err(RetrievalError::new_err(format!(
            "no stored original found for hash {:?} in namespace {resolved_namespace:?}",
            reference.hash
        ))),
        RetrievalOutcome::Expired => Err(RetrievalError::new_err(format!(
            "stored original for hash {:?} in namespace {resolved_namespace:?} has expired",
            reference.hash
        ))),
    }
}

// ---------------------------------------------------------------------------------------
// Module registration
// ---------------------------------------------------------------------------------------

#[pymodule]
fn tokenfold(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyPreset>()?;
    m.add_class::<PyInputFormat>()?;
    m.add_class::<PyEncoding>()?;
    m.add_class::<PyStatus>()?;
    m.add_class::<PyPruningPolicy>()?;
    m.add_class::<PyEstimatorInfo>()?;
    m.add_class::<PyCompressionReport>()?;
    m.add_class::<PyCompressionResult>()?;

    m.add("TokenFoldError", py.get_type::<TokenFoldError>())?;
    m.add("InvalidInputError", py.get_type::<InvalidInputError>())?;
    m.add("SafetyError", py.get_type::<SafetyError>())?;
    m.add("EstimatorError", py.get_type::<EstimatorError>())?;
    m.add("ConfigError", py.get_type::<ConfigError>())?;
    m.add("InternalError", py.get_type::<InternalError>())?;
    m.add("RetrievalError", py.get_type::<RetrievalError>())?;
    m.add("BudgetUnmetError", py.get_type::<BudgetUnmetError>())?;

    m.add_function(wrap_pyfunction!(compress, m)?)?;
    m.add_function(wrap_pyfunction!(inspect, m)?)?;
    m.add_function(wrap_pyfunction!(decode, m)?)?;
    m.add_function(wrap_pyfunction!(retrieve, m)?)?;
    Ok(())
}
