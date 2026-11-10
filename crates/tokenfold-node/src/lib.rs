//! Off-thread session driving; only custom counts cross the JavaScript boundary.
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;
use serde::Deserialize;
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

fn error(name: &str) -> Error {
    Error::new(Status::GenericFailure, name)
}
fn fit_error(e: FitError) -> Error {
    error(match e {
        FitError::InvalidInput(_) => "InvalidInputError",
        FitError::Config(_) => "ConfigError",
        FitError::Safety => "SafetyError",
        FitError::Accounting(_) => "AccountingError",
        FitError::Cancelled => "CancelledError",
        _ => "InternalError",
    })
}
fn runtime_error(e: RuntimeError) -> Error {
    error(match e {
        RuntimeError::Config => "ConfigError",
        RuntimeError::Busy => "BusyError",
        RuntimeError::Closed => "ClosedError",
        RuntimeError::Cancelled => "CancelledError",
        _ => "ScorerError",
    })
}
fn counter(kind: &str, value: &str) -> Result<Box<dyn TokenCounter + Send>> {
    match kind {
        "encoding" => Ok(Box::new(EncodingCounter::new(value).map_err(fit_error)?)),
        "openai" => Ok(Box::new(ProviderCounter::openai(value).map_err(fit_error)?)),
        _ => Err(error("ConfigError")),
    }
}
#[napi]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
#[napi]
pub fn receipt_json(source: String) -> Result<String> {
    Receipt::from_json(&source)
        .and_then(|r| r.to_json())
        .map_err(fit_error)
}

#[derive(Default)]
struct Cancel {
    cancelled: AtomicBool,
    token: Mutex<Option<CancellationToken>>,
}
impl Cancel {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(token) = self.token.lock().unwrap().as_ref() {
            token.cancel();
        }
    }
    fn check(&self) -> Result<()> {
        if self.cancelled.load(Ordering::Acquire) {
            Err(error("CancelledError"))
        } else {
            Ok(())
        }
    }
}
#[napi]
#[derive(Default)]
pub struct NativeCancellation {
    inner: Arc<Cancel>,
}
#[napi]
impl NativeCancellation {
    #[napi(constructor)]
    pub fn new() -> Self {
        Self::default()
    }
    #[napi]
    pub fn cancel(&self) {
        self.inner.cancel();
    }
    #[napi(getter)]
    pub fn started(&self) -> bool {
        self.inner.token.lock().unwrap().is_some()
    }
}
struct ClientState {
    closed: bool,
    sessions: HashMap<u64, Arc<Cancel>>,
}
struct Client {
    state: Mutex<ClientState>,
    dispatcher: Option<ScorerDispatcher>,
    timeout: Duration,
}
impl Drop for Client {
    fn drop(&mut self) {
        for c in self.state.lock().unwrap().sessions.values() {
            c.cancel();
        }
        if let Some(d) = &self.dispatcher {
            d.close();
        }
    }
}
static NEXT: AtomicU64 = AtomicU64::new(1);
#[napi]
pub struct NativeClient {
    inner: Arc<Client>,
}
#[napi]
impl NativeClient {
    #[napi(constructor)]
    pub fn new(config: Option<String>, capacity: u32, timeout_ms: u32) -> Result<Self> {
        if capacity > 128 || !(1..=120000).contains(&timeout_ms) {
            return Err(error("ConfigError"));
        }
        let timeout = Duration::from_millis(timeout_ms.into());
        let dispatcher = config
            .map(|s| {
                let config = SelectRuntimeConfig::from_path(std::path::Path::new(&s))?;
                ScorerDispatcher::new(
                    config,
                    QueuePolicy {
                        max_pending_scores: capacity as usize,
                        wait_timeout: timeout,
                    },
                )
            })
            .transpose()
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
    #[napi]
    pub fn begin(
        &self,
        source: String,
        options: String,
        cancel: &NativeCancellation,
    ) -> Result<AsyncTask<Begin>> {
        let mut state = self.inner.state.lock().unwrap();
        if state.closed {
            return Err(error("ClosedError"));
        }
        cancel.inner.check()?;
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        state.sessions.insert(id, cancel.inner.clone());
        Ok(AsyncTask::new(Begin {
            source,
            options,
            owner: Some(Owner {
                client: self.inner.clone(),
                cancel: cancel.inner.clone(),
                id,
            }),
        }))
    }
    #[napi]
    pub fn close(&self) -> AsyncTask<Close> {
        let mut state = self.inner.state.lock().unwrap();
        state.closed = true;
        for c in state.sessions.values() {
            c.cancel();
        }
        AsyncTask::new(Close {
            client: self.inner.clone(),
        })
    }
    #[napi]
    pub fn warmup(&self, callback: ThreadsafeFunction<String, ()>) -> Result<()> {
        if self.inner.state.lock().unwrap().closed {
            return Err(error("ClosedError"));
        }
        let ticket = Arc::new(
            self.inner
                .dispatcher
                .as_ref()
                .ok_or_else(|| error("ConfigError"))?
                .warmup()
                .map_err(runtime_error)?,
        );
        let done = ticket.clone();
        ticket.on_complete(Box::new(move || {
            let result = done
                .try_take()
                .ok_or_else(|| error("InternalError"))
                .and_then(|c| c.outcome.map(|_| String::new()).map_err(runtime_error));
            callback.call(result, ThreadsafeFunctionCallMode::NonBlocking);
        }));
        Ok(())
    }
}
pub struct Close {
    client: Arc<Client>,
}
impl Task for Close {
    type Output = ();
    type JsValue = ();
    fn compute(&mut self) -> Result<()> {
        if let Some(d) = &self.client.dispatcher {
            d.close();
        }
        Ok(())
    }
    fn resolve(&mut self, _: Env, _: ()) -> Result<()> {
        Ok(())
    }
}
struct Owner {
    client: Arc<Client>,
    cancel: Arc<Cancel>,
    id: u64,
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.client.state.lock().unwrap().sessions.remove(&self.id);
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Options {
    target: u64,
    mode: BudgetMode,
    allow_selection: bool,
    counter_kind: String,
    counter_value: String,
}
pub struct Begin {
    source: String,
    options: String,
    owner: Option<Owner>,
}
impl Task for Begin {
    type Output = NativeFit;
    type JsValue = NativeFit;
    fn compute(&mut self) -> Result<NativeFit> {
        let owner = self.owner.take().ok_or_else(|| error("InternalError"))?;
        owner.cancel.check()?;
        let options: Options =
            serde_json::from_str(&self.options).map_err(|_| error("ConfigError"))?;
        let spec: FitInputSpec =
            serde_json::from_str(&self.source).map_err(|_| error("InvalidInputError"))?;
        let counter = if options.counter_kind == "custom" {
            None
        } else {
            Some(counter(&options.counter_kind, &options.counter_value)?)
        };
        let counter_id = counter
            .as_ref()
            .map(|c| c.id().to_owned())
            .unwrap_or(options.counter_value);
        let session = FitSession::new(
            spec.prepare().map_err(fit_error)?,
            FitOptions {
                target_tokens: options.target,
                budget_mode: options.mode,
                allow_selection: options.allow_selection,
                counter_id,
                select: owner
                    .client
                    .dispatcher
                    .as_ref()
                    .map(ScorerDispatcher::scorer_info),
            },
        )
        .map_err(fit_error)?;
        {
            let mut token = owner.cancel.token.lock().unwrap();
            *token = Some(session.cancellation_token());
            if owner.cancel.cancelled.load(Ordering::Acquire) {
                session.cancellation_token().cancel();
            }
        }
        let mut state = FitState {
            session,
            counter,
            ticket: None,
            pending_count: None,
            owner,
            action: String::new(),
        };
        state.action = state.step(None, None)?;
        Ok(NativeFit {
            state: Arc::new(Mutex::new(Some(state))),
        })
    }
    fn resolve(&mut self, _: Env, output: NativeFit) -> Result<NativeFit> {
        Ok(output)
    }
}
struct FitState {
    session: FitSession,
    counter: Option<Box<dyn TokenCounter + Send>>,
    ticket: Option<(u64, ScoreTicket)>,
    pending_count: Option<(u64, AccountingScope)>,
    owner: Owner,
    action: String,
}
impl FitState {
    fn step(&mut self, count: Option<String>, counter_id: Option<String>) -> Result<String> {
        let mut reply = if let Some(source) = count {
            let (candidate_id, scope) = self
                .pending_count
                .take()
                .ok_or_else(|| error("InternalError"))?;
            let count: TokenCount =
                serde_json::from_str(&source).map_err(|_| error("AccountingError"))?;
            Some(FitReply::Counted {
                candidate_id,
                scope,
                count,
                counter_id: counter_id.ok_or_else(|| error("AccountingError"))?,
            })
        } else {
            None
        };
        if let Some((id, ticket)) = self.ticket.as_ref() {
            let Some(c) = ticket.try_take() else {
                return Ok("{\"action\":\"score\"}".into());
            };
            reply = Some(c.into_reply(*id).map_err(runtime_error)?);
            self.ticket = None;
        }
        loop {
            self.owner.cancel.check()?;
            let action = if let Some(c) = self.counter.as_ref() {
                self.session.drive(c.as_ref(), reply.take())
            } else {
                self.session.advance(reply.take())
            }
            .map_err(fit_error)?;
            match action {
                FitAction::Count { candidate_id, request, .. } => {
                    self.pending_count = Some((candidate_id,request.scope));
                    return Ok(serde_json::json!({"action":"count","format":request.format,"scope":request.scope,"payload":payload(request.payload)}).to_string());
                }
                FitAction::Score { action_id,query,groups,.. } => {
                    let dispatcher = self.owner.client.dispatcher.as_ref().ok_or_else(|| error("InternalError"))?;
                    let request = OriginalScoringRequest { query, groups:groups.into_iter().map(|g| OriginalScoringGroup { id:g.id,text:g.scoring_text }).collect() };
                    match dispatcher.submit(request, self.session.cancellation_token(), Instant::now()+self.owner.client.timeout) {
                        Ok(ticket) => { self.ticket = Some((action_id,ticket)); return Ok("{\"action\":\"score\"}".into()); }
                        Err(e) => reply = Some(FitReply::ScoreFailed { action_id, failure:e.score_failure().map_err(runtime_error)? }),
                    }
                }
                FitAction::Done(o) => return Ok(serde_json::json!({"action":"done","payload":payload(o.payload),"receipt":o.receipt.to_json().map_err(fit_error)?}).to_string()),
            }
        }
    }
}
fn payload(p: RenderedPayload) -> serde_json::Value {
    match p {
        RenderedPayload::Text(s) => s.into(),
        RenderedPayload::Json(v) => v,
    }
}
#[napi]
pub struct NativeFit {
    state: Arc<Mutex<Option<FitState>>>,
}
#[napi]
impl NativeFit {
    #[napi(getter)]
    pub fn action(&self) -> Result<String> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .as_ref()
            .ok_or_else(|| error("ClosedError"))?
            .action
            .clone())
    }
    #[napi]
    pub fn check_cancelled(&self) -> Result<()> {
        self.state
            .lock()
            .unwrap()
            .as_ref()
            .ok_or_else(|| error("ClosedError"))?
            .session
            .cancellation_token()
            .check()
            .map_err(fit_error)
    }
    #[napi]
    pub fn step(&self, count: Option<String>, counter_id: Option<String>) -> AsyncTask<Step> {
        AsyncTask::new(Step {
            state: self.state.clone(),
            count,
            counter_id,
        })
    }
    #[napi]
    pub fn on_score(&self, callback: ThreadsafeFunction<String, ()>) -> Result<()> {
        let guard = self.state.lock().unwrap();
        let state = guard.as_ref().ok_or_else(|| error("ClosedError"))?;
        let ticket = &state
            .ticket
            .as_ref()
            .ok_or_else(|| error("InternalError"))?
            .1;
        ticket.on_complete(Box::new(move || {
            callback.call(Ok(String::new()), ThreadsafeFunctionCallMode::NonBlocking);
        }));
        Ok(())
    }
    #[napi]
    pub fn finish(&self) {
        self.state.lock().unwrap().take();
    }
}
pub struct Step {
    state: Arc<Mutex<Option<FitState>>>,
    count: Option<String>,
    counter_id: Option<String>,
}
impl Task for Step {
    type Output = String;
    type JsValue = String;
    fn compute(&mut self) -> Result<String> {
        self.state
            .lock()
            .unwrap()
            .as_mut()
            .ok_or_else(|| error("ClosedError"))?
            .step(self.count.take(), self.counter_id.take())
    }
    fn resolve(&mut self, _: Env, output: String) -> Result<String> {
        Ok(output)
    }
}

pub struct Count {
    kind: String,
    value: String,
    source: String,
}
impl Task for Count {
    type Output = String;
    type JsValue = String;
    fn compute(&mut self) -> Result<String> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            format: FitFormat,
            scope: AccountingScope,
            payload: serde_json::Value,
        }
        let input: Input =
            serde_json::from_str(&self.source).map_err(|_| error("AccountingError"))?;
        let payload = if input.scope == AccountingScope::RenderedText {
            RenderedPayload::Text(
                input
                    .payload
                    .as_str()
                    .ok_or_else(|| error("AccountingError"))?
                    .to_owned(),
            )
        } else {
            RenderedPayload::Json(input.payload)
        };
        let request = RenderedRequest {
            format: input.format,
            scope: input.scope,
            payload,
            core_transforms: vec![],
        };
        serde_json::to_string(
            &counter(&self.kind, &self.value)?
                .count(&request)
                .map_err(fit_error)?,
        )
        .map_err(|_| error("AccountingError"))
    }
    fn resolve(&mut self, _: Env, output: String) -> Result<String> {
        Ok(output)
    }
}
#[napi]
pub fn count(kind: String, value: String, source: String) -> AsyncTask<Count> {
    AsyncTask::new(Count {
        kind,
        value,
        source,
    })
}
