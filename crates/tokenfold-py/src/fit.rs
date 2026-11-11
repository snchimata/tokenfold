//! Resumable native fitting: only custom counts cross the host boundary.
use crate::{ConfigError, InternalError, InvalidInputError, SafetyError, TokenFoldError};
use pyo3::create_exception;
use pyo3::prelude::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokenfold_adapters::fit_request::{FitInputSpec, ProviderCounter};
use tokenfold_adapters::select_runtime::{
    OriginalScoringGroup, OriginalScoringRequest, QueuePolicy, RuntimeError, ScoreTicket,
    ScorerDispatcher, SelectRuntimeConfig,
};
use tokenfold_core::fit::*;

create_exception!(tokenfold, AccountingError, TokenFoldError);
create_exception!(tokenfold, BusyError, TokenFoldError);
create_exception!(tokenfold, ClosedError, TokenFoldError);
create_exception!(tokenfold, ScorerError, TokenFoldError);
create_exception!(tokenfold, CancelledError, TokenFoldError);

fn fit_error(error: FitError) -> PyErr {
    match error {
        FitError::InvalidInput(_) => InvalidInputError::new_err("invalid fit input"),
        FitError::Config(_) => ConfigError::new_err("invalid fit configuration"),
        FitError::Safety => SafetyError::new_err("secret-shaped content is not accepted by fit"),
        FitError::Accounting(_) => AccountingError::new_err("accounting failed"),
        FitError::Cancelled => CancelledError::new_err("fit cancelled after cleanup"),
        _ => InternalError::new_err("fit protocol or internal error"),
    }
}
fn runtime_error(error: RuntimeError) -> PyErr {
    match error {
        RuntimeError::Config => ConfigError::new_err("invalid Select runtime configuration"),
        RuntimeError::Busy => BusyError::new_err("scorer queue is full"),
        RuntimeError::Closed => ClosedError::new_err("client is closed"),
        RuntimeError::Cancelled => CancelledError::new_err("fit cancelled after cleanup"),
        _ => ScorerError::new_err("Select runtime failed"),
    }
}
#[derive(Default)]
struct Cancellation {
    cancelled: AtomicBool,
    sessions: Mutex<HashMap<u64, CancellationToken>>,
}
#[pyclass(name = "CancellationToken", frozen)]
#[derive(Default)]
pub struct PyCancellation {
    inner: Arc<Cancellation>,
}
#[pymethods]
impl PyCancellation {
    #[new]
    fn new() -> Self {
        Self::default()
    }
    fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::Release);
        for token in self.inner.sessions.lock().unwrap().values() {
            token.cancel();
        }
    }
    #[getter]
    fn cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }
}
struct ClientState {
    closed: bool,
    sessions: HashMap<u64, CancellationToken>,
}
struct Client {
    state: Mutex<ClientState>,
    dispatcher: Option<ScorerDispatcher>,
    timeout: Duration,
}
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
impl Drop for Client {
    fn drop(&mut self) {
        for token in self.state.lock().unwrap().sessions.values() {
            token.cancel();
        }
        if let Some(dispatcher) = &self.dispatcher {
            dispatcher.close();
        }
    }
}
#[pyclass(name = "_Client", frozen)]
struct PyClient {
    inner: Arc<Client>,
}
#[pymethods]
impl PyClient {
    #[new]
    #[pyo3(signature = (select=None, max_pending_scores=32, score_wait_timeout_ms=5000))]
    fn new(
        py: Python<'_>,
        select: Option<String>,
        max_pending_scores: usize,
        score_wait_timeout_ms: u64,
    ) -> PyResult<Self> {
        if max_pending_scores > 128 || !(1..=120000).contains(&score_wait_timeout_ms) {
            return Err(ConfigError::new_err("invalid scorer queue policy"));
        }
        let timeout = Duration::from_millis(score_wait_timeout_ms);
        let dispatcher = py
            .detach(move || {
                select
                    .map(|source| {
                        let config = SelectRuntimeConfig::from_path(std::path::Path::new(&source))?;
                        ScorerDispatcher::new(
                            config,
                            QueuePolicy {
                                max_pending_scores,
                                wait_timeout: timeout,
                            },
                        )
                    })
                    .transpose()
            })
            .map_err(runtime_error)?;
        Ok(Self {
            inner: Arc::new(Client {
                state: Mutex::new(ClientState {
                    closed: false,
                    sessions: HashMap::new(),
                }),
                dispatcher,
                timeout,
            }),
        })
    }
    #[allow(clippy::too_many_arguments)]
    fn begin(
        &self,
        py: Python<'_>,
        source: String,
        target: u64,
        mode: &str,
        allow_selection: bool,
        counter_kind: &str,
        counter_value: &str,
        cancel: &PyCancellation,
    ) -> PyResult<PyFit> {
        let mode = match mode {
            "strict" => BudgetMode::Strict,
            "best_effort" => BudgetMode::BestEffort,
            _ => return Err(ConfigError::new_err("invalid budget mode")),
        };
        let (kind, value) = (counter_kind.to_owned(), counter_value.to_owned());
        let inner = self.inner.clone();
        let cancellation = cancel.inner.clone();
        py.detach(move || {
            let mut state = inner.state.lock().unwrap();
            if state.closed {
                return Err(ClosedError::new_err("client is closed"));
            }
            if cancellation.cancelled.load(Ordering::Acquire) {
                return Err(CancelledError::new_err("fit cancelled"));
            }
            // No input/counter work under the client lock: independent fits never serialize Core.
            drop(state);
            let spec: FitInputSpec = serde_json::from_str(&source)
                .map_err(|_| InvalidInputError::new_err("invalid fit input"))?;
            let counter: Option<Box<dyn TokenCounter + Send>> = match kind.as_str() {
                "encoding" => Some(Box::new(EncodingCounter::new(&value).map_err(fit_error)?)),
                "openai" => Some(Box::new(
                    ProviderCounter::openai(&value).map_err(fit_error)?,
                )),
                "custom" => None,
                _ => return Err(ConfigError::new_err("invalid counter")),
            };
            let counter_id = counter
                .as_ref()
                .map(|c| c.id().to_string())
                .unwrap_or(value);
            let session = FitSession::new(
                spec.prepare().map_err(fit_error)?,
                FitOptions {
                    target_tokens: target,
                    budget_mode: mode,
                    allow_selection,
                    counter_id,
                    select: inner.dispatcher.as_ref().map(ScorerDispatcher::scorer_info),
                },
            )
            .map_err(fit_error)?;
            let token = session.cancellation_token();
            let id = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
            state = inner.state.lock().unwrap();
            if state.closed {
                return Err(ClosedError::new_err("client is closed"));
            }
            state.sessions.insert(id, token.clone());
            let mut tokens = cancellation.sessions.lock().unwrap();
            if cancellation.cancelled.load(Ordering::Acquire) {
                token.cancel();
            }
            tokens.insert(id, token);
            drop(tokens);
            drop(state);
            Ok(PyFit {
                state: Mutex::new(FitState {
                    session,
                    counter,
                    ticket: None,
                    pending_count: None,
                }),
                client: inner,
                cancellation,
                id,
            })
        })
    }
    fn warmup(&self, py: Python<'_>) -> PyResult<()> {
        py.detach(|| {
            if self.inner.state.lock().unwrap().closed {
                return Err(ClosedError::new_err("client is closed"));
            }
            self.inner
                .dispatcher
                .as_ref()
                .ok_or_else(|| ConfigError::new_err("no Select runtime configured"))?
                .warmup()
                .map_err(runtime_error)?
                .wait()
                .outcome
                .map_err(runtime_error)?;
            Ok(())
        })
    }
    fn close(&self, py: Python<'_>) {
        py.detach(|| {
            let mut state = self.inner.state.lock().unwrap();
            state.closed = true;
            for token in state.sessions.values() {
                token.cancel();
            }
            drop(state);
            if let Some(dispatcher) = &self.inner.dispatcher {
                dispatcher.close();
            }
        });
    }
}
struct FitState {
    session: FitSession,
    counter: Option<Box<dyn TokenCounter + Send>>,
    ticket: Option<(u64, ScoreTicket)>,
    pending_count: Option<(u64, AccountingScope)>,
}
#[pyclass(name = "_Fit", frozen)]
struct PyFit {
    state: Mutex<FitState>,
    client: Arc<Client>,
    cancellation: Arc<Cancellation>,
    id: u64,
}
impl Drop for PyFit {
    fn drop(&mut self) {
        self.client.state.lock().unwrap().sessions.remove(&self.id);
        self.cancellation.sessions.lock().unwrap().remove(&self.id);
    }
}
#[pymethods]
impl PyFit {
    fn check_cancelled(&self) -> PyResult<()> {
        self.state
            .lock()
            .unwrap()
            .session
            .cancellation_token()
            .check()
            .map_err(fit_error)
    }
    #[pyo3(signature = (count=None, counter_id=None))]
    fn step(
        &self,
        py: Python<'_>,
        count: Option<String>,
        counter_id: Option<String>,
    ) -> PyResult<String> {
        py.detach(|| {
            let mut state = self.state.lock().unwrap();
            let mut reply = if let Some(source) = count {
                let (candidate_id, scope) = state.pending_count.take().ok_or_else(|| InternalError::new_err("unexpected count"))?;
                let count: TokenCount = serde_json::from_str(&source).map_err(|_| AccountingError::new_err("invalid token count"))?;
                Some(FitReply::Counted { candidate_id, scope, count, counter_id: counter_id.ok_or_else(|| AccountingError::new_err("missing counter ID"))? })
            } else { None };
            if let Some((action_id, ticket)) = state.ticket.as_ref() {
                let Some(completion) = ticket.try_take() else { return Ok("{\"action\":\"score\"}".into()); };
                reply = Some(completion.into_reply(*action_id).map_err(runtime_error)?);
                state.ticket = None;
            }
            loop {
                let FitState { session, counter, .. } = &mut *state;
                let action = if let Some(counter) = counter { session.drive(counter.as_ref(), reply.take()) } else { session.advance(reply.take()) }.map_err(fit_error)?;
                match action {
                    FitAction::Count { candidate_id, request, .. } => {
                        state.pending_count = Some((candidate_id, request.scope));
                        return Ok(serde_json::json!({"action":"count", "format":request.format,"scope":request.scope,"payload":payload_value(request.payload)}).to_string());
                    }
                    FitAction::Score { action_id, query, groups, .. } => {
                        let dispatcher = self.client.dispatcher.as_ref().ok_or_else(|| InternalError::new_err("missing scorer"))?;
                        let request = OriginalScoringRequest { query, groups: groups.into_iter().map(|g| OriginalScoringGroup { id:g.id, text:g.scoring_text }).collect() };
                        match dispatcher.submit(request, state.session.cancellation_token(), Instant::now() + self.client.timeout) {
                            Ok(ticket) => { state.ticket = Some((action_id, ticket)); return Ok("{\"action\":\"score\"}".into()); }
                            Err(error) => { reply = Some(FitReply::ScoreFailed { action_id, failure: error.score_failure().map_err(runtime_error)? }); }
                        }
                    }
                    FitAction::Done(output) => return Ok(serde_json::json!({"action":"done","payload":payload_value(output.payload),"receipt":output.receipt.to_json().map_err(fit_error)?}).to_string()),
                }
            }
        })
    }
    fn wait_score(&self, py: Python<'_>) {
        // A private callback wait preserves the one-shot completion for step().
        let pair = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let wake = pair.clone();
        self.state
            .lock()
            .unwrap()
            .ticket
            .as_ref()
            .unwrap()
            .1
            .on_complete(Box::new(move || {
                *wake.0.lock().unwrap() = true;
                wake.1.notify_one();
            }));
        py.detach(move || {
            let mut ready = pair.0.lock().unwrap();
            while !*ready {
                ready = pair.1.wait(ready).unwrap();
            }
        });
    }
    fn on_score(&self, callback: Py<PyAny>) {
        if let Some((_, ticket)) = self.state.lock().unwrap().ticket.as_ref() {
            ticket.on_complete(Box::new(move || {
                Python::attach(|py| {
                    let _ = callback.call0(py);
                });
            }));
        }
    }
}
fn payload_value(payload: RenderedPayload) -> serde_json::Value {
    match payload {
        RenderedPayload::Text(text) => text.into(),
        RenderedPayload::Json(value) => value,
    }
}
#[pyfunction]
fn receipt_json(source: &str) -> PyResult<String> {
    Receipt::from_json(source)
        .and_then(|r| r.to_json())
        .map_err(fit_error)
}

fn counter(kind: &str, value: &str) -> PyResult<Box<dyn TokenCounter + Send>> {
    match kind {
        "encoding" => Ok(Box::new(EncodingCounter::new(value).map_err(fit_error)?)),
        "openai" => Ok(Box::new(ProviderCounter::openai(value).map_err(fit_error)?)),
        _ => Err(ConfigError::new_err("invalid counter")),
    }
}
#[pyfunction]
fn counter_id(py: Python<'_>, kind: &str, value: &str) -> PyResult<String> {
    py.detach(|| Ok(counter(kind, value)?.id().to_owned()))
}
#[pyfunction]
fn count(
    py: Python<'_>,
    kind: &str,
    value: &str,
    format: &str,
    scope: &str,
    payload: &str,
) -> PyResult<String> {
    py.detach(|| {
        let format: FitFormat = serde_json::from_value(format.into())
            .map_err(|_| AccountingError::new_err("invalid format"))?;
        let scope: AccountingScope = serde_json::from_value(scope.into())
            .map_err(|_| AccountingError::new_err("invalid scope"))?;
        let value_payload: serde_json::Value = serde_json::from_str(payload)
            .map_err(|_| AccountingError::new_err("invalid payload"))?;
        let payload = if scope == AccountingScope::RenderedText {
            RenderedPayload::Text(
                value_payload
                    .as_str()
                    .ok_or_else(|| AccountingError::new_err("invalid text"))?
                    .to_owned(),
            )
        } else {
            RenderedPayload::Json(value_payload)
        };
        let count = counter(kind, value)?
            .count(&RenderedRequest {
                format,
                scope,
                payload,
                core_transforms: vec![],
            })
            .map_err(fit_error)?;
        serde_json::to_string(&count).map_err(|_| AccountingError::new_err("invalid count"))
    })
}

pub fn register(py: Python<'_>, module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyClient>()?;
    module.add_class::<PyFit>()?;
    module.add_class::<PyCancellation>()?;
    module.add("AccountingError", py.get_type::<AccountingError>())?;
    module.add("BusyError", py.get_type::<BusyError>())?;
    module.add("ClosedError", py.get_type::<ClosedError>())?;
    module.add("ScorerError", py.get_type::<ScorerError>())?;
    module.add("CancelledError", py.get_type::<CancelledError>())?;
    module.add_function(wrap_pyfunction!(receipt_json, module)?)?;
    module.add_function(wrap_pyfunction!(counter_id, module)?)?;
    module.add_function(wrap_pyfunction!(count, module)?)?;
    Ok(())
}
