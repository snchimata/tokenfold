//! Versioned per-attempt measurement events for provider round trips.
//!
//! One attempt (one forwarded provider request) produces exactly one terminal
//! [`MeasurementEvent`], whether it completed, timed out, was cancelled, or failed, and whether
//! the provider reported usage or not. Two rules make the shape worth parsing:
//!
//! - **Absent is `null`, never `0`.** Every field with no honest data source is `None`, so a
//!   reader can tell "the provider reported no usage" from "the provider reported zero tokens".
//! - **Usage is last-wins, not summed.** Providers publish cumulative snapshots (OpenAI sends one
//!   final `usage` object; Anthropic stages `input_tokens` in `message_start` and
//!   `output_tokens` in `message_delta`). Snapshots merge field-by-field, so a repeated snapshot
//!   can never be double-counted.
//!
//! [`MeasurementReader`] reads the streamed response body *without buffering or rewriting it*: it
//! keeps at most [`MAX_SCAN_BYTES`] of an unterminated line, forwards every byte untouched, and
//! stops accounting entirely if the body is malformed or exceeds that bound. Both framing shapes
//! are handled — server-sent events and a single JSON document — chosen from the body's first
//! non-whitespace byte, so an event stream is never line-split out of a JSON document and vice
//! versa. Accounting failures disable measurement for that attempt only — never forwarding.
//!
//! Local counts use the provider-independent estimators in [`crate::token_estimator`]; provider
//! usage is compared against them as a *signed*, input-side delta
//! ([`MeasurementEvent::provider_delta_tokens`]): provider prompt count minus local after count,
//! which is why a local count that over-counts the provider yields a negative number.

use std::io::{ErrorKind, Read};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::errors::TokenFoldError;
use crate::report::EstimatorInfo;

/// Version of the [`MeasurementEvent`] contract. Bump it for any incompatible field change;
/// readers must reject a version they do not know rather than guess at it.
pub const MEASUREMENT_SCHEMA_VERSION: &str = "1.0";

/// Prefix a writer puts in front of the serialized event, and [`parse_event_line`] looks for.
pub const EVENT_LOG_PREFIX: &str = "tokenfold.measurement";

/// Upper bound on the bytes held while looking for an event boundary. A body that exceeds it
/// without a newline is treated as unmeasurable rather than buffered.
pub const MAX_SCAN_BYTES: usize = 64 * 1024;

/// How an attempt ended. `TimedOut` and `Cancelled` are deliberately distinct: a deadline is the
/// proxy's own configured bound, a cancellation is the peer going away.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionState {
    Completed,
    TimedOut,
    Cancelled,
    Failed,
}

impl CompletionState {
    /// Maps a transport read error onto the attempt's terminal state.
    fn from_read_error(error: &std::io::Error) -> Self {
        match error.kind() {
            ErrorKind::TimedOut | ErrorKind::WouldBlock => CompletionState::TimedOut,
            ErrorKind::ConnectionReset
            | ErrorKind::ConnectionAborted
            | ErrorKind::BrokenPipe
            | ErrorKind::UnexpectedEof => CompletionState::Cancelled,
            _ => CompletionState::Failed,
        }
    }
}

/// What happened to provider usage for this attempt. A non-`Reported` disposition is always
/// paired with `provider_usage: null`: an untrustworthy number is worse than no number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageDisposition {
    /// At least one usage snapshot was seen and the whole body was scanned cleanly.
    Reported,
    /// The body was scanned cleanly and carried no usage (e.g. streaming without
    /// `stream_options.include_usage`).
    Absent,
    /// A payload was unparseable, or its `usage` was present but unusable.
    Malformed,
    /// The body exceeded [`MAX_SCAN_BYTES`] without an event boundary, so scanning stopped early.
    Oversized,
}

/// Which tokenizer family, if any, this codebase can count a model's text with exactly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelResolution {
    /// The model maps onto a tokenizer this crate implements, so local counts can claim
    /// exactness for it.
    Known { model: String, tokenizer: String },
    /// No tokenizer mapping exists; local counts for it must stay labeled estimates.
    Unknown { requested: String },
}

/// Maps a provider model name onto a tokenizer family this crate can reproduce exactly.
///
/// Only documented `tiktoken` families are claimed. Anything else — including every Anthropic and
/// every locally served model name — resolves to [`ModelResolution::Unknown`] rather than
/// borrowing another family's exactness.
pub fn resolve_model(model: &str) -> ModelResolution {
    // Order matters: `gpt-4o`/`gpt-4.1` must be tested before the shorter `gpt-4` prefix.
    const O200K_PREFIXES: &[&str] = &["gpt-4o", "gpt-4.1", "chatgpt-4o", "o1", "o3", "o4"];
    const CL100K_PREFIXES: &[&str] = &[
        "gpt-4-turbo",
        "gpt-4",
        "gpt-3.5",
        "gpt-35",
        "text-embedding-ada-002",
        "text-embedding-3",
    ];
    let normalized = model.trim().to_ascii_lowercase();
    for (prefixes, tokenizer) in [
        (O200K_PREFIXES, "o200k_base"),
        (CL100K_PREFIXES, "cl100k_base"),
    ] {
        if prefixes.iter().any(|p| normalized.starts_with(p)) {
            return ModelResolution::Known {
                model: model.trim().to_string(),
                tokenizer: tokenizer.to_string(),
            };
        }
    }
    ModelResolution::Unknown {
        requested: model.trim().to_string(),
    }
}

/// Token counts as reported by the provider. OpenAI field names, with Anthropic's
/// `input_tokens`/`output_tokens` accepted as the prompt/completion equivalent. Nothing is
/// derived: a provider that reports only a total gets `total_tokens` and two `None`s.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderUsage {
    pub prompt_tokens: Option<usize>,
    pub completion_tokens: Option<usize>,
    pub total_tokens: Option<usize>,
}

impl ProviderUsage {
    /// The provider's own total, or the sum of its two parts when it reported both.
    pub fn total(&self) -> Option<usize> {
        self.total_tokens
            .or_else(|| Some(self.prompt_tokens? + self.completion_tokens?))
    }

    /// Folds a later cumulative snapshot over an earlier one, field by field. Overwriting (rather
    /// than accumulating) is what stops a repeated snapshot from being counted twice.
    fn merge(&mut self, later: ProviderUsage) {
        if later.prompt_tokens.is_some() {
            self.prompt_tokens = later.prompt_tokens;
        }
        if later.completion_tokens.is_some() {
            self.completion_tokens = later.completion_tokens;
        }
        if later.total_tokens.is_some() {
            self.total_tokens = later.total_tokens;
        }
    }
}

/// The per-attempt facts known before the response body is read. Kept separate from
/// [`MeasurementEvent`] so a reader still being constructed cannot be mistaken for a finished one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasurementSpec {
    pub request_id: String,
    /// Opaque, privacy-safe session key — see [`opaque_id`]. Absent when the caller supplied none.
    pub session_id: Option<String>,
    /// 1-based attempt number within `request_id` (retries and agent steps increment it).
    pub attempt: u32,
    pub model: Option<ModelResolution>,
    /// Identity of the local policy applied to this attempt (e.g. the applied transform
    /// versions); `null` when no local transform ran.
    pub policy_revision: Option<String>,
    pub estimator: Option<EstimatorInfo>,
    pub local_before_tokens: Option<usize>,
    pub local_after_tokens: Option<usize>,
    pub local_transform_micros: Option<u64>,
}

impl MeasurementSpec {
    pub fn new(request_id: impl Into<String>) -> Self {
        Self {
            request_id: request_id.into(),
            session_id: None,
            attempt: 1,
            model: None,
            policy_revision: None,
            estimator: None,
            local_before_tokens: None,
            local_after_tokens: None,
            local_transform_micros: None,
        }
    }

    /// Closes the attempt into its terminal event. Public because a caller that never received a
    /// body (a connection failure) still owes the attempt exactly one event.
    pub fn finish(
        &self,
        completion: CompletionState,
        provider_usage: Option<ProviderUsage>,
        usage_disposition: UsageDisposition,
    ) -> MeasurementEvent {
        let provider_delta_tokens = provider_usage
            .and_then(|usage| usage.prompt_tokens)
            .zip(self.local_after_tokens)
            .map(|(prompt, local)| prompt as i64 - local as i64);
        MeasurementEvent {
            schema_version: MEASUREMENT_SCHEMA_VERSION.to_string(),
            request_id: self.request_id.clone(),
            session_id: self.session_id.clone(),
            attempt: self.attempt,
            model: self.model.clone(),
            policy_revision: self.policy_revision.clone(),
            estimator: self.estimator.clone(),
            local_before_tokens: self.local_before_tokens,
            local_after_tokens: self.local_after_tokens,
            local_transform_micros: self.local_transform_micros,
            provider_usage,
            usage_disposition,
            provider_delta_tokens,
            completion,
        }
    }
}

/// One terminal measurement event, serialized as a single JSON object. See the module docs for
/// the `null`-over-`0` and last-wins rules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasurementEvent {
    pub schema_version: String,
    pub request_id: String,
    pub session_id: Option<String>,
    pub attempt: u32,
    pub model: Option<ModelResolution>,
    pub policy_revision: Option<String>,
    pub estimator: Option<EstimatorInfo>,
    pub local_before_tokens: Option<usize>,
    pub local_after_tokens: Option<usize>,
    pub local_transform_micros: Option<u64>,
    pub provider_usage: Option<ProviderUsage>,
    pub usage_disposition: UsageDisposition,
    /// `provider prompt count - local after count`, deliberately signed: the local estimate can be
    /// either side of the provider's. Both sides are input-side, so this is a request-side
    /// calibration figure — it includes whatever prompt-template overhead the provider adds around
    /// the body it received, and it is `null` unless the provider reported a prompt count and a
    /// local after-count exists.
    pub provider_delta_tokens: Option<i64>,
    pub completion: CompletionState,
}

/// Derives an opaque, stable session key from a caller-supplied identifier: 16 hex characters of
/// its SHA-256, so a session stays correlation-stable across attempts while the raw value never
/// reaches a log line. Same intent as the retrieval store's hashed storage keys.
pub fn opaque_id(raw: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(raw.as_bytes());
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Parses one log line into a [`MeasurementEvent`].
///
/// Returns `None` for any line that is not an event (so a reader can scan a mixed log), and an
/// error for a line that claims to be an event but cannot be read — including an unknown
/// `schema_version`, which is never guessed at.
pub fn parse_event_line(line: &str) -> Option<Result<MeasurementEvent, TokenFoldError>> {
    let payload = line.trim().strip_prefix(EVENT_LOG_PREFIX)?;
    Some(parse_event(payload.trim()))
}

/// Parses the JSON body of an event (what [`parse_event_line`] strips the prefix from).
pub fn parse_event(json: &str) -> Result<MeasurementEvent, TokenFoldError> {
    let event: MeasurementEvent = serde_json::from_str(json)
        .map_err(|e| TokenFoldError::InvalidInput(format!("unreadable measurement event: {e}")))?;
    if event.schema_version != MEASUREMENT_SCHEMA_VERSION {
        return Err(TokenFoldError::InvalidInput(format!(
            "unsupported measurement event schema_version {} (this build reads {})",
            event.schema_version, MEASUREMENT_SCHEMA_VERSION
        )));
    }
    Ok(event)
}

/// Destination for the one terminal event per attempt. A closure rather than a logger so this
/// module owns no output policy and callers can assert on the events directly.
pub type EventSink = Box<dyn Fn(MeasurementEvent) + Send + Sync>;

/// Serializes an event the way a writer puts it on a log line.
pub fn format_event_line(event: &MeasurementEvent) -> Result<String, TokenFoldError> {
    let json = serde_json::to_string(event).map_err(|e| {
        TokenFoldError::InvalidInput(format!("unserializable measurement event: {e}"))
    })?;
    Ok(format!("{EVENT_LOG_PREFIX} {json}"))
}

/// Tracks provider usage found in a byte stream while that stream is being forwarded.
#[derive(Debug, Default)]
struct UsageScanner {
    /// Bytes since the last event boundary; never exceeds [`MAX_SCAN_BYTES`].
    pending: Vec<u8>,
    mode: ScanMode,
    usage: ProviderUsage,
    saw_usage: bool,
    malformed: bool,
    stopped: bool,
}

/// A response body is either framed as server-sent events or is one JSON document, and the two
/// cannot share a reader: an SSE body must never be line-split out of a JSON document, and a JSON
/// document must not be mistaken for an event stream. The first non-whitespace byte decides.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum ScanMode {
    #[default]
    Undecided,
    Events,
    Document,
}

impl UsageScanner {
    /// Feeds bytes as they pass through. Retains at most one unterminated line, or one bounded
    /// JSON document.
    fn push(&mut self, chunk: &[u8]) {
        if self.stopped || chunk.is_empty() {
            return;
        }
        self.pending.extend_from_slice(chunk);
        if self.mode == ScanMode::Undecided {
            match self.pending.iter().find(|b| !b.is_ascii_whitespace()) {
                Some(b'{') | Some(b'[') => self.mode = ScanMode::Document,
                Some(_) => self.mode = ScanMode::Events,
                // Nothing but whitespace so far; the next chunk decides.
                None => return,
            }
        }
        if self.mode == ScanMode::Document {
            self.enforce_bound();
            return;
        }
        while let Some(newline) = self.pending.iter().position(|b| *b == b'\n') {
            let remainder = self.pending.split_off(newline + 1);
            let line = std::mem::replace(&mut self.pending, remainder);
            self.read_line(&line);
            if self.stopped {
                self.pending.clear();
                return;
            }
        }
        self.enforce_bound();
    }

    /// ponytail: bounded scan, not a bounded stream — the body is still forwarded in full; only
    /// this attempt's accounting is abandoned. Providers publish usage last, so a body past the
    /// bound could not be read with a bigger window either.
    fn enforce_bound(&mut self) {
        if self.pending.len() > MAX_SCAN_BYTES {
            self.stopped = true;
            self.pending.clear();
        }
    }

    /// Reads the last line of a body that never got a trailing newline — a plain JSON response,
    /// or a provider that closed mid-event.
    fn finish(&mut self) {
        if self.stopped || self.pending.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.pending);
        match self.mode {
            ScanMode::Document => match serde_json::from_slice::<Value>(&pending) {
                Ok(value) => self.read_usage(&value),
                Err(_) => self.malformed = true,
            },
            ScanMode::Events | ScanMode::Undecided => self.read_line(&pending),
        }
    }

    /// One server-sent event line. `data:` lines carry the JSON; comments, `event:`/`id:` fields
    /// and the terminal `[DONE]` sentinel carry none.
    fn read_line(&mut self, line: &[u8]) {
        let line = trim_ascii(line);
        let Some(payload) = line.strip_prefix(b"data:") else {
            return;
        };
        let payload = trim_ascii(payload);
        if payload == b"[DONE]" {
            return;
        }
        match serde_json::from_slice::<Value>(payload) {
            Ok(value) => self.read_usage(&value),
            Err(_) => self.malformed = true,
        }
    }

    fn read_usage(&mut self, value: &Value) {
        match value.get("usage") {
            None | Some(Value::Null) => {}
            Some(raw) => match usage_from_json(raw) {
                Some(usage) => {
                    self.usage.merge(usage);
                    self.saw_usage = true;
                }
                None => self.malformed = true,
            },
        }
    }

    /// The terminal disposition, and the usage that is honest to publish with it. Oversized and
    /// malformed both suppress the numbers: a partially-scanned or corrupt body cannot support a
    /// trustworthy count, even when an earlier snapshot looked fine.
    fn outcome(&self) -> (UsageDisposition, Option<ProviderUsage>) {
        if self.stopped {
            (UsageDisposition::Oversized, None)
        } else if self.malformed {
            (UsageDisposition::Malformed, None)
        } else if self.saw_usage {
            (UsageDisposition::Reported, Some(self.usage))
        } else {
            (UsageDisposition::Absent, None)
        }
    }
}

/// Reads a provider `usage` object. `None` means the value cannot be accounted for (wrong type,
/// or none of the known counters), which the caller treats as malformed.
fn usage_from_json(raw: &Value) -> Option<ProviderUsage> {
    let object = raw.as_object()?;
    let counter = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| object.get(*key).and_then(Value::as_u64))
            .map(|value| value as usize)
    };
    let usage = ProviderUsage {
        prompt_tokens: counter(&["prompt_tokens", "input_tokens"]),
        completion_tokens: counter(&["completion_tokens", "output_tokens"]),
        total_tokens: counter(&["total_tokens"]),
    };
    if usage == ProviderUsage::default() {
        None
    } else {
        Some(usage)
    }
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map(|i| i + 1)
        .unwrap_or(start);
    &bytes[start..end]
}

/// Streams a response body through unchanged while measuring it, then emits exactly one
/// [`MeasurementEvent`].
///
/// Exactly-once is structural: the event is emitted at end of body, on a read error, or when the
/// reader is dropped without reaching either (a cancellation), and the first of those wins.
pub struct MeasurementReader<R: Read> {
    inner: R,
    spec: MeasurementSpec,
    sink: EventSink,
    scanner: UsageScanner,
    emitted: bool,
}

impl<R: Read> MeasurementReader<R> {
    pub fn new(inner: R, spec: MeasurementSpec, sink: EventSink) -> Self {
        Self {
            inner,
            spec,
            sink,
            scanner: UsageScanner::default(),
            emitted: false,
        }
    }

    fn emit(&mut self, completion: CompletionState) {
        if self.emitted {
            return;
        }
        self.emitted = true;
        self.scanner.finish();
        let (disposition, usage) = self.scanner.outcome();
        (self.sink)(self.spec.finish(completion, usage, disposition));
    }
}

impl<R: Read> Read for MeasurementReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self.inner.read(buf) {
            Ok(0) => {
                self.emit(CompletionState::Completed);
                Ok(0)
            }
            Ok(read) => {
                self.scanner.push(&buf[..read]);
                Ok(read)
            }
            Err(error) => {
                self.emit(CompletionState::from_read_error(&error));
                Err(error)
            }
        }
    }
}

impl<R: Read> Drop for MeasurementReader<R> {
    fn drop(&mut self) {
        // Only reached without an end of body or a read error: the body was abandoned, which for
        // a forwarding proxy means the peer went away mid-response.
        self.emit(CompletionState::Cancelled);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};

    fn collector() -> (Arc<Mutex<Vec<MeasurementEvent>>>, EventSink) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let handle = Arc::clone(&events);
        let sink: EventSink = Box::new(move |event| handle.lock().unwrap().push(event));
        (events, sink)
    }

    /// Reads a body through the measuring reader the way a forwarding proxy does, returning the
    /// bytes that were passed through and the events that were emitted.
    fn forward(body: &[u8], spec: MeasurementSpec) -> (Vec<u8>, Vec<MeasurementEvent>) {
        let (events, sink) = collector();
        let mut reader = MeasurementReader::new(Cursor::new(body.to_vec()), spec, sink);
        let mut forwarded = Vec::new();
        reader.read_to_end(&mut forwarded).unwrap();
        drop(reader);
        let emitted = events.lock().unwrap().clone();
        (forwarded, emitted)
    }

    /// Yields one byte per `read` call: the worst-case fragmentation a streaming body can have.
    struct OneByte<R: Read>(R);

    impl<R: Read> Read for OneByte<R> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let mut one = [0u8; 1];
            let read = self.0.read(&mut one)?;
            buf[..read].copy_from_slice(&one[..read]);
            Ok(read)
        }
    }

    /// Serves its whole body, then fails with `kind` — a stalled or reset upstream.
    struct FailsAfter<R: Read> {
        inner: R,
        buffered: Vec<u8>,
        kind: ErrorKind,
    }

    impl<R: Read> FailsAfter<R> {
        fn new(inner: R, kind: ErrorKind) -> Self {
            Self {
                inner,
                buffered: Vec::new(),
                kind,
            }
        }
    }

    impl<R: Read> Read for FailsAfter<R> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.buffered.is_empty() {
                let mut rest = Vec::new();
                self.inner.read_to_end(&mut rest)?;
                self.buffered = rest;
            }
            if self.buffered.is_empty() {
                return Err(std::io::Error::new(self.kind, "upstream went away"));
            }
            let read = buf.len().min(self.buffered.len());
            buf[..read].copy_from_slice(&self.buffered[..read]);
            self.buffered.drain(..read);
            Ok(read)
        }
    }

    fn spec() -> MeasurementSpec {
        MeasurementSpec::new("tc-test")
    }

    #[test]
    fn a_pretty_printed_json_body_is_read_as_one_document() {
        let body = b"{\n  \"choices\": [{\"message\": {\"content\": \"hi\"}}],\n  \"usage\": {\n    \"prompt_tokens\": 12,\n    \"completion_tokens\": 3,\n    \"total_tokens\": 15\n  }\n}\n";
        let (forwarded, events) = forward(body, spec());
        assert_eq!(forwarded, body);
        assert_eq!(events[0].usage_disposition, UsageDisposition::Reported);
        assert_eq!(events[0].provider_usage.unwrap().total(), Some(15));
    }

    #[test]
    fn a_json_document_over_the_scan_bound_stops_accounting_but_still_forwards() {
        let mut body = Vec::from(&b"{\"choices\":[{\"message\":{\"content\":\""[..]);
        body.extend(std::iter::repeat_n(b'x', MAX_SCAN_BYTES));
        body.extend_from_slice(b"\"}}],\"usage\":{\"total_tokens\":9}}");
        let (forwarded, events) = forward(&body, spec());
        assert_eq!(forwarded, body);
        assert_eq!(events[0].usage_disposition, UsageDisposition::Oversized);
        assert_eq!(events[0].provider_usage, None);
    }

    #[test]
    fn resolve_model_maps_known_families_and_never_infers_the_rest() {
        assert_eq!(
            resolve_model("gpt-4o-mini"),
            ModelResolution::Known {
                model: "gpt-4o-mini".to_string(),
                tokenizer: "o200k_base".to_string()
            }
        );
        // The longer family prefix must win over `gpt-4`.
        assert_eq!(
            resolve_model("gpt-4.1-2025-04-14"),
            ModelResolution::Known {
                model: "gpt-4.1-2025-04-14".to_string(),
                tokenizer: "o200k_base".to_string()
            }
        );
        assert_eq!(
            resolve_model("gpt-4-turbo"),
            ModelResolution::Known {
                model: "gpt-4-turbo".to_string(),
                tokenizer: "cl100k_base".to_string()
            }
        );
        assert_eq!(
            resolve_model("CLAUDE-3-5-SONNET"),
            ModelResolution::Unknown {
                requested: "CLAUDE-3-5-SONNET".to_string()
            }
        );
        // A locally served model gets no tokenizer claim either.
        assert_eq!(
            resolve_model("qwen2.5:7b"),
            ModelResolution::Unknown {
                requested: "qwen2.5:7b".to_string()
            }
        );
    }

    #[test]
    fn provider_delta_is_signed_in_both_directions() {
        let body = br#"{"usage":{"prompt_tokens":100,"completion_tokens":20,"total_tokens":120}}"#;
        // Local count larger than the provider's prompt count: a negative delta.
        let mut under = spec();
        under.local_after_tokens = Some(150);
        let (_, events) = forward(body, under);
        assert_eq!(events[0].provider_delta_tokens, Some(-50));

        // Local count smaller: positive. The provider's completion tokens never enter the delta,
        // which is why this is 100 - 60 rather than 120 - 60.
        let mut over = spec();
        over.local_after_tokens = Some(60);
        let (_, events) = forward(body, over);
        assert_eq!(events[0].provider_delta_tokens, Some(40));

        // Without a local count there is no delta to publish — not zero.
        let (_, events) = forward(body, spec());
        assert_eq!(events[0].provider_delta_tokens, None);
        assert_eq!(events[0].usage_disposition, UsageDisposition::Reported);
        assert_eq!(events[0].provider_usage.unwrap().total(), Some(120));
    }

    #[test]
    fn a_provider_that_reports_only_a_total_publishes_usage_but_no_delta() {
        let mut template = spec();
        template.local_after_tokens = Some(30);
        let (_, events) = forward(b"{\"usage\":{\"total_tokens\":42}}", template);
        assert_eq!(events[0].usage_disposition, UsageDisposition::Reported);
        assert_eq!(events[0].provider_usage.unwrap().total(), Some(42));
        assert_eq!(events[0].provider_delta_tokens, None);
    }

    #[test]
    fn fragmented_sse_usage_is_found_and_every_byte_is_forwarded() {
        let body = concat!(
            "event: message_start\r\n",
            "data: {\"type\":\"message_start\",\"usage\":{\"input_tokens\":11}}\r\n\r\n",
            ": keep-alive\n\n",
            "data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":7}}\n\n",
            "data: [DONE]\n\n"
        );
        let (events, sink) = collector();
        let mut reader =
            MeasurementReader::new(OneByte(Cursor::new(body.as_bytes().to_vec())), spec(), sink);
        let mut forwarded = Vec::new();
        reader.read_to_end(&mut forwarded).unwrap();
        drop(reader);

        assert_eq!(String::from_utf8(forwarded).unwrap(), body);
        let events = events.lock().unwrap().clone();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].completion, CompletionState::Completed);
        assert_eq!(events[0].usage_disposition, UsageDisposition::Reported);
        assert_eq!(
            events[0].provider_usage.unwrap(),
            ProviderUsage {
                prompt_tokens: Some(11),
                completion_tokens: Some(7),
                total_tokens: None
            }
        );
    }

    #[test]
    fn repeated_cumulative_snapshots_are_not_double_counted() {
        let snapshot = "data: {\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5}}\n\n";
        let body = format!("{snapshot}{snapshot}{snapshot}");
        let (_, events) = forward(body.as_bytes(), spec());
        assert_eq!(events.len(), 1);
        let usage = events[0].provider_usage.unwrap();
        assert_eq!(usage.prompt_tokens, Some(10));
        assert_eq!(usage.completion_tokens, Some(5));
        assert_eq!(usage.total(), Some(15));
    }

    #[test]
    fn staged_snapshots_merge_instead_of_overwriting_with_nulls() {
        let body = concat!(
            "data: {\"usage\":{\"prompt_tokens\":9,\"total_tokens\":9}}\n\n",
            "data: {\"usage\":{\"completion_tokens\":4,\"total_tokens\":13}}\n\n"
        );
        let (_, events) = forward(body.as_bytes(), spec());
        let usage = events[0].provider_usage.unwrap();
        assert_eq!(usage.prompt_tokens, Some(9));
        assert_eq!(usage.completion_tokens, Some(4));
        assert_eq!(usage.total(), Some(13));
    }

    #[test]
    fn missing_usage_stays_absent_rather_than_zero() {
        let body = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n";
        let (forwarded, events) = forward(body.as_bytes(), spec());
        assert_eq!(String::from_utf8(forwarded).unwrap(), body);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].usage_disposition, UsageDisposition::Absent);
        assert_eq!(events[0].provider_usage, None);
        assert_eq!(events[0].provider_delta_tokens, None);
    }

    #[test]
    fn malformed_usage_disables_accounting_without_touching_the_body() {
        // A valid snapshot followed by a corrupt event: the corrupt event wins, because a
        // partially readable body cannot support a trustworthy count.
        let body = concat!(
            "data: {\"usage\":{\"total_tokens\":42}}\n\n",
            "data: {not json at all\n\n",
            "data: [DONE]\n\n"
        );
        let (forwarded, events) = forward(body.as_bytes(), spec());
        assert_eq!(String::from_utf8(forwarded).unwrap(), body);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].usage_disposition, UsageDisposition::Malformed);
        assert_eq!(events[0].provider_usage, None);
        assert_eq!(events[0].completion, CompletionState::Completed);
    }

    #[test]
    fn usage_with_no_known_counter_counts_as_malformed() {
        let body = "data: {\"usage\":{\"prompt_tokens\":\"many\"}}\n\n";
        let (_, events) = forward(body.as_bytes(), spec());
        assert_eq!(events[0].usage_disposition, UsageDisposition::Malformed);
        assert_eq!(events[0].provider_usage, None);
    }

    #[test]
    fn null_usage_is_absent_not_malformed() {
        // Several providers emit `"usage": null` on every chunk but the last.
        let body = "data: {\"usage\":null}\n\ndata: {\"usage\":null}\n\n";
        let (_, events) = forward(body.as_bytes(), spec());
        assert_eq!(events[0].usage_disposition, UsageDisposition::Absent);
    }

    #[test]
    fn an_unterminated_body_over_the_scan_bound_stops_accounting_but_still_forwards() {
        let body = vec![b'x'; MAX_SCAN_BYTES + 2_048];
        let (forwarded, events) = forward(&body, spec());
        assert_eq!(forwarded, body);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].usage_disposition, UsageDisposition::Oversized);
        assert_eq!(events[0].provider_usage, None);
    }

    #[test]
    fn a_deadline_read_error_yields_one_timed_out_event() {
        let body = b"data: {\"usage\":{\"total_tokens\":5}}\n\n";
        let (events, sink) = collector();
        let mut reader = MeasurementReader::new(
            FailsAfter::new(Cursor::new(body.to_vec()), ErrorKind::TimedOut),
            spec(),
            sink,
        );
        let mut forwarded = Vec::new();
        let error = reader.read_to_end(&mut forwarded).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::TimedOut);
        drop(reader);

        let events = events.lock().unwrap().clone();
        assert_eq!(events.len(), 1, "a terminal state must be reported once");
        assert_eq!(events[0].completion, CompletionState::TimedOut);
        assert_eq!(events[0].usage_disposition, UsageDisposition::Reported);
    }

    #[test]
    fn abandoning_a_body_yields_one_cancelled_event() {
        let body = b"data: {\"usage\":{\"total_tokens\":5}}\n\ndata: still going\n\n";
        let (events, sink) = collector();
        {
            let mut reader = MeasurementReader::new(Cursor::new(body.to_vec()), spec(), sink);
            let mut first = [0u8; 8];
            reader.read_exact(&mut first).unwrap();
            // Drop without reaching the end of the body: the peer went away.
        }
        let events = events.lock().unwrap().clone();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].completion, CompletionState::Cancelled);
    }

    #[test]
    fn an_event_line_round_trips_and_an_unknown_version_is_refused() {
        let mut template = spec();
        template.session_id = Some(opaque_id("session-42"));
        template.model = Some(resolve_model("gpt-4o-mini"));
        template.estimator = Some(EstimatorInfo {
            backend: "tiktoken".to_string(),
            model: Some("o200k_base".to_string()),
            is_exact: true,
        });
        template.local_before_tokens = Some(120);
        template.local_after_tokens = Some(90);
        template.local_transform_micros = Some(410);
        let (_, events) = forward(
            b"data: {\"usage\":{\"total_tokens\":102}}\n\n",
            template.clone(),
        );
        let line = format_event_line(&events[0]).unwrap();
        assert!(line.starts_with(EVENT_LOG_PREFIX));
        assert_eq!(parse_event_line(&line).unwrap().unwrap(), events[0]);
        assert!(parse_event_line("GET /v1/chat/completions -> 200 (3ms)").is_none());

        let future = line.replace("\"1.0\"", "\"2.0\"");
        assert!(parse_event_line(&future).unwrap().is_err());
    }

    #[test]
    fn opaque_ids_are_stable_across_attempts_and_do_not_echo_the_raw_value() {
        assert_eq!(opaque_id("session-42"), opaque_id("session-42"));
        assert_ne!(opaque_id("session-42"), opaque_id("session-43"));
        assert_eq!(opaque_id("session-42").len(), 16);
        assert!(!opaque_id("sk-super-secret-session").contains("secret"));
    }

    #[test]
    fn local_counts_serialize_unavailable_fields_as_null_not_zero() {
        let mut template = spec();
        template.local_before_tokens = Some(40);
        let (_, events) = forward(b"data: [DONE]\n\n", template);
        let json = serde_json::to_value(&events[0]).unwrap();
        assert_eq!(json["local_before_tokens"], 40);
        assert!(json["local_after_tokens"].is_null());
        assert!(json["provider_usage"].is_null());
        assert!(json["provider_delta_tokens"].is_null());
        assert!(json["local_transform_micros"].is_null());
        assert_eq!(json["usage_disposition"], "absent");
        assert_eq!(json["completion"], "completed");
    }

    #[test]
    fn canonical_v1_measurement_event_fixture_round_trips_without_schema_drift() {
        let expected: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/measurement_event_v1.json"
        ))
        .unwrap();
        let event: MeasurementEvent = serde_json::from_value(expected.clone()).unwrap();
        assert_eq!(serde_json::to_value(event).unwrap(), expected);

        let schema: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/measurement_event_v1.schema.json"
        ))
        .unwrap();
        let expected_keys = expected.as_object().unwrap().keys().collect::<Vec<_>>();
        let schema_keys = schema["properties"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>();
        assert_eq!(schema_keys, expected_keys);
        assert_eq!(
            schema["required"].as_array().unwrap().len(),
            expected_keys.len()
        );
    }

    #[test]
    fn a_synthetic_session_reconciles_its_usage_aggregates_exactly() {
        // One session, three attempts: usage reported, usage absent, usage unusable. Only the
        // reported attempt contributes a provider number, and the deltas sum to exactly
        // (provider total - local total) because every delta is computed from the same two sides.
        let session = opaque_id("synthetic-session");
        let build = |attempt: u32, local_after: Option<usize>, body: &[u8]| {
            let mut template = spec();
            template.session_id = Some(session.clone());
            template.attempt = attempt;
            template.local_after_tokens = local_after;
            forward(body, template).1.remove(0)
        };
        let events = [
            build(
                1,
                Some(40),
                b"data: {\"usage\":{\"prompt_tokens\":90,\"completion_tokens\":12}}\n\n",
            ),
            build(
                2,
                Some(50),
                b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
            ),
            build(3, Some(7), b"data: {not json\n\n"),
        ];

        assert!(
            events
                .iter()
                .all(|event| event.session_id == events[0].session_id)
        );
        assert_eq!(
            events
                .iter()
                .map(|event| (event.attempt, event.usage_disposition))
                .collect::<Vec<_>>(),
            vec![
                (1, UsageDisposition::Reported),
                (2, UsageDisposition::Absent),
                (3, UsageDisposition::Malformed),
            ]
        );

        let provider_prompt_total: usize = events
            .iter()
            .filter_map(|event| event.provider_usage.and_then(|usage| usage.prompt_tokens))
            .sum();
        let provider_total_tokens: usize = events
            .iter()
            .filter_map(|event| event.provider_usage.and_then(|usage| usage.total()))
            .sum();
        let local_total: usize = events
            .iter()
            .filter_map(|event| event.local_after_tokens)
            .sum();
        let delta_total: i64 = events
            .iter()
            .filter_map(|event| event.provider_delta_tokens)
            .sum();
        // A delta exists only where both sides are known, so the reconciliation has to use that
        // same subset: folding the two usage-less attempts' local counts into the right-hand side
        // would show a discrepancy that is really just absence, not drift.
        let local_total_where_usage_reported: usize = events
            .iter()
            .filter(|event| event.provider_delta_tokens.is_some())
            .filter_map(|event| event.local_after_tokens)
            .sum();

        assert_eq!(provider_total_tokens, 102);
        assert_eq!(provider_prompt_total, 90);
        assert_eq!(local_total, 97);
        assert_eq!(local_total_where_usage_reported, 40);
        assert_eq!(
            delta_total,
            provider_prompt_total as i64 - local_total_where_usage_reported as i64
        );
    }
}
