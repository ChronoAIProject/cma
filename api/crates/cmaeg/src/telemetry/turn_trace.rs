//! Reader-first, metadata-only NyxID turn trace propagation.
//!
//! A trace parent is diagnostic input after callback authentication. It never
//! selects a channel, consumer, event or idempotency key. Records use a small
//! bounded queue and a closed JSON schema so a full writer cannot delay or
//! change delivery.

use cmaeg_core::seal::TurnTraceContext;
use serde::Serialize;
use std::{
    io::{self, Write},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, SyncSender},
        OnceLock,
    },
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

pub const TARGET: &str = "nyxid::turn_trace";
const CAPACITY: usize = 4096;
static WRITER: OnceLock<SyncSender<Record>> = OnceLock::new();
static INSTANCE: OnceLock<String> = OnceLock::new();
static DROPPED: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Context {
    pub trace_id: String,
    pub parent_span_id: String,
    pub ingress_request_id: Option<String>,
}

impl Context {
    pub(crate) fn from_wire(value: TurnTraceContext) -> Self {
        Self {
            trace_id: value.trace_id,
            parent_span_id: value.parent_span_id,
            ingress_request_id: value.ingress_request_id,
        }
    }

    pub(crate) fn wire(&self) -> TurnTraceContext {
        TurnTraceContext {
            trace_id: self.trace_id.clone(),
            parent_span_id: self.parent_span_id.clone(),
            ingress_request_id: self.ingress_request_id.clone(),
        }
    }

    pub(crate) fn child_span(&self) -> String {
        let bytes = Uuid::new_v4().as_bytes()[..8]
            .try_into()
            .expect("eight bytes");
        format!("{:016x}", u64::from_be_bytes(bytes).max(1))
    }

    fn turn_id(&self) -> Option<String> {
        let mut value = self.trace_id.clone();
        if value.len() != 32 || value.bytes().any(|byte| !byte.is_ascii_hexdigit()) {
            return None;
        }
        value.insert(8, '-');
        value.insert(13, '-');
        value.insert(18, '-');
        value.insert(23, '-');
        let uuid = Uuid::parse_str(&value).ok()?;
        (uuid.get_version_num() == 4).then_some(uuid.to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GapReason {
    MissingParent,
    InvalidParent,
}
impl GapReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::MissingParent => "missing_parent",
            Self::InvalidParent => "invalid_parent",
        }
    }
}

pub(crate) fn parse_parent(value: Option<&str>) -> Result<Option<Context>, GapReason> {
    let Some(value) = value else {
        return Err(GapReason::MissingParent);
    };
    let fields: Vec<_> = value.split('-').collect();
    if fields.len() != 4
        || fields[0] != "00"
        || fields[1].len() != 32
        || fields[2].len() != 16
        || fields[3].len() != 2
        || !hex_nonzero(fields[1])
        || !hex_nonzero(fields[2])
        || !lower_hex(fields[3])
    {
        return Err(GapReason::InvalidParent);
    }
    Ok(Some(Context {
        trace_id: fields[1].to_owned(),
        parent_span_id: fields[2].to_owned(),
        ingress_request_id: None,
    }))
}

fn hex_nonzero(value: &str) -> bool {
    lower_hex(value) && value.bytes().any(|b| b != b'0')
}

fn lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || byte.is_ascii_lowercase() && byte <= b'f')
}

#[derive(Serialize)]
struct Record {
    schema_version: u8,
    target: &'static str,
    service: &'static str,
    instance_id: String,
    timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    span_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_span_id: Option<String>,
    request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    ingress_request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    related_turn_id: Option<String>,
    phase: &'static str,
    stage: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    part_index: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_code: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gap_reason: Option<&'static str>,
}

fn now() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(millis) * 1_000_000)
        .ok()
        .and_then(|value| {
            value
                .format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".into())
}

fn instance() -> String {
    INSTANCE.get_or_init(|| Uuid::new_v4().to_string()).clone()
}

fn fresh_span() -> String {
    let bytes = Uuid::new_v4().as_bytes()[..8]
        .try_into()
        .expect("eight bytes");
    format!("{:016x}", u64::from_be_bytes(bytes).max(1))
}

fn bounded_error_code(code: u32) -> Option<u32> {
    matches!(
        code,
        400 | 401 | 403 | 404 | 409 | 422 | 429 | 500 | 502 | 503
    )
    .then_some(code)
}

fn bounded_outcome(outcome: Option<&'static str>) -> Option<&'static str> {
    outcome.filter(|value| {
        matches!(
            *value,
            "completed"
                | "failed"
                | "cancelled"
                | "unknown"
                | "streamed"
                | "silent"
                | "confirmed"
                | "rejected"
                | "partial"
        )
    })
}

fn emit(record: Record) {
    let Some(sender) = WRITER.get() else {
        return;
    };
    if sender.try_send(record).is_err() {
        let dropped = DROPPED.fetch_add(1, Ordering::Relaxed) + 1;
        // A full queue is deliberately non-blocking. Emit a sparse health
        // signal so operators can distinguish partial traces from no traffic.
        if dropped.is_power_of_two() {
            tracing::warn!(dropped, "nyxid turn trace records are being dropped");
        }
    }
}

pub(crate) fn start() {
    if WRITER.get().is_some() {
        return;
    }
    let (sender, receiver) = mpsc::sync_channel(CAPACITY);
    if WRITER.set(sender).is_err() {
        return;
    }
    std::thread::Builder::new()
        .name("cmaeg-nyxid-trace".into())
        .spawn(move || {
            let stdout = io::stdout();
            while let Ok(record) = receiver.recv() {
                if let Ok(mut bytes) = serde_json::to_vec(&record) {
                    if bytes.len() <= 2048 {
                        bytes.push(b'\n');
                        // Ordinary service logs share stdout. Own its lock only
                        // while emitting one complete line, never while waiting
                        // for the next trace record.
                        let mut output = stdout.lock();
                        let _ = output.write_all(&bytes);
                        let _ = output.flush();
                    }
                }
            }
        })
        .expect("trace writer thread");
}

pub(crate) fn ingress_gap(reason: GapReason) {
    emit(Record {
        schema_version: 1,
        target: TARGET,
        service: "cma",
        instance_id: instance(),
        timestamp: now(),
        turn_id: None,
        trace_id: None,
        span_id: Some(fresh_span()),
        parent_span_id: None,
        request_id: Uuid::new_v4().to_string(),
        ingress_request_id: None,
        related_turn_id: None,
        phase: "link",
        stage: "ingress",
        part_index: None,
        outcome: None,
        duration_ms: None,
        http_status: None,
        error_code: None,
        gap_reason: Some(reason.as_str()),
    });
}

pub(crate) fn milestone(
    context: Option<&Context>,
    stage: &'static str,
    phase: &'static str,
    outcome: Option<&'static str>,
    status: Option<u16>,
    code: Option<u32>,
) {
    milestone_record(context, stage, phase, outcome, status, code, None);
}

pub(crate) fn milestone_duration(
    context: Option<&Context>,
    stage: &'static str,
    phase: &'static str,
    outcome: Option<&'static str>,
    status: Option<u16>,
    code: Option<u32>,
    started: std::time::Instant,
) {
    milestone_record(
        context,
        stage,
        phase,
        outcome,
        status,
        code,
        Some(started.elapsed()),
    );
}

fn milestone_record(
    context: Option<&Context>,
    stage: &'static str,
    phase: &'static str,
    outcome: Option<&'static str>,
    status: Option<u16>,
    code: Option<u32>,
    duration: Option<std::time::Duration>,
) {
    emit(Record {
        schema_version: 1,
        target: TARGET,
        service: "cma",
        instance_id: instance(),
        timestamp: now(),
        turn_id: context.and_then(Context::turn_id),
        trace_id: context.map(|value| value.trace_id.clone()),
        span_id: context.map(Context::child_span),
        parent_span_id: context.map(|value| value.parent_span_id.clone()),
        request_id: Uuid::new_v4().to_string(),
        ingress_request_id: context.and_then(|value| value.ingress_request_id.clone()),
        related_turn_id: None,
        phase,
        stage,
        part_index: None,
        outcome: bounded_outcome(outcome),
        duration_ms: duration.map(|value| value.as_millis().min(u128::from(u64::MAX)) as u64),
        http_status: status,
        error_code: code.and_then(bounded_error_code),
        gap_reason: None,
    });
}

/// Build the child W3C parent sent to NyxID while retaining the admitted
/// upstream trace identity. The span id is fresh for every relay exchange.
pub(crate) fn child_traceparent(value: &TurnTraceContext) -> String {
    let context = Context::from_wire(value.clone());
    format!("00-{}-{}-01", context.trace_id, context.child_span())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_trace_writer_releases_stdout_for_service_logs() {
        use std::io::BufRead;
        use std::process::{Command, Stdio};
        use std::time::Duration;

        const PROBE: &str = "CMAEG_TRACE_STDOUT_PROBE";
        const MARKER: &str = "ordinary service log after trace";
        if std::env::var_os(PROBE).is_some() {
            start();
            ingress_gap(GapReason::MissingParent);
            let mut acknowledgement = String::new();
            io::stdin().read_line(&mut acknowledgement).unwrap();
            writeln!(io::stdout(), "{MARKER}").unwrap();
            return;
        }

        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "telemetry::turn_trace::tests::idle_trace_writer_releases_stdout_for_service_logs",
                "--nocapture",
            ])
            .env(PROBE, "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, receive) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in io::BufReader::new(stdout).lines() {
                if send.send(line.unwrap()).is_err() {
                    break;
                }
            }
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut saw_trace = false;
        let mut saw_service_log = false;
        while let Ok(line) =
            receive.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
        {
            if !saw_trace && line.contains("\"target\":\"nyxid::turn_trace\"") {
                saw_trace = true;
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(b"continue\n")
                    .unwrap();
            }
            if line.contains(MARKER) {
                saw_service_log = true;
                break;
            }
        }
        if !saw_service_log {
            let _ = child.kill();
        }
        let status = child.wait().unwrap();
        reader.join().unwrap();
        assert!(saw_trace, "background writer must emit its trace record");
        assert!(
            saw_service_log,
            "idle trace writer must release stdout for service logs"
        );
        assert!(status.success());
    }

    #[test]
    fn traceparent_is_strict_and_never_echoes_invalid_input() {
        let valid = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        let parsed = parse_parent(Some(valid)).unwrap().unwrap();
        assert_eq!(parsed.trace_id, "4bf92f3577b34da6a3ce929d0e0e4736");
        for invalid in [
            "",
            "01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-zz",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-secret",
            "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01",
        ] {
            assert!(parse_parent(Some(invalid)).is_err(), "{invalid}");
        }
        assert_eq!(parse_parent(None), Err(GapReason::MissingParent));
    }

    #[test]
    fn numeric_error_codes_are_allowlisted() {
        assert_eq!(bounded_error_code(403), Some(403));
        assert!(bounded_error_code(0).is_none());
        assert!(bounded_error_code(418).is_none());
    }

    #[test]
    fn child_traceparent_preserves_trace_and_rotates_span() {
        let context = TurnTraceContext {
            trace_id: "4bf92f3577b34da6a3ce929d0e0e4736".into(),
            parent_span_id: "00f067aa0ba902b7".into(),
            ingress_request_id: None,
        };
        let first = child_traceparent(&context);
        let second = child_traceparent(&context);
        assert!(first.starts_with("00-4bf92f3577b34da6a3ce929d0e0e4736-"));
        assert!(second.starts_with("00-4bf92f3577b34da6a3ce929d0e0e4736-"));
        assert_ne!(first, second);
        assert_eq!(first.len(), 55);
    }
}
