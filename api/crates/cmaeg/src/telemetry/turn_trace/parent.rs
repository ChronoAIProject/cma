use axum::http::HeaderMap;
use std::fmt;
use uuid::Uuid;

/// A nonzero W3C span ID, independent of every gateway/platform identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SpanId(u64);

impl SpanId {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        if value.len() != 16 || !lower_hex(value) {
            return None;
        }
        let value = u64::from_str_radix(value, 16).ok()?;
        (value != 0).then_some(Self(value))
    }

    pub(crate) fn fresh() -> Self {
        let random = Uuid::new_v4();
        let bytes = random.as_bytes()[..8].try_into().expect("eight bytes");
        Self(u64::from_be_bytes(bytes).max(1))
    }
}

impl fmt::Display for SpanId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

/// Validated headers remain diagnostics; they never establish a provider
/// admission association or change a body, signature, claim or idempotency key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteParent {
    pub trace_id: Uuid,
    pub span_id: SpanId,
    pub ingress_request_id: Option<Uuid>,
}

impl RemoteParent {
    /// Call only after the endpoint's ordinary authorization succeeds and while
    /// tracing is enabled. Future versions retain the W3C-defined fixed prefix;
    /// unknown extension fields are never interpreted or forwarded.
    pub(crate) fn from_headers(headers: &HeaderMap) -> Option<Self> {
        let value = single(headers, "traceparent")?;
        if value.len() < 55 || !value.is_ascii() {
            return None;
        }
        let mut fields = value.split('-');
        let version = fields.next()?;
        if version.len() != 2 || !lower_hex(version) || version == "ff" {
            return None;
        }
        let trace = fields.next()?;
        let span = fields.next()?;
        let flags = fields.next()?;
        if (version == "00" && fields.next().is_some())
            || trace.len() != 32
            || !lower_hex(trace)
            || flags.len() != 2
            || !lower_hex(flags)
        {
            return None;
        }
        let trace_id = Uuid::parse_str(trace).ok()?;
        if trace_id.is_nil() {
            return None;
        }
        Some(Self {
            trace_id,
            span_id: SpanId::parse(span)?,
            ingress_request_id: single(headers, "x-request-id").and_then(uuid_v4),
        })
    }
}

pub(super) fn single<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    (values.next().is_none() && value.len() <= 256).then_some(value)
}

fn lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

pub(super) fn uuid_v4(value: &str) -> Option<Uuid> {
    if value.len() != 36 {
        return None;
    }
    let id = Uuid::parse_str(value).ok()?;
    (id.get_version_num() == 4
        && id.get_variant() == uuid::Variant::RFC4122
        && id.hyphenated().to_string() == value)
        .then_some(id)
}
