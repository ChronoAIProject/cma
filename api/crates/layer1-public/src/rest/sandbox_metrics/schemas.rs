//! Complete public schemas, including explicit gaps and lossless large integers.
use serde_json::{json, Value};
fn reference(name: &str) -> Value {
    json!({"$ref":format!("#/components/schemas/{name}")})
}
fn nullable(schema: Value) -> Value {
    json!({"oneOf":[schema,{"type":"null"}]})
}
fn array(schema: Value) -> Value {
    json!({"type":"array","items":schema})
}
fn object(properties: Value) -> Value {
    let required: Vec<_> = properties.as_object().unwrap().keys().cloned().collect();
    json!({"type":"object","additionalProperties":false,"required":required,"properties":properties})
}
pub(super) fn schemas() -> Value {
    let text = json!({"type":"string"});
    let boolean = json!({"type":"boolean"});
    let quantity = reference("UsageMetricQuantity");
    let integer = reference("UsageMetricInteger");
    let status = json!({"type":"string","enum":["provisioning","installing","active","pausing","paused","reviving","credential_rotating","rotating","failed","retiring","retired"]});
    let filter_properties = json!({"owner":nullable(json!({"type":"string","minLength":1,"maxLength":256})),
        "statuses":array(status.clone()),"runtime_version":nullable(json!({"type":"string","minLength":1,"maxLength":512})),
        "sandbox_id":nullable(json!({"type":"string","pattern":"^sbx_[a-f0-9]{32}$"})),
        "agent_id":nullable(json!({"type":"string","pattern":"^agt_[a-f0-9]{32}$"})),
        "search":{"type":"string","maxLength":128,"description":"Literal normalized name or ID prefix, never a regular expression."}});
    let mut filters = object(filter_properties.clone());
    filters.as_object_mut().unwrap().remove("required");
    let range = object(json!({"from":integer,"to":integer}));
    let step = json!({"type":"integer","enum":[60,120,180,240,300,600,900,1200,1800,3600,7200,10800,14400,21600,43200,86400],
        "description":"Day-dividing multiple of canonical 60/300/3600 seconds. Range and epoch boundaries determine the allowed minimum; at most 2016 points."});
    let mut input =
        object(json!({"from":integer,"to":integer,"step_seconds":step,"filters":filters}));
    input.as_object_mut().unwrap().remove("required");
    let canonical = object(
        json!({"from":integer,"to":integer,"step_seconds":step,"filters":object(filter_properties)}),
    );
    let capacity =
        object(json!({"cpu_cores":quantity,"memory_bytes":quantity,"disk_bytes":quantity}));
    let allocated = object(
        json!({"disk_retained_seconds":quantity,"uptime_seconds":quantity,"cpu_core_hours":quantity,
        "memory_gib_hours":quantity,"disk_gib_hours":quantity,"complete":boolean,"projected_tail_seconds":integer,"unknown_seconds":integer}),
    );
    let measurement = json!({"state":{"type":"string","enum":["ok","partial","stale","unmounted","not_running","unavailable","unattributed"]},
        "observed_at":nullable(quantity.clone()),"source":text,"covered_seconds":quantity,"expected_seconds":quantity,
        "unknown_mapping_seconds":quantity,"coverage_ratio":nullable(quantity.clone()),"available_from":nullable(quantity.clone()),
        "last_observed":nullable(quantity.clone()),"percent_of_limit":nullable(quantity.clone()),
        "filesystem_capacity_bytes":nullable(quantity.clone()),"exceeds_capacity":boolean});
    let mut cpu = measurement.clone();
    let mut gauge = measurement;
    for key in [
        "latest_cores",
        "mean_cores",
        "max_cores",
        "observed_core_hours",
    ] {
        cpu[key] = nullable(quantity.clone());
    }
    for key in [
        "latest_bytes",
        "mean_bytes",
        "max_bytes",
        "observed_gib_hours",
    ] {
        gauge[key] = nullable(quantity.clone());
    }
    let measured = object(
        json!({"cpu":reference("UsageMetricCpu"),"memory":reference("UsageMetricGauge"),"disk":reference("UsageMetricGauge")}),
    );
    let mut owner = object(json!({"subject":text,"display_name":text}));
    owner["required"] = json!(["subject"]);
    let billing = object(
        json!({"basis":{"type":"string","const":"allocated"},"status":{"type":"string","const":"not_integrated"},"charges":{"type":"null"}}),
    );
    let infrastructure = object(
        json!({"state":text,"unproven_count":integer,"unassigned_pool":{"type":"array","items":nullable(quantity.clone()),"minItems":3,"maxItems":3},
        "unattributed":{"type":"array","items":nullable(quantity.clone()),"minItems":3,"maxItems":3},"observed_at":nullable(integer.clone())}),
    );
    let summary = object(
        json!({"sandbox_count":integer,"running_count":integer,"allocated_period":reference("UsageMetricAllocatedPeriod"),
        "total_allocated_capacity":reference("UsageMetricCapacity"),"covered_allocated_capacity":reference("UsageMetricCapacity"),
        "measured":nullable(reference("UsageMetricMeasured")),"excluded_count":integer,"infrastructure":nullable(reference("UsageMetricInfrastructure"))}),
    );
    let row = object(
        json!({"sandbox_id":text,"name":text,"owner":owner,"status":status,"runtime_version":nullable(text.clone()),
        "agent_count":integer,"last_activity_at":nullable(integer.clone()),"allocation":reference("UsageMetricCapacity"),
        "allocated_period":reference("UsageMetricAllocatedPeriod"),"measured":reference("UsageMetricMeasured"),"billing":billing,"shared_sandbox_usage":boolean}),
    );
    let point = object(
        json!({"from":integer,"to":integer,"mean":nullable(quantity.clone()),"min":nullable(quantity.clone()),"max":nullable(quantity.clone()),
        "covered_seconds":quantity,"expected_seconds":quantity,"unknown_mapping_seconds":quantity,"numerator":nullable(quantity.clone()),
        "denominator":nullable(quantity.clone()),"reason":nullable(text.clone()),"excluded_count":integer}),
    );
    let series = object(
        json!({"report_id":text,"metric":{"type":"string","enum":["cpu","memory","disk"]},
        "unit":{"type":"string","enum":["cores","bytes"]},"source":text,
        "lines":array(object(json!({"id":text,"label":text,"points":array(reference("UsageMetricPoint"))})))}),
    );
    let report = object(
        json!({"schema_version":{"type":"integer","const":1},"report_id":text,
        "status":{"type":"string","enum":["pending","ready","failed"]},"report_complete":boolean,
        "scope":object(json!({"principal":text,"admin":boolean})),"query":reference("UsageMetricCanonicalQuery"),
        "as_of":integer,"measurement_cutoff":integer,"effective_range":reference("UsageMetricRange"),"expires_at":integer,
        "allocation_watermark":text,"mapping_revision":integer,"available_from":nullable(integer.clone()),"tracking_started_at":nullable(integer.clone()),
        "warnings":array(text.clone()),"source_health":text,"summary":nullable(reference("UsageMetricSummary")),"poll_url":text,"retry_after_seconds":integer}),
    );
    json!({"UsageMetricQuantity":{"oneOf":[{"type":"number","minimum":0},{"type":"string","pattern":"^[1-9][0-9]{15,308}$"}],
            "description":"Finite observational quantity. Integers above 9007199254740991 are canonical decimal strings; preserve strings before presentation conversion."},
        "UsageMetricInteger":{"oneOf":[{"type":"integer","minimum":0,"maximum":9007199254740991_u64},
            {"type":"string","pattern":"^[1-9][0-9]{15,19}$"}],"description":"Unsigned 64-bit integer; values above JavaScript safe precision use decimal strings."},
        "UsageMetricQuery":input,"UsageMetricCanonicalQuery":canonical,"UsageMetricRange":range,
        "UsageMetricCapacity":capacity,"UsageMetricAllocatedPeriod":allocated,"UsageMetricCpu":object(cpu),"UsageMetricGauge":object(gauge),
        "UsageMetricMeasured":measured,"UsageMetricInfrastructure":infrastructure,"UsageMetricSummary":summary,"UsageMetricRow":row,
        "UsageMetricPoint":point,"UsageMetricSeries":series,"UsageMetricReport":report,
        "UsageFilterOptionsQuery":{"type":"object","additionalProperties":false,"required":["kind"],"properties":{
            "kind":{"type":"string","enum":["owner","sandbox","runtime"]},"search":{"type":"string","maxLength":128},
            "owner":{"type":"string","maxLength":255},"selected":{"type":"string","maxLength":255},
            "cursor":{"type":"string","maxLength":4096},"limit":{"type":"integer","minimum":1,"maximum":50,"default":50}}},
        "UsageFilterOptions":object(json!({"items":array(object(json!({"value":text,"label":text}))),"next":nullable(text.clone())})),
        "UsageMetricOwners":object(json!({"items":array(owner),"next":nullable(text.clone()),"report_id":text})),
        "UsageMetricPage":object(json!({"items":array(reference("UsageMetricRow")),"next":nullable(text.clone()),"report_id":text,"as_of":integer,"total_count":integer}))})
}
