use serde_json::json;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
use tokenfold_adapters::select_runtime::*;
use tokenfold_core::{fit::CancellationToken, retrieval_store::hex_sha256};
const FAKE: &str = include_str!("resident_fake.py");
struct Fixture {
    root: PathBuf,
    config: SelectRuntimeConfig,
}
impl Fixture {
    fn new(mode: &str, startup: u64) -> Self {
        Self::with_inference(mode, startup, 1000)
    }
    fn with_inference(mode: &str, startup: u64, inference: u64) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let scratch = repo.join(".superpowers/sdd/sdk-updates/runtime-scratch");
        std::fs::create_dir_all(&scratch).unwrap();
        let root = scratch.join(format!(
            "rust-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let source = root.join("src/tokenfold_select_runtime");
        std::fs::create_dir_all(&source).unwrap();
        let names = [
            "__init__.py",
            "__main__.py",
            "ettin.py",
            "integrity.py",
            "protocol.py",
            "provision.py",
            "worker.py",
        ];
        let mut data = vec![];
        for name in names {
            let raw = if name == "__main__.py" {
                FAKE.as_bytes()
            } else {
                b"# fixture\n"
            };
            std::fs::write(source.join(name), raw).unwrap();
            data.extend_from_slice(name.as_bytes());
            data.push(0);
            data.extend_from_slice(raw);
            data.push(0);
        }
        std::fs::write(root.join("mode"), mode).unwrap();
        let revision = "a".repeat(64);
        let manifest = serde_json::to_vec(&json!({"revision":revision})).unwrap();
        std::fs::write(root.join("ettin-manifest.json"), &manifest).unwrap();
        let python = std::process::Command::new("python")
            .args(["-c", "import sys; print(sys.executable)"])
            .output()
            .unwrap();
        let python = PathBuf::from(String::from_utf8(python.stdout).unwrap().trim())
            .canonicalize()
            .unwrap();
        let value = json!({"python_executable":python,"python_sha256":hex_sha256(&std::fs::read(&python).unwrap()),"runtime_source":source,"runtime_source_sha256":hex_sha256(&data),"runtime_version":"0.6.0","bundle":root,"manifest_sha256":hex_sha256(&manifest),"revision":revision,"device":"cpu","precision":"fp32","approved_by":"test","scratch_root":root,"startup_timeout_ms":startup,"inference_timeout_ms":inference,"cleanup_timeout_ms":100});
        std::fs::write(
            root.join("approval.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        let config = SelectRuntimeConfig::from_json(&serde_json::to_vec(&value).unwrap()).unwrap();
        Self { root, config }
    }
    fn dispatcher(&self, max: usize) -> ScorerDispatcher {
        ScorerDispatcher::new(
            self.config.clone(),
            QueuePolicy {
                max_pending_scores: max,
                wait_timeout: Duration::from_secs(5),
            },
        )
        .unwrap()
    }
    fn pids(&self) -> Vec<u32> {
        std::fs::read_to_string(self.root.join("pids"))
            .unwrap_or_default()
            .lines()
            .map(|s| s.split(':').next().unwrap().parse().unwrap())
            .collect()
    }
    fn wait_file(&self, name: &str) {
        let until = Instant::now() + Duration::from_secs(5);
        while !self.root.join(name).exists() {
            assert!(Instant::now() < until, "missing {name}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn assert_reaped(&self) {
        for line in std::fs::read_to_string(self.root.join("pids"))
            .unwrap_or_default()
            .lines()
        {
            let (pid, birth) = line.split_once(':').unwrap();
            let pid = pid.parse().unwrap();
            let birth = birth.parse().unwrap();
            assert!(!same_process_alive(pid, birth), "leaked child {pid}");
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            self.assert_reaped();
        }
        let expected = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.superpowers/sdd/sdk-updates/runtime-scratch")
            .canonicalize()
            .unwrap();
        let path = self.root.canonicalize().unwrap();
        assert_eq!(path.parent(), Some(expected.as_path()));
        std::fs::remove_dir_all(path).unwrap();
    }
}
#[cfg(windows)]
fn alive(pid: u32) -> bool {
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
        fn GetExitCodeProcess(process: isize, code: *mut u32) -> i32;
        fn CloseHandle(process: isize) -> i32;
    }
    unsafe {
        let handle = OpenProcess(0x1000, 0, pid);
        if handle == 0 {
            return false;
        }
        let mut code = 0;
        let ok = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        ok != 0 && code == 259
    }
}
#[cfg(windows)]
fn same_process_alive(pid: u32, birth: u64) -> bool {
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
        fn GetProcessTimes(
            handle: isize,
            created: *mut u64,
            exited: *mut u64,
            kernel: *mut u64,
            user: *mut u64,
        ) -> i32;
        fn CloseHandle(handle: isize) -> i32;
    }
    unsafe {
        let handle = OpenProcess(0x1000, 0, pid);
        if handle == 0 {
            return false;
        }
        let (mut c, mut e, mut k, mut u) = (0, 0, 0, 0);
        let ok = GetProcessTimes(handle, &mut c, &mut e, &mut k, &mut u);
        CloseHandle(handle);
        ok != 0 && c == birth && alive(pid)
    }
}
#[cfg(unix)]
fn same_process_alive(pid: u32, _birth: u64) -> bool {
    alive(pid)
}
#[cfg(unix)]
fn alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}
fn request(query: &str) -> OriginalScoringRequest {
    OriginalScoringRequest {
        query: query.into(),
        groups: vec![OriginalScoringGroup {
            id: "a".into(),
            text: "whole group".into(),
        }],
    }
}
fn submit(d: &ScorerDispatcher, q: &str) -> ScoreTicket {
    d.submit(
        request(q),
        CancellationToken::default(),
        Instant::now() + Duration::from_secs(5),
    )
    .unwrap()
}
#[test]
fn queue_policy_has_bounded_defaults() {
    let p = QueuePolicy::default();
    assert_eq!(p.max_pending_scores, 32);
    assert_eq!(p.wait_timeout.as_millis(), 5000);
    assert!(!RuntimeError::SelectionLimit.attempted());
    assert!(RuntimeError::SelectionLimitAttempted.attempted());
}
#[test]
fn two_scores_and_coalesced_warmup_share_a_real_child() {
    let f = Fixture::new("normal", 3000);
    let d = f.dispatcher(32);
    let a = d.warmup();
    let b = d.warmup();
    a.unwrap().wait().outcome.unwrap();
    b.unwrap().wait().outcome.unwrap();
    for _ in 0..2 {
        let result = submit(&d, "normal").wait();
        assert_eq!(result.outcome.unwrap()[0].score, 1.25);
        assert!(result.metadata.outside_training_group_counts);
        assert!(result.metadata.scoring_ms.is_some());
    }
    d.close();
    d.close();
    assert_eq!(f.pids().len(), 1);
    f.assert_reaped();
    assert!(matches!(d.warmup(), Err(RuntimeError::Closed)));
    let trace = std::fs::read_to_string(f.root.join("trace")).unwrap();
    assert_eq!(
        trace.lines().collect::<Vec<_>>(),
        ["1:load:", "2:score:normal", "3:score:normal", "4:close:"]
    );
}
#[test]
fn invalid_frames_invalidate_and_next_explicit_fit_cold_starts() {
    let f = Fixture::new("normal", 3000);
    let d = f.dispatcher(32);
    for q in [
        "nan",
        "oversize",
        "stale",
        "id_mismatch",
        "revision",
        "duplicate",
    ] {
        assert_eq!(
            submit(&d, q).wait().outcome,
            Err(RuntimeError::ResponseInvalid)
        );
        f.assert_reaped();
    }
    assert_eq!(
        submit(&d, "limit").wait().outcome,
        Err(RuntimeError::SelectionLimitAttempted)
    );
    d.close();
    assert_eq!(f.pids().len(), 7);
}
#[test]
fn cold_load_has_separate_budget_from_inference() {
    let f = Fixture::new("normal", 3000);
    let d = f.dispatcher(32);
    let result = submit(&d, "cold_load").wait();
    assert!(result.outcome.is_ok());
    assert!(result.metadata.startup_ms.unwrap() >= 1200.0);
    assert!(result.metadata.scoring_ms.unwrap() < 1000.0);
    assert_eq!(
        submit(&d, "inference_timeout").wait().outcome,
        Err(RuntimeError::Timeout)
    );
    d.close();
    f.assert_reaped();
}
#[test]
fn startup_timeout_and_stopped_reader_are_killed_and_waited() {
    for mode in ["startup_timeout", "stopped_reader"] {
        let f = Fixture::new(mode, 200);
        let d = f.dispatcher(32);
        let mut req = request("normal");
        req.groups[0].text = "x".repeat(800000);
        let start = Instant::now();
        let t = d
            .submit(
                req,
                CancellationToken::default(),
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(t.wait().outcome, Err(RuntimeError::Timeout));
        assert!(start.elapsed() < Duration::from_secs(3));
        d.close();
        f.assert_reaped();
    }
}
#[test]
fn full_queue_waiter_cancel_and_expiry_do_not_touch_active_child() {
    let f = Fixture::new("normal", 3000);
    let d = f.dispatcher(32);
    let active = submit(&d, "hold");
    f.wait_file("active");
    let mut waiters = vec![];
    for i in 0..32 {
        waiters.push(submit(&d, &format!("queued{i}")));
    }
    assert!(matches!(
        d.submit(
            request("busy"),
            CancellationToken::default(),
            Instant::now() + Duration::from_secs(5)
        ),
        Err(RuntimeError::Busy)
    ));
    waiters[0].cancel();
    assert_eq!(waiters[0].wait().outcome, Err(RuntimeError::Cancelled));
    let expiry = d
        .submit(
            request("expired"),
            CancellationToken::default(),
            Instant::now() + Duration::from_millis(30),
        )
        .unwrap();
    assert_eq!(expiry.wait().outcome, Err(RuntimeError::QueueTimeout));
    assert!(active.try_take().is_none());
    assert!(alive(f.pids()[0]));
    active.cancel();
    assert_eq!(active.wait().outcome, Err(RuntimeError::Cancelled));
    drop(waiters);
    d.close();
    let trace = std::fs::read_to_string(f.root.join("trace")).unwrap();
    assert!(!trace.contains("queued0"));
    assert!(!trace.contains("expired"));
    assert!(!trace.contains("busy"));
    f.assert_reaped();
}
#[test]
fn close_wakes_active_waiters_and_preserves_caller_scratch() {
    let f = Fixture::new("normal", 3000);
    let d = f.dispatcher(32);
    std::fs::write(f.root.join("caller-owned"), "keep").unwrap();
    let active = submit(&d, "hold");
    f.wait_file("active");
    let waiter = submit(&d, "waiter");
    let start = Instant::now();
    d.close();
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(active.wait().outcome, Err(RuntimeError::Closed));
    assert_eq!(waiter.wait().outcome, Err(RuntimeError::Closed));
    assert_eq!(
        std::fs::read_to_string(f.root.join("caller-owned")).unwrap(),
        "keep"
    );
    f.assert_reaped();
}
#[test]
fn oversized_native_admission_never_starts_worker() {
    let f = Fixture::new("normal", 3000);
    let d = f.dispatcher(0);
    let mut r = request("x");
    r.groups[0].text = "x".repeat(1048576);
    assert!(matches!(
        d.submit(
            r,
            CancellationToken::default(),
            Instant::now() + Duration::from_secs(5)
        ),
        Err(RuntimeError::SelectionLimit)
    ));
    assert!(f.pids().is_empty());
    d.close();
}
#[test]
fn approval_drift_before_boot_is_not_a_scorer_attempt() {
    let f = Fixture::new("normal", 3000);
    let d = f.dispatcher(32);
    std::fs::write(
        f.root.join("src/tokenfold_select_runtime/worker.py"),
        "# changed source\n",
    )
    .unwrap();
    let outcome = submit(&d, "normal").wait().outcome;
    assert_eq!(outcome, Err(RuntimeError::Config));
    assert!(!outcome.unwrap_err().attempted());
    assert!(f.pids().is_empty());
    d.close();
}
#[test]
fn dropped_waiter_is_removed_and_core_fit_completes_while_queue_full() {
    use tokenfold_core::fit::*;
    let f = Fixture::with_inference("normal", 3000, 30000);
    let d = f.dispatcher(32);
    let active = submit(&d, "hold");
    f.wait_file("active");
    let dropped = submit(&d, "dropped");
    drop(dropped);
    std::thread::sleep(Duration::from_millis(20));
    let mut waiters = vec![];
    for _ in 0..32 {
        waiters.push(submit(&d, "waiting"));
    }
    let mut core = FitSession::new(
        Box::new(TextRequest::new("short".into()).unwrap()),
        FitOptions {
            target_tokens: 100,
            counter_id: "independent".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let FitAction::Count {
        candidate_id,
        request,
        ..
    } = core.advance(None).unwrap()
    else {
        panic!("count")
    };
    let result = core
        .advance(Some(FitReply::Counted {
            candidate_id,
            counter_id: "independent".into(),
            scope: request.scope,
            count: TokenCount::exact(1),
        }))
        .unwrap();
    assert!(matches!(result, FitAction::Done(_)));
    assert!(active.try_take().is_none());
    active.cancel();
    assert_eq!(active.wait().outcome, Err(RuntimeError::Cancelled));
    drop(waiters);
    d.close();
    assert!(
        !std::fs::read_to_string(f.root.join("trace"))
            .unwrap()
            .contains("dropped")
    );
}
#[test]
#[ignore = "explicit approved local environment/model only; never provisions or downloads"]
fn real_approved_runtime_functional() {
    let path = std::env::var_os("TOKENFOLD_TEST_RUNTIME_CONFIG").expect("explicit config required");
    let config = SelectRuntimeConfig::from_path(&PathBuf::from(path)).unwrap();
    let d = ScorerDispatcher::new(config, QueuePolicy::default()).unwrap();
    let warmup = d.warmup().unwrap().wait();
    assert!(warmup.outcome.is_ok(), "{warmup:?}");
    println!(
        "functional warmup startup_ms={:?}",
        warmup.metadata.startup_ms
    );
    for _ in 0..2 {
        let req = OriginalScoringRequest {
            query: "Which group explains cache invalidation?".into(),
            groups: vec![
                OriginalScoringGroup {
                    id: "cache".into(),
                    text: "Invalidate an entry when its source changes.".into(),
                },
                OriginalScoringGroup {
                    id: "logs".into(),
                    text: "Write diagnostic events to the application log.".into(),
                },
            ],
        };
        let result = d
            .submit(
                req,
                CancellationToken::default(),
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap()
            .wait();
        println!("functional score={result:?}");
        let scores = result.outcome.unwrap();
        assert_eq!(scores.len(), 2);
        assert!(scores.iter().all(|s| s.score.is_finite()));
        assert_eq!(scores[0].id, "cache");
        assert_eq!(scores[1].id, "logs");
    }
    d.close();
}
#[test]
fn approved_package_parent_cannot_shadow_python_dependencies() {
    let f = Fixture::new("normal", 3000);
    std::fs::write(
        f.root.join("src/json.py"),
        "raise RuntimeError('unapproved sibling injection')\n",
    )
    .unwrap();
    let d = f.dispatcher(32);
    assert!(submit(&d, "normal").wait().outcome.is_ok());
    d.close();
}
#[test]
fn unpinned_package_extensions_require_reapproval() {
    let f = Fixture::new("normal", 3000);
    let d = f.dispatcher(32);
    std::fs::write(
        f.root.join("src/tokenfold_select_runtime/ettin.pyd"),
        "unapproved",
    )
    .unwrap();
    assert_eq!(
        submit(&d, "normal").wait().outcome,
        Err(RuntimeError::Config)
    );
    assert!(f.pids().is_empty());
    d.close();
}
#[test]
fn failed_os_spawn_remains_an_honest_start_attempt() {
    let f = Fixture::new("normal", 3000);
    let original = std::fs::read_to_string(f.root.join("approval.json")).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&original).unwrap();
    let executable = f.root.join("invalid-executable.exe");
    std::fs::write(&executable, "not an executable").unwrap();
    value["python_executable"] = json!(executable);
    value["python_sha256"] = json!(hex_sha256(b"not an executable"));
    let config = SelectRuntimeConfig::from_json(&serde_json::to_vec(&value).unwrap()).unwrap();
    let d = ScorerDispatcher::new(config, QueuePolicy::default()).unwrap();
    let completion = submit(&d, "normal").wait();
    assert_eq!(completion.outcome, Err(RuntimeError::Failure));
    assert!(RuntimeError::Failure.attempted());
    assert!(matches!(
        completion.into_reply(1).unwrap(),
        tokenfold_core::fit::FitReply::ScoreCompleted {
            outcome: tokenfold_core::fit::ScoreOutcome::Failed(
                tokenfold_core::fit::ScoreFailure::Failure
            ),
            ..
        }
    ));
    d.close();
    assert!(f.pids().is_empty());
}
#[test]
fn dropping_a_consumed_ticket_does_not_cancel_its_fit_session() {
    let f = Fixture::new("normal", 3000);
    let d = f.dispatcher(32);
    let cancel = CancellationToken::default();
    let ticket = d
        .submit(
            request("normal"),
            cancel.clone(),
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
    ticket.wait().outcome.unwrap();
    drop(ticket);
    assert!(cancel.check().is_ok());
    d.close();
}
#[test]
fn concurrent_close_callers_both_observe_reaped_process() {
    let f = Fixture::new("slow_close", 3000);
    let d = f.dispatcher(32);
    submit(&d, "normal").wait().outcome.unwrap();
    std::thread::scope(|scope| {
        scope.spawn(|| d.close());
        f.wait_file("closing");
        d.close();
        f.assert_reaped();
    });
}
#[test]
fn resident_scorer_sync_host_entry_reuses_approved_owner() {
    let f = Fixture::new("normal", 3000);
    let d: ResidentScorer = f.dispatcher(32);
    assert_eq!(
        d.score(request("normal"), CancellationToken::default())
            .unwrap()[0]
            .id,
        "a"
    );
    d.close();
    assert_eq!(f.pids().len(), 1);
}
#[test]
fn schema_one_formatted_byte_limit_is_checked_before_spawn() {
    let f = Fixture::new("normal", 3000);
    let d = f.dispatcher(32);
    let mut req = OriginalScoringRequest {
        query: "query".into(),
        groups: (0..512)
            .map(|i| OriginalScoringGroup {
                id: format!("g{i}"),
                text: "x".repeat(1900),
            })
            .collect(),
    };
    let raw=serde_json::to_vec(&json!({"schema_version":1,"model_revision":"a".repeat(64),"query":req.query,"groups":req.groups})).unwrap();
    req.groups[0]
        .text
        .push_str(&"x".repeat(1048576 - 128 - raw.len()));
    assert!(matches!(
        d.submit(
            req,
            CancellationToken::default(),
            Instant::now() + Duration::from_secs(5)
        ),
        Err(RuntimeError::SelectionLimit)
    ));
    assert!(f.pids().is_empty());
    d.close();
}
fn source_pin(root: &std::path::Path) -> String {
    let mut bytes = vec![];
    for name in [
        "__init__.py",
        "__main__.py",
        "ettin.py",
        "integrity.py",
        "protocol.py",
        "provision.py",
        "worker.py",
    ] {
        bytes.extend_from_slice(name.as_bytes());
        bytes.push(0);
        bytes.extend(std::fs::read(root.join(name)).unwrap());
        bytes.push(0);
    }
    hex_sha256(&bytes)
}
fn owned_venv(f: &Fixture, symlinks: bool) -> (PathBuf, PathBuf) {
    let approval: serde_json::Value =
        serde_json::from_slice(&std::fs::read(f.root.join("approval.json")).unwrap()).unwrap();
    let root = f.root.join("owned-venv");
    let output=std::process::Command::new(approval["python_executable"].as_str().unwrap()).args(["-I","-S","-B","-c", "import json,os,sys,sysconfig,venv; root=sys.argv[1]; venv.EnvBuilder(with_pip=False,symlinks=sys.argv[2]=='1').create(root); print(json.dumps(dict(python=os.path.join(root,'Scripts' if os.name=='nt' else 'bin','python.exe' if os.name=='nt' else 'python'),site=sysconfig.get_path('purelib',scheme='venv',vars=dict(base=root,platbase=root)))))"]).arg(&root).arg(if symlinks{"1"}else{"0"}).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let paths: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    (
        PathBuf::from(paths["python"].as_str().unwrap()),
        PathBuf::from(paths["site"].as_str().unwrap()),
    )
}
fn repin(f: &Fixture, python: &std::path::Path, source: &std::path::Path) -> SelectRuntimeConfig {
    let mut approval: serde_json::Value =
        serde_json::from_slice(&std::fs::read(f.root.join("approval.json")).unwrap()).unwrap();
    approval["python_executable"] = json!(python);
    approval["python_sha256"] = json!(hex_sha256(&std::fs::read(python).unwrap()));
    approval["runtime_source"] = json!(source);
    approval["runtime_source_sha256"] = json!(source_pin(source));
    SelectRuntimeConfig::from_json(&serde_json::to_vec(&approval).unwrap()).unwrap()
}
#[test]
fn startup_hooks_cannot_run_or_cache_an_unpinned_worker() {
    let f = Fixture::new("normal", 5000);
    let (python, site) = owned_venv(&f, false);
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/tokenfold-select-runtime/src/tokenfold_select_runtime")
        .canonicalize()
        .unwrap();
    let before = source_pin(&source);
    let marker = f.root.join("startup-hook-ran");
    let cached = f.root.join("cached-worker-ran");
    std::fs::write(site.join("runtime_test_startup_hook.py"),format!("import pathlib,sys,types\npathlib.Path({}).write_text('unapproved startup ran')\nmodule=types.ModuleType('tokenfold_select_runtime.worker')\nmodule.main=lambda: pathlib.Path({}).write_text('cached worker ran')\nsys.modules[module.__name__]=module\n",json!(marker.to_string_lossy()),json!(cached.to_string_lossy()))).unwrap();
    std::fs::write(
        site.join("startup-hook.pth"),
        "import runtime_test_startup_hook\n",
    )
    .unwrap();
    let customize = f.root.join("sitecustomize-ran");
    std::fs::write(
        site.join("sitecustomize.py"),
        format!(
            "from pathlib import Path\nPath({}).write_text('sitecustomize ran')\n",
            json!(customize.to_string_lossy())
        ),
    )
    .unwrap();
    let d = ScorerDispatcher::new(repin(&f, &python, &source), QueuePolicy::default()).unwrap();
    // The real maintained worker rejects this fixture's incomplete bundle before loading ML.
    assert_eq!(
        d.warmup().unwrap().wait().outcome,
        Err(RuntimeError::Failure)
    );
    d.close();
    assert_eq!(
        [marker.exists(), customize.exists(), cached.exists()],
        [false; 3],
        "startup hook, sitecustomize, cached worker must all remain unexecuted"
    );
    assert_eq!(source_pin(&source), before);
}
#[test]
fn isolated_startup_discovers_venv_dependencies_without_resolving_away_its_prefix() {
    let f = Fixture::new("normal", 5000);
    let (python, site) = owned_venv(&f, true);
    if !std::fs::symlink_metadata(&python)
        .unwrap()
        .file_type()
        .is_symlink()
    {
        println!("SKIPPED symlink-specific assertion: platform venv used a copied executable");
    }
    let source = f.root.join("src/tokenfold_select_runtime");
    let prefix = python.parent().unwrap().parent().unwrap();
    std::fs::write(
        site.join("runtime_test_dependency.py"),
        format!(
            "import os,sys\nassert os.path.samefile(sys.prefix,{})\n",
            json!(prefix.to_string_lossy())
        ),
    )
    .unwrap();
    std::fs::write(
        source.join("__main__.py"),
        format!("import runtime_test_dependency\n{FAKE}"),
    )
    .unwrap();
    let d = ScorerDispatcher::new(repin(&f, &python, &source), QueuePolicy::default()).unwrap();
    assert!(submit(&d, "normal").wait().outcome.is_ok());
    d.close();
}
