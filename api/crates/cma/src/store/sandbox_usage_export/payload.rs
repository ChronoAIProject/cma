use super::{
    records::{Captured, Stream},
    *,
};
use layer2_core::sandbox_usage_evidence::MinuteFact;
use serde_json::{json, Value};

fn ms(seconds: u64) -> Result<String, SandboxStoreError> {
    seconds
        .checked_mul(1000)
        .filter(|v| *v <= i64::MAX as u64)
        .map(|v| v.to_string())
        .ok_or_else(corrupt)
}

pub(super) struct Ids {
    pub opening: String,
    pub usage: String,
    pub facts: Vec<String>,
}

impl Ids {
    pub fn new() -> Self {
        Self {
            opening: uuid::Uuid::new_v4().to_string(),
            usage: uuid::Uuid::new_v4().to_string(),
            facts: (0..layer2_core::sandbox_usage_evidence::MAX_FACTS)
                .map(|_| uuid::Uuid::new_v4().to_string())
                .collect(),
        }
    }
}

fn header(
    stream: &Stream,
    capture: &Captured,
    id: &str,
    part: u64,
    now: u64,
) -> Result<Value, SandboxStoreError> {
    let source = &capture.source;
    let cutoff = source.ended_at;
    if now < cutoff {
        return Err(corrupt());
    }
    let sequence = stream
        .sequence
        .checked_add(1)
        .filter(|v| *v <= i64::MAX as u64)
        .ok_or_else(corrupt)?;
    Ok(json!({
        "schema_version":1,"producer_namespace":stream.namespace,
        "stream_id":stream.id,"event_id":id,"sequence":sequence.to_string(),
        "previous_event_digest":stream.previous_digest,
        "owner_ref":{"issuer":stream.owner_issuer,"subject":stream.owner,"kind":"unresolved"},
        "sandbox_id":stream.sandbox_id,"source_epoch":stream.source_epoch,"basis":"allocated",
        "metric_definition_version":"allocated-v1","algorithm_version":"lifecycle-minute-split-v1",
        "source_refs":[{"kind":"lifecycle","id":source.transaction_id(),
            "revision":source.revision.to_string(),"part":part.to_string()}],
        "source_digest":records::hash(&serde_json::to_vec(capture).map_err(|_| corrupt())?),
        "observed_at_ms":ms(cutoff)?,"source_cutoff_ms":ms(cutoff)?,"produced_at_ms":ms(now)?,
    }))
}

pub(super) fn opening(
    stream: &Stream,
    capture: &Captured,
    id: &str,
    now: u64,
) -> Result<Value, SandboxStoreError> {
    let mut event = header(stream, capture, id, 0, now)?;
    let ranges = if stream.tracking_started_at < stream.capture_start {
        vec![json!({"start_ms":ms(stream.tracking_started_at)?,"end_ms":ms(stream.capture_start)?})]
    } else {
        vec![]
    };
    event["kind"] = "stream_open".into();
    event["opening"] = json!({"first_source":event["source_refs"][0],
        "capture_start_ms":ms(stream.capture_start)?,"tracking_started_at_ms":ms(stream.tracking_started_at)?,
        "complete_lifecycle":stream.complete_lifecycle,"historical_missing_ranges":ranges});
    Ok(event)
}

pub(super) fn usage(
    stream: &Stream,
    capture: &Captured,
    facts: &[MinuteFact],
    ids: &Ids,
    part: u64,
    now: u64,
) -> Result<Value, SandboxStoreError> {
    let mut event = header(stream, capture, &ids.usage, part, now)?;
    let mut values: Vec<Value> = facts
        .iter()
        .zip(&ids.facts)
        .map(|(fact, id)| fact_value(capture, fact, id))
        .collect::<Result<_, _>>()?;
    values.sort_by(|a, b| a["fact_id"].as_str().cmp(&b["fact_id"].as_str()));
    event["kind"] = "usage".into();
    event["facts"] = values.into();
    Ok(event)
}

pub(super) fn fact_value(
    capture: &Captured,
    fact: &MinuteFact,
    id: &str,
) -> Result<Value, SandboxStoreError> {
    let resource = &capture.resource;
    let allocation = &capture.source.allocation;
    if (allocation.running
        && (resource.runtime_id.is_none() || resource.runtime_generation.is_none()))
        || (allocation.disk_gib > 0
            && (resource.home_id.is_none() || resource.home_generation.is_none()))
    {
        return Err(SandboxStoreError::Unavailable(
            "allocated usage binding evidence missing".into(),
        ));
    }
    let runtime_binding = resource
        .runtime_id
        .as_deref()
        .zip(resource.runtime_generation);
    let home_binding = resource
        .home_id
        .as_deref()
        .zip(resource.home_generation.as_deref());
    let coverage = |seconds| -> Result<Value, SandboxStoreError> {
        Ok(
            json!({"covered_binding_ms":ms(seconds)?,"expected_binding_ms":ms(seconds)?,
            "quality":"observed","missing_reason":null}),
        )
    };
    Ok(
        json!({"fact_id":id,"evidence_revision":"1","bucket_start_ms":ms(fact.bucket_start)?,
        "start_ms":ms(fact.started_at)?,"end_ms":ms(fact.ended_at)?,
        "resource":{"runtime_id":runtime_binding.map(|(id, _)|id),
            "runtime_generation":runtime_binding.map(|(_, generation)|generation.to_string()),
            "home_id":home_binding.map(|(id, _)|id),
            "home_generation":home_binding.map(|(_, generation)|generation)},
        "allocation":{"cpu_millis":allocation.cpu_millis.to_string(),
            "memory_mib":allocation.memory_mib.to_string(),"disk_gib":allocation.disk_gib.to_string()},
        "quantities":{"sandbox_cpu_millis_seconds":fact.quantity.cpu_millis_seconds.to_string(),
            "sandbox_memory_mib_seconds":fact.quantity.memory_mib_seconds.to_string(),
            "sandbox_disk_gib_seconds":fact.quantity.disk_gib_seconds.to_string()},
        "audit_time":{"compute_uptime_ms":ms(fact.quantity.uptime_seconds)?,"retained_home_ms":ms(fact.retained_home_seconds)?},
        "coverage":{"cpu":coverage(fact.quantity.uptime_seconds)?,"memory":coverage(fact.quantity.uptime_seconds)?,
            "disk":coverage(fact.retained_home_seconds)?}}),
    )
}
