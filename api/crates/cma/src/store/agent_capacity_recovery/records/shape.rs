//! Check frozen format-2 BSON before Serde can normalize omitted/null/numbers.
use super::invalid;
use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::bson::{Bson, Document};

type Checked<T = ()> = Result<T, SandboxStoreError>;
const SEQUENCE: i64 = 9_007_199_254_740_991;

pub(super) fn validate(row: &Document) -> Checked {
    fields(
        row,
        &["_id", "format_version", "agent_id", "work", "result"],
        &[],
    )?;
    if row.get_i64("format_version").ok() != Some(2) {
        return Err(invalid());
    }
    let key = text(row, "_id", 82, 82)?;
    if !key.strip_prefix("capacity-recovery:").is_some_and(hex) {
        return Err(invalid());
    }
    text(row, "agent_id", 1, 512)?;
    let work = object(row, "work")?;
    fields(
        work,
        &[
            "format_version",
            "request",
            "target",
            "phase",
            "revision",
            "attempts",
            "next_fence",
            "lease",
            "ready_at",
            "completed_at",
        ],
        &[],
    )?;
    if work.get_i32("format_version").ok() != Some(1) {
        return Err(invalid());
    }
    request(object(work, "request")?)?;
    target(object(work, "target")?)?;
    if !matches!(
        text(work, "phase", 1, 16)?,
        "admitted" | "attaching" | "completed" | "superseded"
    ) {
        return Err(invalid());
    }
    long(work, "revision", 1, i64::MAX)?;
    long(work, "attempts", 0, i64::from(u32::MAX))?;
    long(work, "next_fence", 1, i64::MAX)?;
    match work.get("lease") {
        Some(Bson::Null) => {}
        Some(Bson::Document(lease)) => {
            fields(lease, &["holder", "fence", "expires_at"], &[])?;
            text(lease, "holder", 1, 256)?;
            long(lease, "fence", 1, i64::MAX)?;
            time(lease, "expires_at")?;
        }
        _ => return Err(invalid()),
    }
    time(work, "ready_at")?;
    match work.get("completed_at") {
        Some(Bson::Null) => {}
        Some(Bson::String(_)) => time(work, "completed_at")?,
        _ => return Err(invalid()),
    }
    let result = object(row, "result")?;
    fields(
        result,
        &[
            "agent_digest",
            "logs_digest",
            "agent_revision",
            "through_seq",
            "published_at",
        ],
        &[],
    )?;
    for key in ["agent_digest", "logs_digest"] {
        if result
            .get_binary_generic(key)
            .map_or(true, |bytes| bytes.len() != 32)
        {
            return Err(invalid());
        }
    }
    long(result, "agent_revision", 1, i64::MAX)?;
    long(result, "through_seq", 2, SEQUENCE)?;
    time(result, "published_at")
}

fn request(row: &Document) -> Checked {
    fields(
        row,
        &[
            "agent_id",
            "creation_key",
            "creation_fingerprint",
            "failure_seq",
            "requested_at",
        ],
        &[],
    )?;
    identity(row)?;
    long(row, "failure_seq", 2, SEQUENCE)?;
    time(row, "requested_at")
}

fn target(row: &Document) -> Checked {
    fields(
        row,
        &[
            "agent_id",
            "creation_key",
            "creation_fingerprint",
            "conversation_id",
            "conversation_generation",
            "runtime",
            "home_identity",
            "original_open",
            "active_turn",
            "input_sha256",
        ],
        &[],
    )?;
    identity(row)?;
    text(row, "conversation_id", 1, 512)?;
    long(row, "conversation_generation", 1, i64::MAX)?;
    text(row, "home_identity", 1, 16_384)?;
    digest(row, "input_sha256")?;
    match row.get("active_turn") {
        Some(Bson::Null) => {}
        Some(Bson::Int64(_)) => long(row, "active_turn", 1, SEQUENCE)?,
        _ => return Err(invalid()),
    }
    let runtime = object(row, "runtime")?;
    fields(
        runtime,
        &["runtime_id", "runtime_version", "bound_at"],
        &["boot_id", "provision_generation"],
    )?;
    text(runtime, "runtime_id", 1, 16_384)?;
    text(runtime, "runtime_version", 1, 16_384)?;
    time(runtime, "bound_at")?;
    // Runtime optionals use omission, never explicit BSON null.
    if runtime.contains_key("boot_id") {
        text(runtime, "boot_id", 1, 32)?;
    }
    if runtime.contains_key("provision_generation") {
        long(runtime, "provision_generation", 1, i64::MAX)?;
    }
    let open = object(row, "original_open")?;
    fields(
        open,
        &["step", "operation_id", "request_sha256", "completed_at"],
        &[],
    )?;
    if text(open, "step", 1, 32)? != "open_conversation" {
        return Err(invalid());
    }
    text(open, "operation_id", 1, 545)?;
    digest(open, "request_sha256")?;
    time(open, "completed_at")
}

fn identity(row: &Document) -> Checked {
    text(row, "agent_id", 1, 512)?;
    text(row, "creation_key", 1, 256)?;
    digest(row, "creation_fingerprint")
}

fn fields(row: &Document, required: &[&str], optional: &[&str]) -> Checked {
    if required.iter().any(|key| !row.contains_key(*key))
        || row
            .keys()
            .any(|key| !required.contains(&key.as_str()) && !optional.contains(&key.as_str()))
    {
        return Err(invalid());
    }
    Ok(())
}

fn object<'a>(row: &'a Document, key: &str) -> Checked<&'a Document> {
    row.get_document(key).map_err(|_| invalid())
}

fn text<'a>(row: &'a Document, key: &str, min: usize, max: usize) -> Checked<&'a str> {
    let value = row.get_str(key).map_err(|_| invalid())?;
    if !(min..=max).contains(&value.chars().count()) {
        return Err(invalid());
    }
    Ok(value)
}

fn long(row: &Document, key: &str, min: i64, max: i64) -> Checked {
    if !row.get_i64(key).is_ok_and(|n| (min..=max).contains(&n)) {
        return Err(invalid());
    }
    Ok(())
}

fn hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn digest(row: &Document, key: &str) -> Checked {
    if !hex(text(row, key, 64, 64)?) {
        return Err(invalid());
    }
    Ok(())
}

fn time(row: &Document, key: &str) -> Checked {
    if layer2_core::seconds_from_rfc3339(text(row, key, 20, 20)?).is_none() {
        return Err(invalid());
    }
    Ok(())
}
