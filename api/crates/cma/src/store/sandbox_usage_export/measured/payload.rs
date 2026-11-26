use super::*;
use layer2_core::sandbox_usage_evidence::MeasuredMinute;
use serde_json::{json, Value};

pub(super) fn fact(
    value: &MeasuredMinute,
    resource: &ResourceSnapshot,
    id: &str,
    revision: u64,
) -> Value {
    let metric = |index: usize| {
        let m = &value.metrics[index];
        json!({"covered_binding_ms":m.covered_binding_ms,"expected_binding_ms":m.expected_binding_ms,
            "quality":m.quality,"missing_reason":m.missing_reason})
    };
    let start = value.bucket_start * 1000;
    json!({"fact_id":id,"evidence_revision":revision.to_string(),"bucket_start_ms":start.to_string(),
        "start_ms":start.to_string(),"end_ms":(start+60_000).to_string(),
        "resource":{"runtime_id":resource.runtime_id,"runtime_generation":resource.runtime_generation.map(|v|v.to_string()),
            "home_id":resource.home_id,"home_generation":resource.home_generation},"allocation":null,
        "quantities":{"sandbox_cpu_core_nanoseconds_v1":value.metrics[0].quantity,
            "sandbox_memory_working_set_byte_milliseconds_v1":value.metrics[1].quantity,
            "sandbox_disk_used_byte_milliseconds_v1":value.metrics[2].quantity},
        "audit_time":{"compute_uptime_ms":value.compute_uptime_ms.to_string(),"retained_home_ms":value.retained_home_ms.to_string()},
        "coverage":{"cpu":metric(0),"memory":metric(1),"disk":metric(2)}})
}

pub(super) fn event(
    stream: &MeasuredStream,
    input: &MeasuredInput,
    event_id: &str,
    digest: &str,
    now: u64,
) -> Result<Value, SandboxStoreError> {
    if now < input.minute + 60 || stream.sequence >= i64::MAX as u64 {
        return Err(corrupt());
    }
    Ok(
        json!({"schema_version":1,"producer_namespace":stream.namespace,"stream_id":stream.id,"event_id":event_id,
        "sequence":(stream.sequence+1).to_string(),"previous_event_digest":stream.previous_digest,
        "owner_ref":{"issuer":stream.owner_issuer,"subject":stream.owner,"kind":"unresolved"},
        "sandbox_id":stream.sandbox_id,"source_epoch":stream.source_epoch,"basis":"measured",
        "metric_definition_version":"measured-v1","algorithm_version":"binding-integral-half-even-v1",
        "source_refs":[{"kind":"measured_manifest","id":digest,"revision":"1","part":"0"}],
        "source_digest":digest,"observed_at_ms":((input.minute+60)*1000).to_string(),
        "source_cutoff_ms":((input.minute+60)*1000).to_string(),"produced_at_ms":(now*1000).to_string()}),
    )
}

pub(super) fn open(mut event: Value, stream: &MeasuredStream) -> Value {
    let ranges = if stream.tracking_started_at < stream.capture_start {
        vec![
            json!({"start_ms":(stream.tracking_started_at*1000).to_string(),"end_ms":(stream.capture_start*1000).to_string()}),
        ]
    } else {
        vec![]
    };
    event["kind"] = "stream_open".into();
    event["opening"] = json!({"first_source":event["source_refs"][0],"capture_start_ms":(stream.capture_start*1000).to_string(),
        "tracking_started_at_ms":(stream.tracking_started_at*1000).to_string(),"complete_lifecycle":false,"historical_missing_ranges":ranges});
    event
}
