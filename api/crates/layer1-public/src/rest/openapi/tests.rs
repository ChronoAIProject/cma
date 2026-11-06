use std::collections::{BTreeSet, HashSet};

use super::identity::operation_id;
use super::*;

#[test]
fn every_exported_operation_appears_in_the_rest_documentation_inventory() {
    let markdown = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../docs/API_REST.md"
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
    assert_eq!(admin.len(), 27);
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
    assert_eq!(operation_ids.len(), 30);
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
fn every_trigger_launch_and_connection_operation_is_documented() {
    let document = serde_json::to_value(document()).expect("OpenAPI serializes");
    let expected = [
        ("get", "/auth/signup-options"),
        ("get", "/api/v1/triggers"),
        ("post", "/api/v1/triggers"),
        ("get", "/api/v1/triggers/{id}"),
        ("put", "/api/v1/triggers/{id}"),
        ("delete", "/api/v1/triggers/{id}"),
        ("post", "/api/v1/triggers/{id}/publish"),
        ("post", "/api/v1/triggers/{id}/retire"),
        ("get", "/api/v1/triggers/{id}/revisions"),
        ("get", "/api/v1/triggers/{id}/stats"),
        ("get", "/api/v1/triggers/public/{public_id}"),
        ("get", "/api/v1/triggers/public/{public_id}/badge.svg"),
        ("get", "/api/v1/triggers/{id}/embed"),
        ("get", "/api/v1/triggers/{id}/placements"),
        ("post", "/api/v1/triggers/{id}/placements"),
        (
            "post",
            "/api/v1/triggers/{id}/placements/{placement_id}/retire",
        ),
        ("post", "/api/v1/triggers/public/{public_id}/launches"),
        ("get", "/api/v1/launches"),
        ("get", "/api/v1/launches/{id}"),
        ("get", "/api/v1/launches/{id}/events"),
        ("get", "/api/v1/launches/{id}/requirements"),
        ("post", "/api/v1/launches/{id}/start"),
        ("post", "/api/v1/launches/{id}/cancel"),
        ("post", "/api/v1/service-connections"),
        ("get", "/api/v1/service-connections/{handoff_id}"),
        ("get", "/api/v1/agui/launches/{id}/events"),
    ];

    for (method, path) in expected {
        assert!(
            !operation(&document, method, path).is_null(),
            "missing {method} {path}"
        );
    }
}

#[test]
fn only_public_discovery_and_previews_are_anonymous() {
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

#[test]
fn every_trigger_and_launch_wire_schema_is_registered() {
    let document = document();
    let schemas = &document.components.expect("components").schemas;
    for name in [
        "SignupOptions",
        "EmbedDescriptor",
        "EmbedSnippets",
        "TriggerButtonLabelValue",
        "TriggerThemeValue",
        "SandboxReusePolicyValue",
        "WorkspaceReusePolicyValue",
        "AgentReusePolicyValue",
        "TriggerPresentationBody",
        "TriggerTargetBody",
        "TriggerReuseBody",
        "TriggerConsentBody",
        "TriggerContentBody",
        "CreateTriggerRequest",
        "UpdateTriggerRequest",
        "PublishTriggerRequest",
        "CreatePlacementRequest",
        "CreateLaunchRequest",
        "StartLaunchRequest",
        "CreateServiceConnectionRequest",
        "TriggerAvailabilityValue",
        "TriggerStatsView",
        "PlacementView",
        "TriggerRevisionView",
        "PublishedTriggerRevisionView",
        "TriggerView",
        "TriggerPageView",
        "TriggerPlacementStatsView",
        "TriggerStatisticsView",
        "TriggerTrustDisclosureView",
        "TriggerTrustPreviewView",
        "TriggerSizePreviewView",
        "TriggerTechnicalPreviewView",
        "TriggerPreviewView",
        "ResolvedResourceView",
        "LaunchResolutionView",
        "ServiceRequirementView",
        "RequirementsSnapshotView",
        "ReviewNeedView",
        "ReviewRequirementsView",
        "LaunchTrustResourceValue",
        "LaunchTrustDisclosureView",
        "LaunchTrustManifestView",
        "LaunchConsentSurfaceValue",
        "LaunchConfirmationView",
        "LaunchView",
        "LaunchFirstMessageCommandView",
        "LaunchPageView",
        "LaunchEventPageView",
        "ServiceConnectionStateValue",
        "CreatedServiceConnectionView",
        "ServiceConnectionStatusView",
        "LaunchStateValue",
        "LaunchStepKeyValue",
        "LaunchStepStatusValue",
        "RepeatChoiceValue",
        "ServiceRequirementStateValue",
        "FirstMessageStateValue",
        "LaunchEffectKindValue",
        "LaunchStepView",
        "LaunchBlockedReasonView",
        "LaunchFailureView",
        "LaunchEventKindView",
        "LaunchEventView",
    ] {
        assert!(schemas.contains_key(name), "missing schema {name}");
    }
    let schemas = serde_json::to_value(schemas).expect("schemas serialize");
    let association = &schemas["LaunchFirstMessageCommandView"]["properties"];
    assert_eq!(association.as_object().unwrap().len(), 3);
    for field in ["agent_id", "command_id", "response_id"] {
        assert_eq!(association[field]["type"], "string");
    }
    assert!(!schemas["LaunchView"]["required"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("first_message_command")));
}

#[test]
fn inline_trigger_target_schema_exposes_image_agent_and_repository_contract() {
    let document = serde_json::to_value(document()).expect("OpenAPI serializes");
    let schemas = &document["components"]["schemas"];
    for name in [
        "TriggerImageBody",
        "TriggerRepositoryBody",
        "TriggerInlineWorkspaceBody",
        "TriggerInlineAgentBody",
        "TriggerInlineTargetBody",
        "TriggerSkillBody",
    ] {
        assert!(
            schemas[name].is_object(),
            "missing inline trigger schema {name}"
        );
    }

    assert_eq!(
        schema_ref(&schemas["TriggerTargetBody"]["properties"]["inline"]),
        "#/components/schemas/TriggerInlineTargetBody"
    );
    assert!(schemas["TriggerInlineTargetBody"]["required"]
        .as_array()
        .expect("inline target required fields")
        .contains(&serde_json::json!("agent")));
    assert_eq!(
        schemas["TriggerInlineAgentBody"]["properties"]["reasoning_effort"]["$ref"],
        "#/components/schemas/ThinkingEffortValue"
    );
    assert_eq!(
        schemas["TriggerInlineAgentBody"]["properties"]["permission_mode"]["$ref"],
        "#/components/schemas/PermissionLevelValue"
    );
    let repository_reference_type = &schemas["TriggerRepositoryBody"]["properties"]["ref"]["type"];
    assert!(
        repository_reference_type == "string"
            || repository_reference_type
                .as_array()
                .is_some_and(|types| types.contains(&serde_json::json!("string")))
    );
    assert_eq!(
        schema_ref(&schemas["TriggerInlineWorkspaceBody"]["properties"]["repository"]),
        "#/components/schemas/TriggerRepositoryBody"
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
