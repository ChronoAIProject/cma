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
}

impl Context {
    pub(crate) fn from_wire(value: TurnTraceContext) -> Self {
        Self {
            trace_id: value.trace_id,
            parent_span_id: value.parent_span_id,
        }
    }

    pub(crate) fn wire(&self) -> TurnTraceContext {
        TurnTraceContext {
            trace_id: self.trace_id.clone(),
            parent_span_id: self.parent_span_id.clone(),
        }
    }

    pub(crate) fn child_span(&self) -> String {
        let bytes = Uuid::new_v4().as_bytes()[..8]
            .try_into()
            .expect("eight bytes");
        format!("{:016x}", u64::from_be_bytes(bytes).max(1))
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
    trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    span_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_span_id: Option<String>,
    request_id: String,
    phase: &'static str,
    stage: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_code: Option<String>,
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
        .and_then(|value| value.format(&time::format_description::well_known::Rfc3339).ok())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".into())
}

fn instance() -> String {
    INSTANCE
        .get_or_init(|| Uuid::new_v4().to_string())
        .clone()
}

fn fresh_span() -> String {
    let bytes = Uuid::new_v4().as_bytes()[..8]
        .try_into()
        .expect("eight bytes");
    format!("{:016x}", u64::from_be_bytes(bytes).max(1))
}

fn bounded_code(code: &str) -> Option<String> {
    (1..=64)
        .contains(&code.len())
        .then_some(())
        .filter(|_| {
            code.bytes().all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b)
            })
        })
        .map(|_| code.to_owned())
}

fn emit(record: Record) {
    let Some(sender) = WRITER.get() else {
        return;
    };
    if sender.try_send(record).is_err() {
        DROPPED.fetch_add(1, Ordering::Relaxed);
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
            let mut output = io::BufWriter::new(stdout.lock());
            while let Ok(record) = receiver.recv() {
                if let Ok(bytes) = serde_json::to_vec(&record) {
                    if bytes.len() <= 2048 {
                        let _ = output.write_all(&bytes);
                        let _ = output.write_all(b"\n");
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
        trace_id: None,
        span_id: Some(fresh_span()),
        parent_span_id: None,
        request_id: Uuid::new_v4().to_string(),
        phase: "link",
        stage: "ingress",
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
    code: Option<&str>,
) {
    emit(Record {
        schema_version: 1,
        target: TARGET,
        service: "cma",
        instance_id: instance(),
        timestamp: now(),
        trace_id: context.map(|value| value.trace_id.clone()),
        span_id: context.map(Context::child_span),
        parent_span_id: context.map(|value| value.parent_span_id.clone()),
        request_id: Uuid::new_v4().to_string(),
        phase,
        stage,
        outcome,
        duration_ms: None,
        http_status: status,
        error_code: code.and_then(bounded_code),
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
    fn provider_codes_are_bounded_and_redacted() {
        assert_eq!(bounded_code("media_source_refused").as_deref(), Some("media_source_refused"));
        assert!(bounded_code("").is_none());
        assert!(bounded_code("UPPER").is_none());
        assert!(bounded_code("private detail").is_none());
        let oversized = "a".repeat(65);
        assert!(bounded_code(&oversized).is_none());
    }

    #[test]
    fn child_traceparent_preserves_trace_and_rotates_span() {
        let context = TurnTraceContext {
            trace_id: "4bf92f3577b34da6a3ce929d0e0e4736".into(),
            parent_span_id: "00f067aa0ba902b7".into(),
        };
        let first = child_traceparent(&context);
        let second = child_traceparent(&context);
        assert!(first.starts_with("00-4bf92f3577b34da6a3ce929d0e0e4736-"));
        assert!(second.starts_with("00-4bf92f3577b34da6a3ce929d0e0e4736-"));
        assert_ne!(first, second);
        assert_eq!(first.len(), 55);
    }
}
