//! Opt-in text-to-group Select adapter with an approved, direct-child scoring runtime.
//! The runtime reads/writes the file paths in TOKENFOLD_SELECT_REQUEST_PATH and
//! TOKENFOLD_SELECT_RESPONSE_PATH. It must not launch descendants. No shell is invoked,
//! inherited environment or payload logging is permitted, and no model is downloaded.
//! Approval covers the executable SHA-256 plus caller-owned arguments/model revision;
//! callers must protect the executable, auxiliary model files and scratch directory.
//! This enforces the child inference deadline, not OS sandboxing or model quality.

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokenfold_core::allocation::{self, Candidate, Group, SelectRequest, SelectResponse};
use tokenfold_core::retrieval_store::hex_sha256;
use tokenfold_core::token_estimator::TokenEstimator;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextGroup {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub fallback_score: f64,
}

pub struct Context<'a> {
    pub prefix: &'a str,
    pub suffix: &'a str,
    pub groups: &'a [ContextGroup],
}

#[derive(Debug, Clone)]
pub struct SelectPolicy {
    pub enabled: bool,
    pub target_tokens: usize,
    pub inference_timeout: Duration,
}

impl Default for SelectPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            target_tokens: 4096,
            inference_timeout: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovedScorer {
    pub executable: PathBuf,
    pub executable_sha256: String,
    pub arguments: Vec<String>,
    pub model_revision: String,
    pub approved_by: String,
    pub scratch_root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct SelectOutcome {
    pub text: String,
    pub baseline_tokens: usize,
    pub output_tokens: usize,
    pub kept_group_ids: Vec<String>,
    pub used_scorer: bool,
    pub fallback_reason: Option<String>,
    pub selected: bool,
}

#[derive(serde::Serialize)]
struct RuntimeRequest<'a> {
    schema_version: u32,
    model_revision: &'a str,
    query: &'a str,
    groups: Vec<RuntimeGroup<'a>>,
}

#[derive(serde::Serialize)]
struct RuntimeGroup<'a> {
    id: &'a str,
    text: &'a str,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeResponse {
    schema_version: u32,
    model_revision: String,
    scores: Vec<(String, f64)>,
}

fn assemble(context: &Context<'_>, ids: &HashSet<&str>) -> String {
    let mut text = context.prefix.to_string();
    for group in context.groups {
        if ids.contains(group.id.as_str()) {
            text.push_str(&group.text);
        }
    }
    text.push_str(context.suffix);
    text
}

/// Exact text is retained in source order; the assembled prompt is recounted after selection.
/// Disabled/invalid/cancelled/empty/over-budget/no-gain candidates keep the complete baseline.
/// Scorer failure uses caller-declared fallback scores, never unchecked model output.
pub fn select_context(
    context: &Context<'_>,
    query: &str,
    policy: &SelectPolicy,
    estimator: &dyn TokenEstimator,
    runtime: Option<&ApprovedScorer>,
    cancelled: Option<&AtomicBool>,
) -> SelectOutcome {
    let all = context
        .groups
        .iter()
        .map(|g| g.id.as_str())
        .collect::<HashSet<_>>();
    let baseline = assemble(context, &all);
    let baseline_tokens = estimator.count_bytes(baseline.as_bytes());
    let keep_baseline = |reason: String| SelectOutcome {
        text: baseline.clone(),
        baseline_tokens,
        output_tokens: baseline_tokens,
        kept_group_ids: context.groups.iter().map(|g| g.id.clone()).collect(),
        used_scorer: false,
        fallback_reason: Some(reason),
        selected: false,
    };
    if !policy.enabled {
        return keep_baseline("disabled".into());
    }
    if context.groups.is_empty()
        || context.groups.len() > allocation::MAX_SELECT_BATCH
        || all.len() != context.groups.len()
        || baseline.len() > 1024 * 1024
        || query.len() > 4096
        || context
            .groups
            .iter()
            .any(|g| g.id.is_empty() || g.id.len() > 256 || !g.fallback_score.is_finite())
        || policy.inference_timeout.is_zero()
        || policy.inference_timeout > Duration::from_secs(30)
    {
        return keep_baseline("invalid or oversized Select input".into());
    }
    if cancelled.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
        return keep_baseline("cancelled".into());
    }
    let score_result = runtime
        .ok_or_else(|| "scorer unavailable".to_string())
        .and_then(|runtime| runtime.score(context, query, policy.inference_timeout, cancelled));
    if cancelled.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
        return keep_baseline("cancelled".into());
    }
    let (scores, fallback_reason) = match score_result {
        Ok(scores) => (Some(scores), None),
        Err(error) => (None, Some(error)),
    };
    let groups = context
        .groups
        .iter()
        .map(|g| Group {
            id: g.id.clone(),
            required: g.required,
            members: vec![Candidate {
                id: g.id.clone(),
                cost: estimator.count_bytes(g.text.as_bytes()),
                score: g.fallback_score,
            }],
        })
        .collect::<Vec<_>>();
    let fixed = format!("{}{}", context.prefix, context.suffix);
    let allocation = allocation::allocate_with_scorer(
        &groups,
        policy
            .target_tokens
            .saturating_sub(estimator.count_bytes(fixed.as_bytes())),
        scores.clone(),
    );
    let mut ids = allocation
        .kept_group_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    // Additive estimates are only a proposal. Trim whole optional groups and recount the prompt.
    let mut optional = context
        .groups
        .iter()
        .enumerate()
        .filter(|(_, g)| ids.contains(g.id.as_str()) && !g.required)
        .collect::<Vec<_>>();
    optional.sort_by(|(ia, a), (ib, b)| {
        let score = |g: &ContextGroup| {
            scores
                .as_ref()
                .and_then(|s| s.get(&g.id))
                .copied()
                .unwrap_or(g.fallback_score)
        };
        score(a).total_cmp(&score(b)).then(ib.cmp(ia))
    });
    let mut text = assemble(context, &ids);
    let mut output_tokens = estimator.count_bytes(text.as_bytes());
    for (_, group) in optional {
        if output_tokens <= policy.target_tokens {
            break;
        }
        ids.remove(group.id.as_str());
        text = assemble(context, &ids);
        output_tokens = estimator.count_bytes(text.as_bytes());
    }
    if ids.is_empty() || output_tokens > policy.target_tokens || output_tokens >= baseline_tokens {
        return keep_baseline(
            "empty, over-budget or no token gain after full-prompt recount".into(),
        );
    }
    if cancelled.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
        return keep_baseline("cancelled".into());
    }
    SelectOutcome {
        text,
        baseline_tokens,
        output_tokens,
        kept_group_ids: context
            .groups
            .iter()
            .filter(|g| ids.contains(g.id.as_str()))
            .map(|g| g.id.clone())
            .collect(),
        used_scorer: scores.is_some(),
        fallback_reason,
        selected: true,
    }
}

impl ApprovedScorer {
    fn score(
        &self,
        context: &Context<'_>,
        query: &str,
        timeout: Duration,
        cancelled: Option<&AtomicBool>,
    ) -> Result<std::collections::BTreeMap<String, f64>, String> {
        use std::io::Read;
        if self.approved_by.trim().is_empty()
            || self.model_revision.trim().is_empty()
            || !self.executable.is_absolute()
            || !self.scratch_root.is_absolute()
        {
            return Err(
                "explicit local runtime approval, revision and absolute paths are required".into(),
            );
        }
        let mut executable = Vec::new();
        std::fs::File::open(&self.executable)
            .map_err(|e| e.to_string())?
            .take(64 * 1024 * 1024 + 1)
            .read_to_end(&mut executable)
            .map_err(|e| e.to_string())?;
        if executable.len() > 64 * 1024 * 1024 || hex_sha256(&executable) != self.executable_sha256
        {
            return Err("scorer executable does not match its approved SHA-256".into());
        }
        let request = RuntimeRequest {
            schema_version: allocation::SELECT_SCHEMA_VERSION,
            model_revision: &self.model_revision,
            query,
            groups: context
                .groups
                .iter()
                .map(|g| RuntimeGroup {
                    id: &g.id,
                    text: &g.text,
                })
                .collect(),
        };
        let bytes = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
        if tokenfold_core::transforms::redaction::contains_secret(&bytes) {
            return Err("secret-shaped scorer input refused".into());
        }
        let root = self
            .scratch_root
            .canonicalize()
            .map_err(|e| e.to_string())?;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = root.join(format!(
            "select-{}-{}",
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
        builder.create(&directory).map_err(|e| e.to_string())?;
        let files = RuntimeFiles {
            request: directory.join("request.json"),
            response: directory.join("response.json"),
            directory,
        };
        std::fs::write(&files.request, bytes).map_err(|e| e.to_string())?;
        let mut child = Command::new(&self.executable)
            .args(&self.arguments)
            .env_clear()
            .env("TOKENFOLD_SELECT_REQUEST_PATH", &files.request)
            .env("TOKENFOLD_SELECT_RESPONSE_PATH", &files.response)
            .current_dir(&root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
        let deadline = Instant::now() + timeout;
        let status = loop {
            let reason = if cancelled.is_some_and(|f| f.load(Ordering::SeqCst)) {
                Some("scorer cancelled")
            } else if Instant::now() >= deadline {
                Some("scorer inference deadline exceeded")
            } else if std::fs::metadata(&files.response).is_ok_and(|m| m.len() > 65536) {
                Some("scorer response exceeds 64 KiB")
            } else {
                None
            };
            if let Some(reason) = reason {
                child.kill().map_err(|e| e.to_string())?;
                child.wait().map_err(|e| e.to_string())?;
                return Err(reason.into());
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => std::thread::sleep(Duration::from_millis(2)),
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error.to_string());
                }
            }
        };
        if !status.success() {
            return Err("scorer exited unsuccessfully".into());
        }
        let mut bytes = Vec::new();
        std::fs::File::open(&files.response)
            .map_err(|e| e.to_string())?
            .take(65537)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 65536 {
            return Err("scorer response exceeds 64 KiB".into());
        }
        let response: RuntimeResponse = serde_json::from_slice(&bytes)
            .map_err(|_| "scorer response is malformed".to_string())?;
        let request = SelectRequest::new(
            query,
            &self.model_revision,
            context.groups.iter().map(|g| g.id.clone()).collect(),
        )
        .map_err(|e| format!("{e:?}"))?;
        allocation::validate_response(
            &request,
            &SelectResponse {
                schema_version: response.schema_version,
                model_revision: response.model_revision,
                scores: response.scores,
            },
        )
        .map_err(|e| {
            use allocation::SelectRejection::*;
            let kind = match e {
                ModelRevision { .. } => "ModelRevision",
                SchemaVersion { .. } => "SchemaVersion",
                IdMismatch => "IdMismatch",
                NonFiniteScore { .. } => "NonFiniteScore",
                BatchTooLarge { .. } => "BatchTooLarge",
                Unavailable => "Unavailable",
            };
            format!("scorer response rejected: {kind}")
        })
    }
}

struct RuntimeFiles {
    request: PathBuf,
    response: PathBuf,
    directory: PathBuf,
}
impl Drop for RuntimeFiles {
    fn drop(&mut self) {
        // Only these exact generated files; never recursively delete runtime-created paths.
        let _ = std::fs::remove_file(&self.request);
        let _ = std::fs::remove_file(&self.response);
        let _ = std::fs::remove_dir(&self.directory);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokenfold_core::token_estimator::ByteHeuristicEstimator;

    #[test]
    #[ignore = "protocol worker invoked only by the subprocess regression"]
    fn runtime_worker() {
        let request: serde_json::Value = serde_json::from_slice(
            &std::fs::read(std::env::var_os("TOKENFOLD_SELECT_REQUEST_PATH").unwrap()).unwrap(),
        )
        .unwrap();
        if request["query"] == "hang" {
            std::thread::sleep(Duration::from_secs(10));
        }
        let revision = if request["query"] == "wrong_revision" {
            "other"
        } else {
            request["model_revision"].as_str().unwrap()
        };
        let scores = request["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| {
                (
                    g["id"].as_str().unwrap(),
                    if g["id"] == "b" { 10.0 } else { 0.1 },
                )
            })
            .collect::<Vec<_>>();
        std::fs::write(
            std::env::var_os("TOKENFOLD_SELECT_RESPONSE_PATH").unwrap(),
            serde_json::to_vec(
                &serde_json::json!({"schema_version":1,"model_revision":revision,"scores":scores}),
            )
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn approved_process_mapping_scores_deadline_and_fallback_are_real() {
        let root = std::env::temp_dir().join(format!(
            "tf-select-runtime-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let executable = std::env::current_exe().unwrap();
        // Unix debug/coverage executables can exceed the production approval cap.
        // An explicitly approved exec-only launcher replaces itself with the same
        // Rust worker, so deadline/cancellation still control the direct child.
        #[cfg(unix)]
        let executable = {
            use std::os::unix::fs::PermissionsExt;
            let launcher = root.with_extension("sh");
            let quoted = executable.to_str().unwrap().replace('\'', "'\\''");
            let profile = root.with_extension("profraw");
            let profile = profile.to_str().unwrap().replace('\'', "'\\''");
            std::fs::write(
                &launcher,
                format!(
                    "#!/bin/sh\nexport LLVM_PROFILE_FILE='{profile}'\nexec '{quoted}' \"$@\"\n"
                ),
            )
            .unwrap();
            std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700)).unwrap();
            launcher
        };
        #[cfg(unix)]
        let launcher = executable.clone();
        let runtime = ApprovedScorer {
            executable_sha256: hex_sha256(&std::fs::read(&executable).unwrap()),
            executable,
            arguments: vec![
                "--ignored".into(),
                "--exact".into(),
                "select::tests::runtime_worker".into(),
            ],
            model_revision: "pinned-test-v1".into(),
            approved_by: "test-owner".into(),
            scratch_root: root.clone(),
        };
        let groups = vec![
            ContextGroup {
                id: "a".into(),
                text: "aaaaaaaa".into(),
                required: false,
                fallback_score: 2.0,
            },
            ContextGroup {
                id: "b".into(),
                text: "bbbbbbbb".into(),
                required: false,
                fallback_score: 1.0,
            },
            ContextGroup {
                id: "c".into(),
                text: "cccccccc".into(),
                required: false,
                fallback_score: 0.0,
            },
        ];
        let context = Context {
            prefix: "",
            suffix: "",
            groups: &groups,
        };
        let policy = SelectPolicy {
            enabled: true,
            target_tokens: 2,
            inference_timeout: Duration::from_secs(2),
        };
        let output = select_context(
            &context,
            "query",
            &policy,
            &ByteHeuristicEstimator,
            Some(&runtime),
            None,
        );
        assert!(output.selected, "{output:?}");
        assert!(output.used_scorer, "{output:?}");
        assert_eq!(output.kept_group_ids, vec!["b"]);
        assert_eq!(output.text, "bbbbbbbb");
        let wrong = select_context(
            &context,
            "wrong_revision",
            &policy,
            &ByteHeuristicEstimator,
            Some(&runtime),
            None,
        );
        assert!(wrong.selected);
        assert!(!wrong.used_scorer);
        assert!(wrong.fallback_reason.unwrap().contains("ModelRevision"));
        assert_eq!(wrong.kept_group_ids, vec!["a"]);
        let short = SelectPolicy {
            inference_timeout: Duration::from_millis(30),
            ..policy.clone()
        };
        let start = Instant::now();
        let timed_out = select_context(
            &context,
            "hang",
            &short,
            &ByteHeuristicEstimator,
            Some(&runtime),
            None,
        );
        assert!(!timed_out.used_scorer);
        assert!(timed_out.fallback_reason.unwrap().contains("deadline"));
        assert!(start.elapsed() < Duration::from_secs(3));
        let cancel = AtomicBool::new(true);
        assert!(
            !select_context(
                &context,
                "query",
                &policy,
                &ByteHeuristicEstimator,
                Some(&runtime),
                Some(&cancel)
            )
            .selected
        );
        let active_cancel = std::sync::Arc::new(AtomicBool::new(false));
        let signal = active_cancel.clone();
        let trigger = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            signal.store(true, Ordering::SeqCst);
        });
        let interrupted = select_context(
            &context,
            "hang",
            &policy,
            &ByteHeuristicEstimator,
            Some(&runtime),
            Some(&active_cancel),
        );
        trigger.join().unwrap();
        assert!(!interrupted.selected);
        assert_eq!(interrupted.fallback_reason.as_deref(), Some("cancelled"));
        let bad = ApprovedScorer {
            executable_sha256: "not-approved".into(),
            ..runtime
        };
        let rejected = select_context(
            &context,
            "query",
            &policy,
            &ByteHeuristicEstimator,
            Some(&bad),
            None,
        );
        assert!(!rejected.used_scorer);
        assert!(rejected.fallback_reason.unwrap().contains("SHA-256"));
        assert!(std::fs::read_dir(&root).unwrap().next().is_none());
        std::fs::remove_dir(&root).unwrap();
        #[cfg(unix)]
        {
            std::fs::remove_file(launcher).unwrap();
            // env_clear deliberately removes llvm-cov's output path; keep the
            // instrumented worker's own output outside the runtime scratch root.
            let profile = root.with_extension("profraw");
            if profile.exists() {
                std::fs::remove_file(profile).unwrap();
            }
        }
    }

    struct BoundaryEstimator;
    impl TokenEstimator for BoundaryEstimator {
        fn info(&self) -> tokenfold_core::report::EstimatorInfo {
            ByteHeuristicEstimator.info()
        }
        fn count_bytes(&self, bytes: &[u8]) -> usize {
            if bytes.len() > 2 {
                bytes.len() * 4
            } else {
                bytes.len()
            }
        }
    }

    #[test]
    fn full_prompt_recount_keeps_required_text_and_never_emits_empty_or_over_budget_context() {
        let groups = vec![
            ContextGroup {
                id: "required".into(),
                text: "AA".into(),
                required: true,
                fallback_score: 0.0,
            },
            ContextGroup {
                id: "b".into(),
                text: "BB".into(),
                required: false,
                fallback_score: 2.0,
            },
            ContextGroup {
                id: "c".into(),
                text: "CC".into(),
                required: false,
                fallback_score: 1.0,
            },
        ];
        let context = Context {
            prefix: "",
            suffix: "",
            groups: &groups,
        };
        let policy = SelectPolicy {
            enabled: true,
            target_tokens: 4,
            ..SelectPolicy::default()
        };
        let output = select_context(&context, "query", &policy, &BoundaryEstimator, None, None);
        assert!(output.selected);
        assert_eq!(output.text, "AA");
        assert_eq!(output.kept_group_ids, vec!["required"]);
        assert!(output.output_tokens <= 4);
        let too_small = SelectPolicy {
            target_tokens: 0,
            ..policy
        };
        let fallback = select_context(
            &context,
            "query",
            &too_small,
            &BoundaryEstimator,
            None,
            None,
        );
        assert!(!fallback.selected);
        assert_eq!(fallback.text, "AABBCC");
    }
}
