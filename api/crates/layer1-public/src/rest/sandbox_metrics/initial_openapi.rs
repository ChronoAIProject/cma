use serde_json::{json, Map, Value};

pub(super) fn extend(paths: &mut Map<String, Value>, schemas: &mut Value) {
    schemas["UsageInitialProjection"] = json!({"type":"object","additionalProperties":false,
        "properties":{
            "first_page":{"type":"object","additionalProperties":false,"properties":{
                "limit":{"type":"integer","minimum":1,"maximum":50,"default":50},
                "sort":{"type":"string","enum":["name","owner","status","last_activity_at","cpu_mean_cores","memory_mean_bytes","disk_latest_bytes","allocated_cpu_core_hours","allocated_memory_gib_hours","allocated_disk_gib_hours"],"default":"last_activity_at","description":"owner requires administrator authority"},
                "order":{"type":"string","enum":["asc","desc"],"default":"desc"}}},
            "total_metrics":{"type":"array","maxItems":3,"uniqueItems":true,"default":[],"items":{"type":"string","enum":["cpu","memory","disk"]}}}});
    schemas["UsageInitialPage"] = schemas["UsageMetricPage"].clone();
    schemas["UsageInitialPage"]["properties"]["items"]["maxItems"] = json!(50);
    schemas["UsageInitialSeries"] = schemas["UsageMetricSeries"].clone();
    schemas["UsageInitialSeries"]["properties"]["lines"]["maxItems"] = json!(1);
    schemas["UsageInitialSeries"]["properties"]["lines"]["items"]["properties"]["id"]["const"] =
        json!("total");
    schemas["UsageInitialSeries"]["properties"]["lines"]["items"]["properties"]["points"]
        ["maxItems"] = json!(2016);
    schemas["UsageInitialData"] = json!({"type":"object","additionalProperties":false,"required":["projection","rows","charts"],"properties":{
        "projection":{"$ref":"#/components/schemas/UsageInitialProjection"},
        "rows":{"$ref":"#/components/schemas/UsageInitialPage"},
        "charts":{"type":"array","maxItems":3,"items":{"$ref":"#/components/schemas/UsageInitialSeries"}}}});
    schemas["UsageInitialCreate"] = json!({"type":"object","additionalProperties":false,"required":["query","projection"],"properties":{
        "query":{"$ref":"#/components/schemas/UsageMetricQuery"},"projection":{"$ref":"#/components/schemas/UsageInitialProjection"}}});
    schemas["UsageInitialViewInput"] = json!({"type":"object","additionalProperties":false,"required":["selection","projection"],"properties":{
        "selection":{"type":"object","additionalProperties":false,"properties":{
            "window":{"type":"string","enum":["24h","7d","30d","custom"],"default":"24h"},
            "from":{"type":["integer","null"],"minimum":0},"to":{"type":["integer","null"],"minimum":0},
            "step_seconds":{"type":["integer","null"],"minimum":60},
            "filters":schemas["UsageMetricQuery"]["properties"]["filters"],"refresh":{"type":"boolean","default":false}}},
        "projection":{"$ref":"#/components/schemas/UsageInitialProjection"}}});
    for (name, field, descriptor) in [
        ("UsageInitialReport", "report", "UsageMetricReport"),
        ("UsageInitialView", "view", "UsageMetricView"),
    ] {
        for pending in [false, true] {
            let mut properties =
                json!({field:{"$ref":format!("#/components/schemas/{descriptor}")}});
            let report_state = json!({"type":"object","properties":{
                "status":{"type":"string","const":if pending {"pending"} else {"ready"}},
                "report_complete":{"type":"boolean","const":!pending},
                "summary":if pending {json!({"type":"null"})} else {json!({"type":"object"})}
            }});
            let state = if field == "view" {
                json!({"type":"object","properties":{"report":report_state}})
            } else {
                report_state
            };
            properties[field] =
                json!({"allOf":[{"$ref":format!("#/components/schemas/{descriptor}")},state]});
            let mut required = vec![field];
            if !pending {
                properties["initial"] = json!({"$ref":"#/components/schemas/UsageInitialData"});
                required.push("initial");
            }
            schemas[format!("{name}{}", if pending { "Pending" } else { "Ready" })] = json!({"type":"object","additionalProperties":false,"required":required,"properties":properties,
                "description":if pending {"Pending descriptor only; preserve its report ID for later reads."} else {"Complete requested initial data bound to this exact Ready descriptor."}});
        }
    }
    for (prefix, principal) in [
        ("/api/v1/usage/metrics", "owner"),
        ("/api/v1/admin/usage/metrics", "admin"),
    ] {
        for (suffix, name, input, output) in [
            (
                "/initial/reports",
                "CreateInitial",
                "UsageInitialCreate",
                "UsageInitialReport",
            ),
            (
                "/initial/view",
                "LatestInitial",
                "UsageInitialViewInput",
                "UsageInitialView",
            ),
            (
                "/initial/reports/{report}",
                "ReadInitial",
                "UsageInitialProjection",
                "UsageInitialReport",
            ),
        ] {
            let mut operation = json!({"operationId":format!("{principal}UsageMetrics{name}"),"tags":["usage-metrics"],
                "summary":"One currently authorized report with a bounded first page and total charts",
                "description":"Strict JSON body, maximum 16384 bytes. Complete response at most 8388608 bytes within five seconds, private/no-store. No arbitrary child IDs, cursors or grouped charts. Existing-ID reads never admit another report. Missing routes or typed report_view_disabled alone permit compatibility fallback; typed denial, missing report, malformed or partial results do not.",
                "requestBody":{"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{input}")}}}},
                "responses":{
                    "200":response(&format!("{output}Ready"), "Whole requested initial view; no partial success"),
                    "202":response(&format!("{output}Pending"), "Descriptor only; Retry-After: 2. Preserve the admitted report ID."),
                    "400":{"description":"Invalid typed query or projection; no fanout"},
                    "401":{"description":"Authentication required"},"403":{"description":"Current capability required"},
                    "404":{"description":"Typed not_found means report/resource inaccessible; no compatibility fallback"},
                    "410":{"description":"Report expired"},"429":{"description":"Reporting budget exhausted; Retry-After: 2"},
                    "503":{"description":"Unavailable, incomplete, mismatched, expired during read, over byte limit or over deadline; no partial response or compatibility fallback. Retry-After: 2"}}});
            if name == "LatestInitial" {
                operation["responses"]["501"] = json!({"description":"Typed report_view_disabled permits independent legacy reads"});
            }
            if name == "ReadInitial" {
                operation["parameters"] = json!([{"name":"report","in":"path","required":true,"schema":{"type":"string","pattern":"^umr_[a-f0-9]{32}$"}}]);
            }
            paths.insert(format!("{prefix}{suffix}"), json!({"post":operation}));
        }
    }
}
fn response(schema: &str, description: &str) -> Value {
    json!({"description":description,"headers":{"Cache-Control":{"schema":{"type":"string","const":"private, no-store"}}},
        "content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{schema}")}}}})
}
