use std::collections::{BTreeSet, HashSet};

use super::identity::operation_id;
use super::*;

#[test]
fn usage_and_agent_pagination_document_runtime_limits_and_problem_responses() {
    let document = serde_json::to_value(document()).unwrap();
    for (path, default, query_names) in [
        (
            "/api/v1/agents",
            200,
            vec![
                "organization",
                "sandbox",
                "workspace",
                "status",
                "cursor",
                "limit",
            ],
        ),
        (
            "/api/v1/usage/sandboxes",
            50,
            vec!["owner", "cursor", "limit"],
        ),
        (
            "/api/v1/admin/usage/sandboxes",
            50,
            vec!["owner", "cursor", "limit"],
        ),
        (
            "/api/v1/usage/sandboxes/{id}/events",
            50,
            vec!["owner", "cursor", "limit"],
        ),
    ] {
        let operation = operation(&document, "get", path);
        let parameters = operation["parameters"].as_array().unwrap();
        let mut expected: BTreeSet<_> = query_names
            .into_iter()
            .map(|name| (name, "query", false))
            .collect();
        if path.contains("{id}") {
            expected.insert(("id", "path", true));
        }
        assert_eq!(parameters.len(), expected.len(), "{path}");
        assert_eq!(
            parameters
                .iter()
                .map(|param| (
                    param["name"].as_str().unwrap(),
                    param["in"].as_str().unwrap(),
                    param["required"].as_bool().unwrap(),
                ))
                .collect::<BTreeSet<_>>(),
            expected,
            "{path}"
        );
        let schema = &parameters
            .iter()
            .find(|param| param["name"] == "limit")
            .unwrap()["schema"];
        assert_eq!(schema["minimum"].as_f64(), Some(1.0), "{path}");
        assert_eq!(schema["maximum"].as_f64(), Some(200.0), "{path}");
        assert_eq!(schema["default"], default, "{path}");
        assert_eq!(
            operation["responses"]["400"]["content"]["application/problem+json"]["schema"]["$ref"],
            "#/components/schemas/ProblemDocument",
            "{path}"
        );
    }
    let summary = operation(&document, "get", "/api/v1/admin/usage");
    assert!(summary["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .all(|param| param["in"] == "query"));
    assert_eq!(
        summary["responses"]["400"]["content"]["application/problem+json"]["schema"]["$ref"],
        "#/components/schemas/ProblemDocument"
    );
}

#[test]
fn event_worker_list_documents_optional_query_parameters() {
    let document = serde_json::to_value(document()).expect("OpenAPI serializes");
    let parameters = operation(&document, "get", "/api/v1/event-worker-profiles")["parameters"]
        .as_array()
        .expect("list parameters");
    assert_eq!(parameters.len(), 2);
    assert_eq!(
        parameters
            .iter()
            .map(|param| (
                param["name"].as_str().unwrap(),
                param["in"].as_str().unwrap(),
                param["required"].as_bool().unwrap(),
            ))
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([("after", "query", false), ("limit", "query", false)])
    );
    let limit = parameters
        .iter()
        .find(|param| param["name"] == "limit")
        .unwrap();
    assert_eq!(limit["schema"]["minimum"].as_f64(), Some(1.0));
    assert_eq!(limit["schema"]["maximum"].as_f64(), Some(200.0));
    assert_eq!(limit["schema"]["default"], 50);
}

#[test]
fn event_worker_retire_documents_required_revision_query_and_id_path() {
    let document = serde_json::to_value(document()).expect("OpenAPI serializes");
    let parameters = operation(&document, "delete", "/api/v1/event-worker-profiles/{id}")
        ["parameters"]
        .as_array()
        .expect("retire parameters");
    assert_eq!(parameters.len(), 2);
    assert_eq!(
        parameters
            .iter()
            .map(|param| (
                param["name"].as_str().unwrap(),
                param["in"].as_str().unwrap(),
                param["required"].as_bool().unwrap(),
            ))
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([("expected_revision", "query", true), ("id", "path", true)])
    );
}

#[test]
fn every_exported_operation_appears_in_the_rest_documentation_inventory() {
    let markdown = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/openapi-operation-inventory.md"
    ));
    let inventory = markdown
        .split_once("<!-- openapi-operations:start -->")
        .and_then(|(_, rest)| rest.split_once("<!-- openapi-operations:end -->"))
        .map(|(inventory, _)| inventory)
        .expect("REST documentation has a checked operation inventory");
    let document = serde_json::to_value(document()).expect("OpenAPI serializes");
    let paths = document["paths"].as_object().expect("OpenAPI paths object");
    assert!(!paths.is_empty());
    for (path, item) in paths {
        for method in [
            "get", "post", "put", "patch", "delete", "head", "options", "trace",
        ] {
            if item.get(method).is_some() {
                let row = format!("| `{} {path}` |", method.to_ascii_uppercase());
                assert!(inventory.contains(&row), "undocumented operation {row}");
            }
        }
    }
}

#[test]
fn every_profile_and_marketplace_route_is_documented_and_authenticated() {
    let document = document();
    let mut operation_ids = HashSet::new();
    let profiles: Vec<_> = document
        .paths
        .paths
        .iter()
        .filter(|(path, _)| {
            path.starts_with("/api/v1/agent-profiles")
                || path.starts_with("/api/v1/workspace-profiles")
                || path.starts_with("/api/v1/sandbox-profiles")
                || path.starts_with("/api/v1/marketplace")
        })
        .collect();
    assert_eq!(profiles.len(), 21);
    for (_, item) in profiles {
        for operation in [
            item.get.as_ref(),
            item.put.as_ref(),
            item.post.as_ref(),
            item.delete.as_ref(),
            item.patch.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            assert!(operation_ids.insert(operation.operation_id.as_deref().unwrap()));
            assert_eq!(operation.security.as_ref().map(Vec::len), Some(2));
        }
    }
    for path in [
        "/api/v1/agent-profiles",
        "/api/v1/workspace-profiles/{id}/revisions/{revision}",
        "/api/v1/sandbox-profiles/{id}/visibility",
        "/api/v1/marketplace/{kind}/{id}/fork",
    ] {
        assert!(document.paths.paths.contains_key(path), "missing {path}");
    }
    assert_eq!(
        operation_id("post", "/api/v1/marketplace/{kind}/{id}/fork"),
        "post_api_v1_marketplace_by_kind_by_id_fork"
    );
}

#[test]
fn profile_authoring_documents_read_only_queries_and_exact_response_shapes() {
    let document = serde_json::to_value(document()).expect("OpenAPI serializes");
    for (method, suffix, schema, statuses) in [
        (
            "get",
            "bounds",
            "ProfileCapacityBoundsView",
            vec!["401", "403"],
        ),
        (
            "get",
            "services",
            "AuthoringServiceView",
            vec!["401", "403", "503"],
        ),
        (
            "get",
            "skills",
            "AuthoringSkillPage",
            vec!["400", "401", "403", "503"],
        ),
        (
            "post",
            "repositories/resolve",
            "RepositoryResolutionView",
            vec!["400", "401", "403", "404", "413", "415", "422", "503"],
        ),
    ] {
        let path = format!("/api/v1/profile-authoring/{suffix}");
        let operation = operation(&document, method, &path);
        assert_eq!(operation["security"].as_array().unwrap().len(), 2);
        let success = &operation["responses"]["200"]["content"]["application/json"]["schema"];
        let success = if suffix == "services" {
            &success["items"]
        } else {
            success
        };
        assert_eq!(success["$ref"], format!("#/components/schemas/{schema}"));
        for status in statuses {
            assert_eq!(
                operation["responses"][status]["content"]["application/problem+json"]["schema"]
                    ["$ref"],
                "#/components/schemas/ProblemDocument"
            );
            assert!(!operation["responses"][status]["description"]
                .as_str()
                .unwrap()
                .is_empty());
        }
        assert!(!operation["parameters"]
            .as_array()
            .is_some_and(|parameters| parameters
                .iter()
                .any(|parameter| parameter["name"] == "Idempotency-Key")));
    }
    let skills = operation(&document, "get", "/api/v1/profile-authoring/skills");
    let parameters = skills["parameters"].as_array().unwrap();
    assert_eq!(parameters.len(), 3);
    let limit = parameters
        .iter()
        .find(|parameter| parameter["name"] == "limit")
        .unwrap();
    assert_eq!(limit["schema"]["default"], 20);
    assert_eq!(limit["schema"]["minimum"], 1);
    assert_eq!(limit["schema"]["maximum"], 50);
    for (schema, optional) in [
        ("AuthoringServiceView", "description"),
        ("AuthoringSkillView", "description"),
        ("AuthoringSkillPage", "next"),
        ("RepositoryResolutionView", "reference"),
    ] {
        let schema = &document["components"]["schemas"][schema];
        assert!(!schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field == optional));
        assert_eq!(schema["properties"][optional]["type"], "string");
    }
}

#[test]
fn every_admin_route_is_documented_and_authenticated() {
    let document = document();
    let mut operation_ids = HashSet::new();
    let admin: Vec<_> = document
        .paths
        .paths
        .iter()
        .filter(|(path, _)| path.starts_with("/api/v1/admin/"))
        .collect();
    assert_eq!(admin.len(), 50);
    for (_, item) in admin {
        for operation in [
            item.get.as_ref(),
            item.put.as_ref(),
            item.post.as_ref(),
            item.patch.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            assert!(operation_ids.insert(operation.operation_id.as_deref().unwrap()));
            assert_eq!(operation.security.as_ref().map(Vec::len), Some(2));
        }
    }
    assert_eq!(operation_ids.len(), 53);
    assert!(document.paths.paths["/api/v1/admin/admission/ceilings"]
        .put
        .is_some());
    assert!(
        document.paths.paths["/api/v1/admin/sandboxes/{id}/runtime-migration"]
            .post
            .is_some()
    );
    assert!(
        document.paths.paths["/api/v1/admin/sandboxes/{id}/runtime-update/activity"]
            .get
            .is_some()
    );
    let view = document.paths.paths["/api/v1/admin/usage/metrics/view"]
        .get
        .as_ref()
        .expect("retained admin usage view is documented");
    assert_eq!(
        serde_json::to_value(&view.security).unwrap(),
        serde_json::json!([{"bearerAuth": []}, {"sessionCookie": []}])
    );
    assert!(document.paths.paths["/api/v1/admin/admission"]
        .get
        .is_some());
    for path in [
        "/api/v1/admin/admission/stop",
        "/api/v1/admin/admission/resume",
        "/api/v1/admin/admission/pending-homes/reconcile",
    ] {
        assert!(document.paths.paths[path].post.is_some());
    }
    assert!(document
        .paths
        .paths
        .contains_key("/api/v1/admin/profiles/{kind}/{id}/retire"));
    assert!(document
        .paths
        .paths
        .contains_key("/api/v1/admin/runtime-inventory"));
    let pools = &document.paths.paths["/api/v1/admin/warm-pools"];
    assert!(pools.get.is_some() && pools.post.is_some());
    assert!(document.paths.paths["/api/v1/admin/warm-pools/{id}"]
        .put
        .is_some());
}

#[test]
fn every_owner_hierarchy_route_is_documented_and_authenticated() {
    let document = document();
    let hierarchy_paths = [
        "/api/v1/sandboxes",
        "/api/v1/sandboxes/{id}",
        "/api/v1/sandboxes/{id}/pause",
        "/api/v1/sandboxes/{id}/revive",
        "/api/v1/sandboxes/{id}/always-on",
        "/api/v1/sandboxes/{id}/events",
        "/api/v1/sandboxes/{id}/workspaces",
        "/api/v1/workspaces/{id}",
        "/api/v1/workspaces/{id}/worktrees",
        "/api/v1/worktrees/{id}",
    ];
    let paths: Vec<_> = hierarchy_paths
        .iter()
        .map(|path| {
            document
                .paths
                .paths
                .get(*path)
                .unwrap_or_else(|| panic!("missing hierarchy path {path}"))
        })
        .collect();
    let mut operations = Vec::new();
    for item in paths {
        operations.extend(
            [item.get.as_ref(), item.post.as_ref(), item.delete.as_ref()]
                .into_iter()
                .flatten(),
        );
    }
    assert_eq!(operations.len(), 14);
    assert!(operations
        .iter()
        .all(|operation| operation.security.as_ref().map(Vec::len) == Some(2)));

    let document = serde_json::to_value(document).expect("OpenAPI serializes");
    for (path, operation_id, conflict) in [
        (
            "/api/v1/sandboxes/{id}/pause",
            "post_api_v1_sandboxes_by_id_pause",
            "sandbox_not_active",
        ),
        (
            "/api/v1/sandboxes/{id}/revive",
            "post_api_v1_sandboxes_by_id_revive",
            "sandbox_not_paused",
        ),
    ] {
        let operation = operation(&document, "post", path);
        assert_eq!(operation["operationId"], operation_id);
        assert!(operation.get("requestBody").is_none());
        assert!(operation["parameters"]
            .as_array()
            .is_some_and(|parameters| {
                parameters.iter().any(|parameter| {
                    parameter["name"] == "Idempotency-Key"
                        && parameter["in"] == "header"
                        && parameter["required"] == true
                })
            }));
        assert_eq!(
            operation["responses"]["202"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/SandboxView"
        );
        assert_eq!(
            operation["responses"]["401"]["description"],
            "not_authenticated"
        );
        for status in ["400", "401", "403", "404", "409", "422", "503"] {
            assert_eq!(
                operation["responses"][status]["content"]["application/problem+json"]["schema"]
                    ["$ref"],
                "#/components/schemas/ProblemDocument",
                "problem schema for {status} on {path}"
            );
        }
        assert!(operation["responses"]["409"]["description"]
            .as_str()
            .is_some_and(|description| description.contains(conflict)));
    }
}

#[test]
fn only_public_discovery_previews_and_bounded_diagnostics_are_anonymous() {
    let document = serde_json::to_value(document()).expect("OpenAPI serializes");
    let authenticated = serde_json::json!([
        {"bearerAuth": []},
        {"sessionCookie": []},
    ]);
    let mut anonymous = BTreeSet::new();

    for (path, item) in document["paths"].as_object().expect("paths object") {
        for method in ["get", "put", "post", "delete", "patch"] {
            let Some(operation) = item.get(method) else {
                continue;
            };
            let security = &operation["security"];
            if security.as_array().is_some_and(Vec::is_empty) {
                anonymous.insert(format!("{method} {path}"));
            } else if path == crate::rest::worker_credentials::WORKER_CLASSIFIER_PATH {
                assert_eq!(method, "post");
                assert_eq!(security, &serde_json::json!([{ "bearerAuth": [] }]));
            } else {
                assert_eq!(security, &authenticated, "security for {method} {path}");
            }
        }
    }

    assert_eq!(
        anonymous,
        BTreeSet::from([
            "get /api/v1/agui/capabilities".to_owned(),
            "get /api/v1/conformance".to_owned(),
            "get /api/v1/triggers/public/{public_id}".to_owned(),
            "get /api/v1/triggers/public/{public_id}/badge.svg".to_owned(),
            "get /api/v1/triggers/public/{public_id}/open/{placement_id}".to_owned(),
            "post /api/v1/triggers/public/{public_id}/entry-intent".to_owned(),
            "get /auth/signup-options".to_owned(),
            "post /api/v1/telemetry/browser".to_owned(),
        ])
    );
    assert_eq!(
        operation(
            &document,
            "post",
            "/api/v1/triggers/public/{public_id}/launches",
        )["security"],
        authenticated
    );
}

fn schema_ref(value: &serde_json::Value) -> &str {
    value["$ref"]
        .as_str()
        .or_else(|| {
            value["oneOf"]
                .as_array()
                .and_then(|variants| variants.iter().find_map(|variant| variant["$ref"].as_str()))
        })
        .expect("schema contains a component reference")
}

#[test]
fn manual_projections_preserve_signup_shape_and_stream_media_types() {
    let document = serde_json::to_value(document()).expect("OpenAPI serializes");
    let signup = &document["components"]["schemas"]["SignupOptions"];
    let fields: BTreeSet<_> = signup["properties"]
        .as_object()
        .expect("signup properties")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        fields,
        BTreeSet::from([
            "email_signup",
            "invite_code_required",
            "register_available",
            "social_providers",
        ])
    );
    assert!(
        operation(&document, "get", "/auth/signup-options")["responses"]["200"]["content"]
            ["application/json"]
            .is_object()
    );
    assert!(operation(
        &document,
        "get",
        "/api/v1/triggers/public/{public_id}/badge.svg",
    )["responses"]["200"]["content"]["image/svg+xml"]
        .is_object());
    assert!(
        operation(&document, "get", "/api/v1/agui/launches/{id}/events",)["responses"]["200"]
            ["content"]["text/event-stream"]
            .is_object()
    );
}

fn operation<'a>(
    document: &'a serde_json::Value,
    method: &str,
    path: &str,
) -> &'a serde_json::Value {
    &document["paths"][path][method]
}

#[test]
fn usage_presentation_settings_have_a_strict_boolean_admin_contract() {
    let document = serde_json::to_value(document()).unwrap();
    let schemas = &document["components"]["schemas"];
    let section = &schemas["UsageReportingSettingsBody"];
    assert_eq!(section["properties"]["measured_visible"]["type"], "boolean");
    assert_eq!(section["required"], serde_json::json!(["measured_visible"]));
    assert_eq!(section["additionalProperties"], false);
    for name in ["PlatformSettingsBody", "PatchRequest"] {
        assert_eq!(
            schemas[name]["properties"]["usage_reporting"]["$ref"],
            "#/components/schemas/UsageReportingSettingsBody"
        );
    }
}

#[path = "tests/trigger_routes.rs"]
mod trigger_routes;

#[path = "tests/trigger_schemas.rs"]
mod trigger_schemas;
