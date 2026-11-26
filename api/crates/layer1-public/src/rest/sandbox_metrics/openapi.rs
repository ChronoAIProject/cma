use super::schemas::schemas;
use serde_json::json;
pub(crate) fn document() -> utoipa::openapi::OpenApi {
    let mut paths = serde_json::Map::new();
    for (prefix, admin) in [
        ("/api/v1/usage/metrics", false),
        ("/api/v1/admin/usage/metrics", true),
    ] {
        for (suffix, method, name, schema) in [
            ("/options", "post", "options", "UsageFilterOptions"),
            (
                "/reports/{report}/owner-allocations",
                "get",
                "owner_allocations",
                "UsageOwnerAllocations",
            ),
            (
                "/reports/{report}/owners",
                "get",
                "owners",
                "UsageMetricOwners",
            ),
            ("/reports", "post", "create", "UsageMetricReport"),
            ("/reports/{report}", "get", "poll", "UsageMetricReport"),
            (
                "/reports/{report}/sandboxes",
                "get",
                "page",
                "UsageMetricPage",
            ),
            (
                "/reports/{report}/series",
                "get",
                "series",
                "UsageMetricSeries",
            ),
            (
                "/reports/{report}/sandboxes/{sandbox}",
                "get",
                "detail",
                "UsageMetricRow",
            ),
        ] {
            if matches!(name, "owners" | "options" | "owner_allocations") && !admin {
                continue;
            }
            let mut operation = json!({"operationId":format!("{}UsageMetrics{name}",if admin{"admin"}else{"owner"}),"tags":["usage-metrics"],
                "summary":"Authorized measured Sandbox usage; observational estimates, not billing quantities",
                "responses":{"200":{"description":"Ready result; private, no-store","content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{schema}")}}}},
                    "400":{"description":"Invalid typed query or cursor"},"401":{"description":"Authentication required"},"403":{"description":"Capability required"},"404":{"description":"Resource/report inaccessible"},
                    "410":{"description":"Report expired; refresh"},"429":{"description":"Reporting budget exhausted; Retry-After"},"503":{"description":"Source/computation unavailable; Retry-After"}}});
            let mut parameters = Vec::new();
            for id in ["report", "sandbox"] {
                if suffix.contains(&format!("{{{id}}}")) {
                    parameters.push(
                        json!({"name":id,"in":"path","required":true,"schema":{"type":"string"}}),
                    );
                }
            }
            if name == "page" {
                for (key, schema) in [
                    (
                        "limit",
                        json!({"type":"integer","minimum":1,"maximum":200,"default":50}),
                    ),
                    ("cursor", json!({"type":"string"})),
                    (
                        "sort",
                        json!({"type":"string","enum":["name","owner","status","last_activity_at","cpu_mean_cores","memory_mean_bytes","disk_latest_bytes","allocated_cpu_core_hours","allocated_memory_gib_hours","allocated_disk_gib_hours"]}),
                    ),
                    ("order", json!({"type":"string","enum":["asc","desc"]})),
                ] {
                    parameters
                        .push(json!({"name":key,"in":"query","required":false,"schema":schema}));
                }
            }
            if name == "owner_allocations" {
                parameters.extend([
                    json!({"name":"metric","in":"query","required":true,"schema":{"type":"string","enum":["cpu","memory","disk"]}}),
                    json!({"name":"limit","in":"query","required":false,"schema":{"type":"integer","minimum":1,"maximum":200,"default":50}}),
                    json!({"name":"cursor","in":"query","required":false,"schema":{"type":"string","minLength":1,"maxLength":4096}}),
                ]);
                operation["summary"] = json!("Paged owner allocation totals across the frozen administrator report; descending allocation then owner identity, reauthorized and budgeted on every read");
            }
            if name == "owners" {
                parameters.extend([
                    json!({"name":"search","in":"query","required":false,"schema":{"type":"string","maxLength":128}}),
                    json!({"name":"limit","in":"query","required":false,"schema":{"type":"integer","minimum":1,"maximum":200,"default":50}}),
                    json!({"name":"cursor","in":"query","required":false,"schema":{"type":"string"}}),
                ]);
            }
            if name == "options" {
                operation["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"$ref":"#/components/schemas/UsageFilterOptionsQuery"}}}});
                operation["summary"] = json!("Read administrator filter choices independently of reports; names stay out of request URLs");
            }
            if name == "series" {
                for (key, schema, required) in [
                    (
                        "metric",
                        json!({"type":"string","enum":["cpu","memory","disk"]}),
                        true,
                    ),
                    (
                        "group_by",
                        json!({"type":"string","enum":["total","owner","sandbox"]}),
                        false,
                    ),
                    ("series_id", json!({"type":"string"}), false),
                    (
                        "top",
                        json!({"type":"integer","minimum":1,"maximum":20,"default":10}),
                        false,
                    ),
                ] {
                    parameters
                        .push(json!({"name":key,"in":"query","required":required,"schema":schema}));
                }
            }
            operation["parameters"] = parameters.into();
            if name == "create" {
                operation["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"$ref":"#/components/schemas/UsageMetricQuery"}}}});
                operation["responses"]["202"] = json!({"description":"Bounded report job pending; poll_url and Retry-After","content":{"application/json":{"schema":{"$ref":"#/components/schemas/UsageMetricReport"}}}});
            }
            paths.insert(format!("{prefix}{suffix}"), json!({method:operation}));
        }
    }
    let mut schemas = schemas();
    super::view_openapi::extend(&mut paths, &mut schemas);
    super::initial_openapi::extend(&mut paths, &mut schemas);
    serde_json::from_value(json!({"openapi":"3.1.0","info":{"title":"Measured Sandbox usage","version":"1"},"paths":paths,"components":{"schemas":schemas}})).expect("static usage OpenAPI")
}
