//! Approved, offline direct-child Select v3 runtime with nonblocking FIFO tickets.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicU64, Ordering},
    mpsc,
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tokenfold_core::fit::{
    CancellationToken, FitReply, GroupScore, ScoreFailure, ScoreMetadata, ScoreOutcome, ScorerInfo,
};
use tokenfold_core::retrieval_store::hex_sha256;

const REQUEST_LIMIT: usize = 1024 * 1024;
const RESPONSE_LIMIT: usize = 65536;
// -S prevents .pth/sitecustomize before this code; add dependency paths without site hooks.
const BOOTSTRAP: &str = r#"
import importlib.util,os,runpy,sys,sysconfig
sys.pycache_prefix=sys.argv.pop(1)
root=sys.argv.pop(1)
prefix=os.path.dirname(os.path.dirname(sys.executable))
scheme=sysconfig.get_default_scheme()
if os.path.isfile(os.path.join(prefix,'pyvenv.cfg')):
    sys.prefix=sys.exec_prefix=prefix
    scheme='venv'
else:
    prefix=sys.prefix
for kind in ('purelib','platlib'):
    path=sysconfig.get_path(kind,scheme=scheme,vars=dict(base=prefix,platbase=prefix))
    if os.path.isdir(path) and path not in sys.path:
        sys.path.append(path)
spec=importlib.util.spec_from_file_location('tokenfold_select_runtime',os.path.join(root,'__init__.py'),submodule_search_locations=[root])
module=importlib.util.module_from_spec(spec)
sys.modules[spec.name]=module
spec.loader.exec_module(module)
runpy.run_module('tokenfold_select_runtime',run_name='__main__')
"#;

#[test]
fn bootstrap_venv_ignores_captured_base_installation_scheme() {
    let setup = r#"
import os,pathlib,sys,sysconfig,tempfile
with tempfile.TemporaryDirectory() as directory:
    root=pathlib.Path(directory)
    (root/'pyvenv.cfg').touch()
    sys.executable=str(root/('Scripts/python.exe' if os.name=='nt' else 'bin/python'))
    site=pathlib.Path(sysconfig.get_path('purelib',scheme='venv',vars=dict(base=directory,platbase=directory)))
    site.mkdir(parents=True)
    (site/'bootstrap_dependency.py').write_text('VALUE=42\n')
    source=root/'source'
    source.mkdir()
    (source/'__init__.py').touch()
    (source/'__main__.py').write_text('from bootstrap_dependency import VALUE\nassert VALUE==42\n')
    # Reproduce a distro base default captured before -S venv prefix restoration.
    sysconfig._INSTALL_SCHEMES['base_fixture']=dict(sysconfig._INSTALL_SCHEMES['venv'],purelib='{base}/wrong-site',platlib='{platbase}/wrong-site')
    sysconfig.get_path.__defaults__=('base_fixture',None,True)
    bootstrap=sys.argv[1]
    sys.argv=['-c',str(root/'cache'),str(source)]
    exec(bootstrap)
"#;
    let output = Command::new("python")
        .args(["-I", "-S", "-B", "-c", setup, BOOTSTRAP])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

const SOURCE_FILES: &[&str] = &[
    "__init__.py",
    "__main__.py",
    "ettin.py",
    "integrity.py",
    "protocol.py",
    "provision.py",
    "worker.py",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeError {
    Config,
    Busy,
    Closed,
    Cancelled,
    QueueTimeout,
    SelectionLimit,
    SelectionLimitAttempted,
    Timeout,
    Failure,
    ResponseInvalid,
}
impl RuntimeError {
    pub fn attempted(self) -> bool {
        matches!(
            self,
            Self::SelectionLimitAttempted | Self::Timeout | Self::Failure | Self::ResponseInvalid
        )
    }
    pub fn score_failure(self) -> Result<ScoreFailure> {
        Ok(match self {
            Self::QueueTimeout => ScoreFailure::QueueTimeout,
            Self::SelectionLimit => ScoreFailure::SelectionLimit,
            Self::SelectionLimitAttempted => ScoreFailure::SelectionLimitAttempted,
            Self::Timeout => ScoreFailure::Timeout,
            Self::ResponseInvalid => ScoreFailure::ResponseInvalid,
            Self::Failure => ScoreFailure::Failure,
            other => return Err(other),
        })
    }
}
impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "select runtime: {self:?}")
    }
}
impl std::error::Error for RuntimeError {}
type Result<T> = std::result::Result<T, RuntimeError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Approval {
    python_executable: PathBuf,
    python_sha256: String,
    runtime_source: PathBuf,
    runtime_source_sha256: String,
    runtime_version: String,
    bundle: PathBuf,
    manifest_sha256: String,
    revision: String,
    device: String,
    precision: String,
    approved_by: String,
    scratch_root: PathBuf,
    startup_timeout_ms: u64,
    inference_timeout_ms: u64,
    cleanup_timeout_ms: u64,
}
#[derive(Debug, Clone)]
pub struct SelectRuntimeConfig(Approval);
impl SelectRuntimeConfig {
    pub fn from_path(path: &Path) -> Result<Self> {
        Self::from_json(&read_bounded(path, 16384)?)
    }
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > 16384 {
            return Err(RuntimeError::Config);
        }
        let a: Approval = serde_json::from_slice(bytes).map_err(|_| RuntimeError::Config)?;
        let hash = |s: &str| {
            s.len() == 64
                && s.bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        };
        if [
            &a.python_executable,
            &a.runtime_source,
            &a.bundle,
            &a.scratch_root,
        ]
        .iter()
        .any(|p| !p.is_absolute())
            || [
                &a.python_sha256,
                &a.runtime_source_sha256,
                &a.manifest_sha256,
                &a.revision,
            ]
            .iter()
            .any(|s| !hash(s))
            || a.runtime_version != "0.6.0"
            || a.approved_by.trim().is_empty()
            || a.approved_by.len() > 256
            || !matches!(
                (a.device.as_str(), a.precision.as_str()),
                ("cpu", "fp32") | ("cuda:0", "fp32-parameters-bf16-autocast")
            )
            || !(1..=120000).contains(&a.startup_timeout_ms)
            || !(1000..=30000).contains(&a.inference_timeout_ms)
            || !(1..=1000).contains(&a.cleanup_timeout_ms)
        {
            return Err(RuntimeError::Config);
        }
        let config = Self(a);
        config.validate_assets()?;
        Ok(config)
    }
    pub fn scorer_info(&self) -> ScorerInfo {
        ScorerInfo {
            revision: self.0.revision.clone(),
            manifest_hash: self.0.manifest_sha256.clone(),
            runtime_id: format!(
                "tokenfold-select-runtime/0.6.0:{}",
                self.0.runtime_source_sha256
            ),
            device: self.0.device.clone(),
            precision: self.0.precision.clone(),
            qualification: "unqualified".into(),
        }
    }
    fn validate_assets(&self) -> Result<()> {
        let a = &self.0;
        if hex_sha256(&read_bounded(&a.python_executable, 64 * 1024 * 1024)?) != a.python_sha256 {
            return Err(RuntimeError::Config);
        }
        let root = a
            .runtime_source
            .canonicalize()
            .map_err(|_| RuntimeError::Config)?;
        for entry in std::fs::read_dir(&root).map_err(|_| RuntimeError::Config)? {
            let entry = entry.map_err(|_| RuntimeError::Config)?;
            let name = entry.file_name();
            if !SOURCE_FILES.iter().any(|allowed| name == *allowed) && name != "__pycache__" {
                return Err(RuntimeError::Config);
            }
        }
        let mut bytes = vec![];
        for name in SOURCE_FILES {
            let path = root
                .join(name)
                .canonicalize()
                .map_err(|_| RuntimeError::Config)?;
            if !path.starts_with(&root) {
                return Err(RuntimeError::Config);
            }
            bytes.extend_from_slice(name.as_bytes());
            bytes.push(0);
            bytes.extend(read_bounded(&path, 256 * 1024)?);
            bytes.push(0);
        }
        if hex_sha256(&bytes) != a.runtime_source_sha256
            || !a.scratch_root.is_dir()
            || !a.bundle.is_dir()
        {
            return Err(RuntimeError::Config);
        }
        let manifest = read_bounded(&a.bundle.join("ettin-manifest.json"), 65536)?;
        if hex_sha256(&manifest) != a.manifest_sha256 {
            return Err(RuntimeError::Config);
        }
        let manifest: Value =
            serde_json::from_slice(&manifest).map_err(|_| RuntimeError::Config)?;
        if manifest.get("revision").and_then(Value::as_str) != Some(&a.revision) {
            return Err(RuntimeError::Config);
        }
        Ok(())
    }
}
fn read_bounded(path: &Path, cap: usize) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path).map_err(|_| RuntimeError::Config)?;
    let mut bytes = vec![];
    file.take(cap as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| RuntimeError::Config)?;
    if bytes.len() > cap {
        return Err(RuntimeError::Config);
    }
    Ok(bytes)
}
#[derive(Debug, Clone)]
pub struct QueuePolicy {
    pub max_pending_scores: usize,
    pub wait_timeout: Duration,
}
impl Default for QueuePolicy {
    fn default() -> Self {
        Self {
            max_pending_scores: 32,
            wait_timeout: Duration::from_millis(5000),
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct OriginalScoringRequest {
    pub query: String,
    pub groups: Vec<OriginalScoringGroup>,
}
#[derive(Debug, Clone, Serialize)]
pub struct OriginalScoringGroup {
    pub id: String,
    pub text: String,
}
impl OriginalScoringRequest {
    fn body(&self, revision: &str) -> Result<Value> {
        let mut ids = std::collections::HashSet::new();
        if self.query.trim().is_empty()
            || self.query.len() > 4096
            || self.groups.is_empty()
            || self.groups.len() > 512
            || self.groups.iter().any(|g| {
                g.id.is_empty() || g.id.len() > 256 || g.text.is_empty() || !ids.insert(&g.id)
            })
        {
            return Err(RuntimeError::SelectionLimit);
        }
        let value = json!({"schema_version":1,"model_revision":revision,"query":self.query,"groups":self.groups});
        let raw = serde_json::to_vec(&value).map_err(|_| RuntimeError::SelectionLimit)?;
        // Schema-1 Python validation includes default JSON spaces; bound that representation too.
        if raw.len() + (6 + 4 * self.groups.len()).max(128) > REQUEST_LIMIT {
            return Err(RuntimeError::SelectionLimit);
        }
        if tokenfold_core::transforms::redaction::contains_secret(&raw) {
            return Err(RuntimeError::Config);
        }
        Ok(value)
    }
}
#[derive(Debug, Clone)]
pub struct ScoreCompletion {
    pub outcome: Result<Vec<GroupScore>>,
    pub scorer: ScorerInfo,
    pub metadata: ScoreMetadata,
}
impl ScoreCompletion {
    pub fn into_reply(self, action_id: u64) -> Result<FitReply> {
        let outcome = match self.outcome {
            Ok(scores) => ScoreOutcome::Scored {
                scores,
                scorer: self.scorer,
            },
            Err(error) => ScoreOutcome::Failed(error.score_failure()?),
        };
        Ok(FitReply::ScoreCompleted {
            action_id,
            outcome,
            metadata: self.metadata,
        })
    }
}
struct TicketState {
    finished: bool,
    completion: Option<ScoreCompletion>,
    callback: Option<Box<dyn FnOnce() + Send>>,
}
struct TicketInner {
    state: Mutex<TicketState>,
    changed: Condvar,
    cancel: CancellationToken,
}
impl TicketInner {
    fn complete(&self, value: ScoreCompletion) {
        let callback = {
            let mut state = self.state.lock().unwrap();
            state.finished = true;
            state.completion = Some(value);
            self.changed.notify_all();
            state.callback.take()
        };
        if let Some(callback) = callback {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(callback));
        }
    }
}
pub struct ScoreTicket {
    inner: Arc<TicketInner>,
}
impl ScoreTicket {
    pub fn try_take(&self) -> Option<ScoreCompletion> {
        self.inner.state.lock().unwrap().completion.take()
    }
    /// Callback must only schedule host work; it runs on the dispatcher, never a Core executor.
    pub fn on_complete(&self, callback: Box<dyn FnOnce() + Send>) {
        let mut state = self.inner.state.lock().unwrap();
        if state.completion.is_some() {
            drop(state);
            callback();
        } else {
            state.callback = Some(callback);
        }
    }
    pub fn wait(&self) -> ScoreCompletion {
        let mut state = self.inner.state.lock().unwrap();
        loop {
            if let Some(value) = state.completion.take() {
                return value;
            }
            state = self.inner.changed.wait(state).unwrap();
        }
    }
    pub fn cancel(&self) {
        if !self.inner.state.lock().unwrap().finished {
            self.inner.cancel.cancel();
        }
    }
}
impl Drop for ScoreTicket {
    fn drop(&mut self) {
        self.cancel();
    }
}
struct Job {
    body: Option<Value>,
    ticket: Arc<TicketInner>,
    submitted: Instant,
    deadline: Instant,
}
struct Queue {
    jobs: VecDeque<Job>,
    active: bool,
    closing: bool,
    closed: bool,
}
struct Shared {
    queue: Mutex<Queue>,
    changed: Condvar,
    config: SelectRuntimeConfig,
    policy: QueuePolicy,
}
/// The resident process and its FIFO have one owner; this name exposes the synchronous host entry.
pub type ResidentScorer = ScorerDispatcher;
pub struct ScorerDispatcher {
    shared: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
    owner_thread_id: std::thread::ThreadId,
}
impl ScorerDispatcher {
    pub fn new(config: SelectRuntimeConfig, policy: QueuePolicy) -> Result<Self> {
        if policy.max_pending_scores > 128
            || Instant::now().checked_add(policy.wait_timeout).is_none()
        {
            return Err(RuntimeError::Config);
        }
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                jobs: VecDeque::new(),
                active: false,
                closing: false,
                closed: false,
            }),
            changed: Condvar::new(),
            config,
            policy,
        });
        let owner = shared.clone();
        let thread = std::thread::Builder::new()
            .name("tokenfold-select".into())
            .spawn(move || dispatch(owner))
            .map_err(|_| RuntimeError::Failure)?;
        let owner_thread_id = thread.thread().id();
        Ok(Self {
            shared,
            thread: Mutex::new(Some(thread)),
            owner_thread_id,
        })
    }
    pub fn scorer_info(&self) -> ScorerInfo {
        self.shared.config.scorer_info()
    }
    pub fn submit(
        &self,
        request: OriginalScoringRequest,
        cancel: CancellationToken,
        deadline: Instant,
    ) -> Result<ScoreTicket> {
        let body = request.body(&self.shared.config.0.revision)?;
        self.enqueue(Some(body), cancel, deadline)
    }
    /// Synchronous host callers only. Native async/Core/libuv bridges must use submit and tickets.
    pub fn score(
        &self,
        request: OriginalScoringRequest,
        cancel: CancellationToken,
    ) -> Result<Vec<GroupScore>> {
        self.submit(
            request,
            cancel,
            Instant::now() + self.shared.policy.wait_timeout,
        )?
        .wait()
        .outcome
    }
    pub fn warmup(&self) -> Result<ScoreTicket> {
        self.enqueue(
            None,
            CancellationToken::default(),
            Instant::now() + self.shared.policy.wait_timeout,
        )
    }
    fn enqueue(
        &self,
        body: Option<Value>,
        cancel: CancellationToken,
        deadline: Instant,
    ) -> Result<ScoreTicket> {
        if cancel.check().is_err() {
            return Err(RuntimeError::Cancelled);
        }
        let now = Instant::now();
        let mut queue = self.shared.queue.lock().unwrap();
        if queue.closing {
            return Err(RuntimeError::Closed);
        }
        let waiting = queue.jobs.len() + usize::from(queue.active);
        if waiting > self.shared.policy.max_pending_scores {
            return Err(RuntimeError::Busy);
        }
        let inner = Arc::new(TicketInner {
            state: Mutex::new(TicketState {
                finished: false,
                completion: None,
                callback: None,
            }),
            changed: Condvar::new(),
            cancel,
        });
        queue.jobs.push_back(Job {
            body,
            ticket: inner.clone(),
            submitted: now,
            deadline: deadline.min(now + self.shared.policy.wait_timeout),
        });
        self.shared.changed.notify_one();
        Ok(ScoreTicket { inner })
    }
    pub fn close(&self) {
        {
            let mut queue = self.shared.queue.lock().unwrap();
            queue.closing = true;
            self.shared.changed.notify_all();
        }
        if self.owner_thread_id == std::thread::current().id() {
            return;
        }
        let thread = { self.thread.lock().unwrap().take() };
        if let Some(thread) = thread {
            let _ = thread.join();
        } else {
            let mut queue = self.shared.queue.lock().unwrap();
            while !queue.closed {
                queue = self.shared.changed.wait(queue).unwrap();
            }
        }
    }
}
impl Drop for ScorerDispatcher {
    fn drop(&mut self) {
        self.close();
    }
}
fn completion(
    shared: &Shared,
    _job: &Job,
    outcome: Result<Vec<GroupScore>>,
    metadata: ScoreMetadata,
) -> ScoreCompletion {
    ScoreCompletion {
        outcome,
        scorer: shared.config.scorer_info(),
        metadata,
    }
}
fn prune(shared: &Shared, queue: &mut Queue) -> Vec<(Arc<TicketInner>, ScoreCompletion)> {
    let mut keep = VecDeque::new();
    let mut retired = vec![];
    while let Some(job) = queue.jobs.pop_front() {
        let error = if queue.closing {
            Some(RuntimeError::Closed)
        } else if job.ticket.cancel.check().is_err() {
            Some(RuntimeError::Cancelled)
        } else if Instant::now() >= job.deadline {
            Some(RuntimeError::QueueTimeout)
        } else {
            None
        };
        if let Some(error) = error {
            retired.push((
                job.ticket.clone(),
                completion(
                    shared,
                    &job,
                    Err(error),
                    ScoreMetadata {
                        score_queue_ms: Some(job.submitted.elapsed().as_secs_f64() * 1000.0),
                        ..Default::default()
                    },
                ),
            ));
        } else {
            keep.push_back(job);
        }
    }
    queue.jobs = keep;
    retired
}
fn dispatch(shared: Arc<Shared>) {
    let mut worker = None;
    loop {
        let (job, retired, closing) = {
            let mut queue = shared.queue.lock().unwrap();
            let retired = prune(&shared, &mut queue);
            let closing = queue.closing;
            let job = if closing {
                None
            } else {
                queue.jobs.pop_front()
            };
            if job.is_some() {
                queue.active = true;
            }
            if job.is_none() && retired.is_empty() && !closing {
                drop(
                    shared
                        .changed
                        .wait_timeout(queue, Duration::from_millis(2))
                        .unwrap(),
                );
                continue;
            }
            (job, retired, closing)
        };
        for (ticket, value) in retired {
            ticket.complete(value);
        }
        if closing {
            break;
        }
        let Some(job) = job else {
            continue;
        };
        let mut metadata = ScoreMetadata {
            score_queue_ms: Some(job.submitted.elapsed().as_secs_f64() * 1000.0),
            ..Default::default()
        };
        let start = Instant::now();
        let mut outcome = (|| {
            if worker.is_none() {
                worker = Some(Worker::spawn(&shared.config)?);
                worker
                    .as_mut()
                    .unwrap()
                    .load(&shared, &job, job.body.is_none())?;
                metadata.startup_ms = Some(start.elapsed().as_secs_f64() * 1000.0);
            } else if let Some(resident) = worker.as_mut()
                && job.body.is_none()
                && !resident.loaded
            {
                resident.load(&shared, &job, true)?;
                metadata.startup_ms = Some(start.elapsed().as_secs_f64() * 1000.0);
            }
            match &job.body {
                Some(body) => worker
                    .as_mut()
                    .unwrap()
                    .score(&shared, &job, body, &mut metadata),
                None => Ok(vec![]),
            }
        })();
        if shared.queue.lock().unwrap().closing {
            outcome = Err(RuntimeError::Closed);
        } else if job.ticket.cancel.check().is_err() {
            outcome = Err(RuntimeError::Cancelled);
        }
        if outcome.is_err() {
            if metadata.startup_ms.is_none() && worker.as_ref().is_some_and(|w| !w.loaded) {
                metadata.startup_ms = Some(start.elapsed().as_secs_f64() * 1000.0);
            }
            worker.take();
        }
        job.ticket
            .complete(completion(&shared, &job, outcome, metadata));
        let mut queue = shared.queue.lock().unwrap();
        queue.active = false;
        shared.changed.notify_all();
    }
    if let Some(mut worker) = worker {
        worker.close(&shared.config);
    }
    let retired = {
        let mut queue = shared.queue.lock().unwrap();
        prune(&shared, &mut queue)
    };
    for (ticket, value) in retired {
        ticket.complete(value);
    }
    let mut queue = shared.queue.lock().unwrap();
    queue.closed = true;
    shared.changed.notify_all();
}
struct Worker {
    child: Child,
    input: Option<mpsc::SyncSender<Vec<u8>>>,
    writer: Option<JoinHandle<()>>,
    frames: mpsc::Receiver<Result<Value>>,
    reader: Option<JoinHandle<()>>,
    directory: PathBuf,
    root: PathBuf,
    next: u64,
    loaded: bool,
    startup_deadline: Instant,
}
impl Worker {
    fn spawn(config: &SelectRuntimeConfig) -> Result<Self> {
        let startup_deadline = Instant::now() + Duration::from_millis(config.0.startup_timeout_ms);
        config.validate_assets()?;
        let a = &config.0;
        let root = a
            .scratch_root
            .canonicalize()
            .map_err(|_| RuntimeError::Config)?;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = root.join(format!(
            "tokenfold-resident-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(0o700);
            builder
        };
        #[cfg(not(unix))]
        let builder = std::fs::DirBuilder::new();
        builder
            .create(&directory)
            .map_err(|_| RuntimeError::Config)?;
        let mut command = Command::new(&a.python_executable);
        command
            .args(["-I", "-S", "-B", "-u", "-c", BOOTSTRAP])
            .arg(directory.join("unwritten-bytecode"))
            .arg(&a.runtime_source)
            .arg("worker")
            .env_clear()
            .env("HF_HUB_OFFLINE", "1")
            .env("TRANSFORMERS_OFFLINE", "1")
            .env("CUDA_VISIBLE_DEVICES", "0")
            .env("TOKENIZERS_PARALLELISM", "false")
            .env("USERNAME", "tokenfold-worker")
            .env("TEMP", &directory)
            .env("TMP", &directory)
            .env("HOME", &directory)
            .env("HF_HOME", &directory)
            .current_dir(&directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
            if let Some(root) = system_windows_directory() {
                command.env("SystemRoot", &root).env("WINDIR", root);
            }
        }
        let child = match command.spawn() {
            Ok(child) => child,
            Err(_) => {
                let _ = std::fs::remove_dir(&directory);
                return Err(RuntimeError::Failure);
            }
        };
        let (sender, frames) = mpsc::sync_channel(4);
        // Own the child before fallible helper-thread creation, so every error reaps it.
        let mut worker = Self {
            child,
            input: None,
            writer: None,
            frames,
            reader: None,
            directory,
            root,
            next: 0,
            loaded: false,
            startup_deadline,
        };
        let mut input = worker.child.stdin.take().unwrap();
        let (writes, pending) = mpsc::sync_channel::<Vec<u8>>(1);
        worker.writer = Some(
            std::thread::Builder::new()
                .name("tokenfold-select-writer".into())
                .spawn(move || {
                    while let Ok(raw) = pending.recv() {
                        if input
                            .write_all(&(raw.len() as u32).to_le_bytes())
                            .and_then(|_| input.write_all(&raw))
                            .and_then(|_| input.flush())
                            .is_err()
                        {
                            break;
                        }
                    }
                })
                .map_err(|_| RuntimeError::Failure)?,
        );
        worker.input = Some(writes);
        let mut output = worker.child.stdout.take().unwrap();
        worker.reader = Some(
            std::thread::Builder::new()
                .name("tokenfold-select-reader".into())
                .spawn(move || {
                    loop {
                        let frame = read_frame(&mut output);
                        let failed = frame.is_err();
                        if sender.send(frame).is_err() || failed {
                            break;
                        }
                    }
                })
                .map_err(|_| RuntimeError::Failure)?,
        );
        Ok(worker)
    }
    fn send(&mut self, op: &str, body: &Value) -> Result<u64> {
        self.next += 1;
        let raw = serde_json::to_vec(
            &json!({"protocol_version":2,"request_id":self.next,"op":op,"body":body}),
        )
        .map_err(|_| RuntimeError::ResponseInvalid)?;
        if raw.len() > if op == "score" { REQUEST_LIMIT } else { 4096 } {
            return Err(RuntimeError::SelectionLimitAttempted);
        }
        self.input
            .as_ref()
            .ok_or(RuntimeError::Failure)?
            .try_send(raw)
            .map_err(|_| RuntimeError::Failure)?;
        Ok(self.next)
    }
    fn receive(&mut self, shared: &Shared, job: &Job, id: u64, deadline: Instant) -> Result<Value> {
        loop {
            let (closing, retired) = {
                let mut queue = shared.queue.lock().unwrap();
                let retired = prune(shared, &mut queue);
                (queue.closing, retired)
            };
            for (ticket, value) in retired {
                ticket.complete(value);
            }
            if closing {
                return Err(RuntimeError::Closed);
            }
            if job.ticket.cancel.check().is_err() {
                return Err(RuntimeError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(RuntimeError::Timeout);
            }
            match self.frames.recv_timeout(Duration::from_millis(2)) {
                Ok(Ok(frame)) => {
                    if frame.get("protocol_version").and_then(Value::as_u64) != Some(2)
                        || frame.get("request_id").and_then(Value::as_u64) != Some(id)
                        || frame.as_object().is_none_or(|m| m.len() != 4)
                    {
                        return Err(RuntimeError::ResponseInvalid);
                    }
                    if frame.get("op").and_then(Value::as_str) == Some("error") {
                        if frame["body"].as_object().is_none_or(|body| body.len() != 1) {
                            return Err(RuntimeError::ResponseInvalid);
                        }
                        return Err(match frame.pointer("/body/code").and_then(Value::as_str) {
                            Some("selection_limit") => RuntimeError::SelectionLimitAttempted,
                            Some("failure") => RuntimeError::Failure,
                            _ => RuntimeError::ResponseInvalid,
                        });
                    }
                    return Ok(frame);
                }
                Ok(Err(error)) => return Err(error),
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(RuntimeError::Failure),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
    fn load(&mut self, shared: &Shared, job: &Job, warmup: bool) -> Result<()> {
        let id = self.send("load", &json!({"config":shared.config.0,"warmup":warmup}))?;
        let frame = self.receive(shared, job, id, self.startup_deadline)?;
        if frame["op"] != "ready"
            || frame["body"] != json!({"phase":if warmup{"loaded"}else{"initialized"}})
        {
            return Err(RuntimeError::ResponseInvalid);
        }
        self.loaded = warmup;
        Ok(())
    }
    fn score(
        &mut self,
        shared: &Shared,
        job: &Job,
        body: &Value,
        metadata: &mut ScoreMetadata,
    ) -> Result<Vec<GroupScore>> {
        let start = Instant::now();
        let id = self.send("score", body)?;
        let was_loaded = self.loaded;
        let mut deadline = if was_loaded {
            start + Duration::from_millis(shared.config.0.inference_timeout_ms)
        } else {
            self.startup_deadline
        };
        let admitted = self.receive(shared, job, id, deadline)?;
        if admitted["op"] != "ready" || admitted["body"] != json!({"phase":"admitted"}) {
            return Err(RuntimeError::ResponseInvalid);
        }
        let loaded = self.receive(shared, job, id, deadline);
        if !was_loaded {
            metadata.startup_ms =
                Some(metadata.startup_ms.unwrap_or(0.0) + start.elapsed().as_secs_f64() * 1000.0);
        }
        let loaded = loaded?;
        if loaded["op"] != "ready" || loaded["body"] != json!({"phase":"loaded"}) {
            return Err(RuntimeError::ResponseInvalid);
        }
        self.loaded = true;
        let inference = Instant::now();
        if !was_loaded {
            deadline = inference + Duration::from_millis(shared.config.0.inference_timeout_ms);
        }
        let response = self.receive(shared, job, id, deadline);
        metadata.scoring_ms = Some(inference.elapsed().as_secs_f64() * 1000.0);
        let response = response?;
        if response["op"] != "scores" {
            return Err(RuntimeError::ResponseInvalid);
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Scores {
            schema_version: u32,
            model_revision: String,
            scores: Vec<(String, f64)>,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Diagnostics {
            outside_training_group_counts: bool,
            cross_window_context_unqualified: bool,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Body {
            response: Scores,
            diagnostics: Diagnostics,
        }
        let result: Body = serde_json::from_value(response["body"].clone())
            .map_err(|_| RuntimeError::ResponseInvalid)?;
        let groups = body["groups"]
            .as_array()
            .ok_or(RuntimeError::ResponseInvalid)?;
        if result.response.schema_version != 1
            || result.response.model_revision != shared.config.0.revision
            || result.response.scores.len() != groups.len()
            || result
                .response
                .scores
                .iter()
                .zip(groups)
                .any(|((id, score), g)| g["id"].as_str() != Some(id) || !score.is_finite())
        {
            return Err(RuntimeError::ResponseInvalid);
        }
        metadata.outside_training_group_counts = result.diagnostics.outside_training_group_counts;
        metadata.cross_window_context_unqualified =
            result.diagnostics.cross_window_context_unqualified;
        Ok(result
            .response
            .scores
            .into_iter()
            .map(|(id, score)| GroupScore { id, score })
            .collect())
    }
    fn close(&mut self, config: &SelectRuntimeConfig) {
        if self.send("close", &json!({})).is_ok() {
            let until = Instant::now() + Duration::from_millis(config.0.cleanup_timeout_ms);
            while Instant::now() < until {
                if self.child.try_wait().ok().flatten().is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.input.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
        // Disconnect the bounded reader before joining, so malicious extra frames cannot deadlock cleanup.
        let (_, empty) = mpsc::channel();
        let receiver = std::mem::replace(&mut self.frames, empty);
        drop(receiver);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        // Remove only this unique owned directory after canonical confinement; never enumerate scratch siblings.
        if self.directory.parent() == Some(self.root.as_path())
            && self.directory.canonicalize().ok().as_ref() == Some(&self.directory)
        {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }
}
fn read_frame(output: &mut impl Read) -> Result<Value> {
    let mut prefix = [0; 4];
    output
        .read_exact(&mut prefix)
        .map_err(|_| RuntimeError::Failure)?;
    let size = u32::from_le_bytes(prefix) as usize;
    if size == 0 || size > RESPONSE_LIMIT {
        return Err(RuntimeError::ResponseInvalid);
    }
    let mut raw = vec![0; size];
    output
        .read_exact(&mut raw)
        .map_err(|_| RuntimeError::Failure)?;
    // Typed derive rejects duplicate/unknown keys without imposing receipt integer limits on finite scores.
    #[derive(Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct Envelope {
        protocol_version: u32,
        request_id: u64,
        op: String,
        body: Body,
    }
    #[derive(Deserialize, Serialize)]
    #[serde(untagged)]
    enum Body {
        Ready(Ready),
        Scores(Scores),
        Error(Error),
        Closed(Closed),
    }
    #[derive(Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct Ready {
        phase: String,
    }
    #[derive(Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct Error {
        code: String,
    }
    #[derive(Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct Closed {}
    #[derive(Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct Scores {
        response: Response,
        diagnostics: Diagnostics,
    }
    #[derive(Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct Response {
        schema_version: u32,
        model_revision: String,
        scores: Vec<(String, f64)>,
    }
    #[derive(Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct Diagnostics {
        outside_training_group_counts: bool,
        cross_window_context_unqualified: bool,
    }
    let typed: Envelope =
        serde_json::from_slice(&raw).map_err(|_| RuntimeError::ResponseInvalid)?;
    let value = serde_json::to_value(typed).map_err(|_| RuntimeError::ResponseInvalid)?;
    if value["op"] != "scores" && size > 4096 {
        return Err(RuntimeError::ResponseInvalid);
    }
    Ok(value)
}
#[cfg(windows)]
fn system_windows_directory() -> Option<PathBuf> {
    unsafe extern "system" {
        fn GetSystemWindowsDirectoryW(buffer: *mut u16, size: u32) -> u32;
    }
    let mut buffer = [0u16; 32768];
    let length =
        unsafe { GetSystemWindowsDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    (length > 0 && length < buffer.len())
        .then(|| PathBuf::from(String::from_utf16_lossy(&buffer[..length])))
}
