use serde_json::{json, Map, Value};
pub(super) fn extend(paths: &mut Map<String, Value>, schemas: &mut Value) {
    schemas["UsageMetricView"] = json!({"type":"object","additionalProperties":false,
        "required":["schema_version","window","requested_range","report","generated_at","stale","refresh"],
        "properties":{"schema_version":{"type":"integer","const":1},
        "window":{"type":"string","enum":["24h","7d","30d","custom"]},
        "requested_range":{"$ref":"#/components/schemas/UsageMetricRange"},
        "report":{"$ref":"#/components/schemas/UsageMetricReport"},
        "generated_at":{"oneOf":[{"$ref":"#/components/schemas/UsageMetricInteger"},{"type":"null"}]},
        "stale":{"type":"boolean"},"refresh":{"type":"object","additionalProperties":false,
        "required":["state","report_id","retry_after_seconds"],"properties":{
        "state":{"type":"string","enum":["idle","pending","deferred"]},
        "report_id":{"type":["string","null"]},"retry_after_seconds":{"$ref":"#/components/schemas/UsageMetricInteger"}}}}});
    for (prefix, name) in [
        ("/api/v1/usage/metrics", "owner"),
        ("/api/v1/admin/usage/metrics", "admin"),
    ] {
        let parameters: Vec<_> = [
            ("window",json!({"type":"string","enum":["24h","7d","30d","custom"],"default":"24h"})),
            ("from",json!({"type":"integer","minimum":0,"description":"Required only for custom window"})),
            ("to",json!({"type":"integer","minimum":0,"description":"Required only for custom window"})),
            ("step_seconds",json!({"type":"integer","minimum":60})),
            ("filters",json!({"type":"string","maxLength":4096,"description":"URL-encoded strict JSON filters object from UsageMetricQuery"})),
            ("refresh",json!({"type":"boolean","default":false})),
        ].into_iter().map(|(name,schema)|json!({"name":name,"in":"query","required":false,"schema":schema})).collect();
        let response = json!({"description":"Currently authorized committed snapshot; actual range and generation time remain explicit. Stale or refresh-deferred results remain usable.",
            "content":{"application/json":{"schema":{"$ref":"#/components/schemas/UsageMetricView"}}}});
        paths.insert(format!("{prefix}/view"),json!({"get":{"operationId":format!("{name}UsageMetricsLatest"),"tags":["usage-metrics"],
            "summary":"Return the latest committed report and enqueue a bounded refresh; private, no-store",
            "parameters":parameters,"responses":{"200":response,"202":{"description":"No safe retained snapshot; an initial replacement is admitted. Retry-After: 2.","content":{"application/json":{"schema":{"$ref":"#/components/schemas/UsageMetricView"}}}},
            "400":{"description":"Invalid typed selection"},"401":{"description":"Authentication required"},"403":{"description":"Current capability required"},
            "404":{"description":"Typed not_found means selected-resource authority failed; never compatibility fallback"},
            "429":{"description":"No retained snapshot and report admission is full; Retry-After"},
            "501":{"description":"Typed report_view_disabled permits old-endpoint compatibility fallback"},
            "503":{"description":"Integrity/dependency unavailable; never compatibility fallback"}}}}));
    }
}
