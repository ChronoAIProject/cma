use super::{records::hash, *};

/// The retained producer bytes are the transport input, including on retry.
#[derive(Clone, Debug)]
pub(crate) struct FrozenUsageEvent {
    pub event_id: String,
    pub stream_id: String,
    pub namespace: String,
    pub sequence: u64,
    pub digest: String,
    pub produced_at_ms: u64,
    pub payload: Vec<u8>,
}

impl FrozenUsageEvent {
    pub(super) fn read(document: &Document) -> Result<Self, SandboxStoreError> {
        let payload = document
            .get_binary_generic("payload")
            .map_err(|_| corrupt())?;
        if payload.is_empty()
            || payload.len() > 512 * 1024
            || document.get_i64("document_schema_version").ok() != Some(1)
            || document.get_str("charge_disposition").ok() != Some("nonbillable_shadow")
        {
            return Err(corrupt());
        }
        let value: serde_json::Value = serde_json::from_slice(payload).map_err(|_| corrupt())?;
        if serde_json_canonicalizer::to_vec(&value).map_err(|_| corrupt())? != *payload {
            return Err(corrupt());
        }
        let field = |key| value[key].as_str().ok_or_else(corrupt);
        let event_id = field("event_id")?;
        let stream_id = field("stream_id")?;
        for id in [event_id, stream_id] {
            if !uuid::Uuid::parse_str(id)
                .is_ok_and(|v| v.get_version_num() == 4 && v.to_string() == id)
            {
                return Err(corrupt());
            }
        }
        let sequence = integer(field("sequence")?)?;
        let produced_at_ms = integer(field("produced_at_ms")?)?;
        let digest = hash(payload);
        if value["schema_version"] != 1
            || sequence == 0
            || document.get_str("_id").ok() != Some(event_id)
            || document.get_str("stream_id").ok() != Some(stream_id)
            || document.get_i64("sequence").ok() != i64::try_from(sequence).ok()
            || document.get_str("digest").ok() != Some(digest.as_str())
            || document.get_str("owner").ok() != value["owner_ref"]["subject"].as_str()
            || document.get_str("sandbox_id").ok() != value["sandbox_id"].as_str()
        {
            return Err(corrupt());
        }
        Ok(Self {
            event_id: event_id.into(),
            stream_id: stream_id.into(),
            namespace: field("producer_namespace")?.into(),
            sequence,
            digest,
            produced_at_ms,
            payload: payload.clone(),
        })
    }
}

fn integer(value: &str) -> Result<u64, SandboxStoreError> {
    let parsed = value.parse::<u64>().map_err(|_| corrupt())?;
    if parsed > i64::MAX as u64 || parsed.to_string() != value {
        return Err(corrupt());
    }
    Ok(parsed)
}
