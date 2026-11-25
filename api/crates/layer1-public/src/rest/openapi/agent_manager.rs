//! Focused projection of the existing public API, with one interaction model.
use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

const METHODS: &[&str] = &[
    "get", "post", "put", "patch", "delete", "head", "options", "trace",
];

/// Exact method selection prevents unrelated operations sharing a path from
/// leaking into this contract. The full compatibility document is unchanged.
const OPERATIONS: &[(&str, &[&str])] = &[
    ("/api/v1/usage/metrics/reports", &["post"]),
    ("/api/v1/usage/metrics/reports/{report}", &["get"]),
    ("/api/v1/usage/metrics/reports/{report}/sandboxes", &["get"]),
    (
        "/api/v1/usage/metrics/reports/{report}/sandboxes/{sandbox}",
        &["get"],
    ),
    ("/api/v1/usage/metrics/reports/{report}/series", &["get"]),
    ("/api/v1/admin/usage/metrics/reports", &["post"]),
    ("/api/v1/admin/usage/metrics/options", &["post"]),
    (
        "/api/v1/admin/usage/metrics/reports/{report}/owner-allocations",
        &["get"],
    ),
    ("/api/v1/admin/usage/metrics/reports/{report}", &["get"]),
    (
        "/api/v1/admin/usage/metrics/reports/{report}/sandboxes",
        &["get"],
    ),
    (
        "/api/v1/admin/usage/metrics/reports/{report}/sandboxes/{sandbox}",
        &["get"],
    ),
    (
        "/api/v1/admin/usage/metrics/reports/{report}/series",
        &["get"],
    ),
    (
        "/api/v1/admin/usage/metrics/reports/{report}/owners",
        &["get"],
    ),
    ("/api/v1/usage", &["get"]),
    ("/api/v1/usage/sandboxes", &["get"]),
    ("/api/v1/usage/sandboxes/{id}", &["get"]),
    ("/api/v1/usage/sandboxes/{id}/events", &["get"]),
    ("/api/v1/admin/usage", &["get"]),
    ("/api/v1/admin/usage/sandboxes", &["get"]),
    ("/api/v1/documents", &["get", "post"]),
    ("/api/v1/documents/{id}", &["get", "patch"]),
    ("/api/v1/documents/{id}/source", &["get"]),
    ("/api/v1/documents/{id}/revisions", &["get"]),
    ("/api/v1/documents/{id}/shares", &["get"]),
    ("/api/v1/documents/{id}/share-recipients", &["get"]),
    (
        "/api/v1/documents/{id}/shares/{subject}",
        &["put", "delete"],
    ),
    ("/api/v1/documents/{id}/share-links", &["get", "post"]),
    ("/api/v1/documents/{id}/share-links/{link_id}", &["delete"]),
    ("/api/v1/documents/{id}/comments", &["get", "post"]),
    ("/api/v1/documents/{id}/comments/{comment_id}", &["delete"]),
    ("/api/v1/agents", &["get", "post"]),
    ("/api/v1/desktop-capability", &["get"]),
    ("/api/v1/agents/{id}", &["get", "delete"]),
    ("/api/v1/agents/{id}/recover-worktree", &["post"]),
    ("/api/v1/agents/{id}/desktop", &["get"]),
    ("/api/v1/agents/{id}/desktop/ws", &["get"]),
    ("/api/v1/agent-lifecycle/{id}", &["get"]),
    ("/api/v1/agent-lifecycle/{id}/cancel", &["post"]),
    ("/api/v1/agents/{id}/transcript", &["get"]),
    ("/api/v1/agents/{id}/usage", &["get"]),
    ("/api/v1/agents/{id}/provider-associations", &["get"]),
    ("/api/v1/agents/{id}/models", &["get"]),
    ("/api/v1/agents/{id}/preferences", &["get", "patch"]),
    ("/api/v1/agents/{id}/settings", &["get", "patch"]),
    ("/api/v1/agents/{id}/message-feedback", &["get"]),
    ("/api/v1/agents/{id}/suggestions", &["get"]),
    (
        "/api/v1/agents/{id}/messages/{message_id}/feedback",
        &["get", "put"],
    ),
    (
        "/api/v1/agents/{id}/messages/{message_id}/feedback/reasons",
        &["put"],
    ),
    ("/api/v1/agents/{id}/attachments", &["post"]),
    ("/api/v1/agents/{id}/share-links", &["get", "post"]),
    ("/api/v1/agents/{id}/share-links/{share_id}", &["delete"]),
    ("/api/v1/agents/{id}/share-recipients", &["get"]),
    ("/api/v1/agents/{id}/original-request", &["get"]),
    ("/api/v1/agents/{id}/reply-preview", &["get"]),
    ("/api/v1/agent-share-links/{share_id}/redeem", &["post"]),
    ("/api/v1/agents/{id}/files", &["post", "delete"]),
    ("/api/v1/agents/{id}/files/tree", &["get"]),
    ("/api/v1/agents/{id}/files/content", &["get"]),
    ("/api/v1/agents/{id}/files/download", &["get"]),
    ("/api/v1/agents/{id}/artifacts", &["get"]),
    ("/api/v1/agents/{id}/deliverable-references", &["post"]),
    (
        "/api/v1/agents/{id}/answers/{answer_id}/deliverables",
        &["get"],
    ),
    (
        "/api/v1/agents/{id}/answers/{answer_id}/deliverables/{delivery_id}",
        &["get"],
    ),
    (
        "/api/v1/agents/{id}/answers/{answer_id}/deliverables/{delivery_id}/download",
        &["get"],
    ),
    ("/api/v1/agents/{id}/artifacts/{artifact_id}", &["get"]),
    (
        "/api/v1/agents/{id}/artifacts/{artifact_id}/download",
        &["get"],
    ),
    ("/api/v1/agents/{id}/service-access", &["put"]),
    ("/api/v1/agents/{id}/access-requests", &["get"]),
    (
        "/api/v1/agents/{id}/access-requests/{request_id}/approve",
        &["post"],
    ),
    (
        "/api/v1/agents/{id}/access-requests/{request_id}/deny",
        &["post"],
    ),
    ("/api/v1/sandboxes", &["get"]),
    ("/api/v1/sandboxes/{id}", &["get", "delete"]),
    ("/api/v1/sandboxes/{id}/pause", &["post"]),
    ("/api/v1/sandboxes/{id}/revive", &["post"]),
    ("/api/v1/sandboxes/{id}/runtime-update", &["post"]),
    ("/api/v1/sandboxes/{id}/events", &["get"]),
    ("/api/v1/sandboxes/{id}/workspaces", &["get"]),
    ("/api/v1/workspaces/{id}", &["get"]),
    ("/api/v2", &["get"]),
    ("/api/v2/agents/{agent_id}/responses", &["post"]),
    ("/api/v2/responses/{response_id}", &["get"]),
    ("/api/v2/responses/{response_id}/events", &["get"]),
    ("/api/v2/conversations/{conversation_id}", &["get"]),
    ("/api/v2/conversations/{conversation_id}/items", &["get"]),
    ("/api/v2/agents/{agent_id}/events/head", &["get"]),
    ("/api/v2/agents/{agent_id}/events/snapshot", &["get"]),
    ("/api/v2/agents/{agent_id}/events", &["get"]),
    (
        "/api/v2/agents/{agent_id}/responses/{response_id}/controls",
        &["get", "post"],
    ),
    (
        "/api/v2/agents/{agent_id}/responses/{response_id}/interrupt",
        &["post"],
    ),
    ("/api/v2/agents/{agent_id}/queued-inputs", &["get"]),
    (
        "/api/v2/agents/{agent_id}/responses/{response_id}/queued-input",
        &["post"],
    ),
    (
        "/api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation",
        &["get", "post"],
    ),
    ("/api/v2/commands/{command_id}", &["get"]),
];

/// Schemas, parameters, errors and operation identities come from the same
/// handler annotations as `/openapi.json`; this does not introduce new APIs.
pub fn document() -> Value {
    let full = serde_json::to_value(super::document()).expect("public OpenAPI serializes");
    project(full)
}

fn project(mut document: Value) -> Value {
    let paths = document["paths"].as_object_mut().expect("OpenAPI paths");
    paths.retain(|path, _| OPERATIONS.iter().any(|(selected, _)| path == selected));
    for (path, methods) in OPERATIONS {
        let item = paths
            .get_mut(*path)
            .and_then(Value::as_object_mut)
            .expect("selected Agent Manager path exists in the public API");
        for method in *methods {
            assert!(
                item.contains_key(*method),
                "selected method exists: {method} {path}"
            );
        }
        item.retain(|key, _| !METHODS.contains(&key.as_str()) || methods.contains(&key.as_str()));
    }
    document["info"]["title"] = json!("CMA Agent Manager API");
    document["info"]["description"] = json!(
        "Create an Agent, retain its agent_id, and use that same ID for lifecycle and Responses v2 interaction. \
         POST /api/v1/agents returns a lifecycle operation ID; read /api/v1/agent-lifecycle/{id} for agent_id. \
         Its workspace_id or sandbox_id options reuse an existing environment. Send messages through \
         /api/v2/agents/{agent_id}/responses with stream:true; conversation IDs in this projection are Agent IDs. \
         Response and command IDs identify individual turns and controls, not new Agents. \
         Authenticate with Authorization: Bearer <NyxID access token>, or the CMA same-origin browser session cookie. \
         Creating a new Sandbox also requires renewable owner delegation established by the existing CMA OAuth login. \
         Preserve the same Idempotency-Key and exact request on an uncertain retry. Observe accepted work until it settles. \
         This focused contract excludes profile authoring, administration and older interaction projections; \
         the full compatibility document remains available at /openapi.json."
    );
    document["servers"] = json!([{
        "url": "/", "description": "Current CMA origin; canonical operation paths remain /api/v1 and /api/v2"
    }]);
    prune_tags(&mut document);
    prune_components(&mut document);
    document
}

fn prune_tags(document: &mut Value) {
    let used: BTreeSet<String> = document["paths"]
        .as_object()
        .into_iter()
        .flat_map(|paths| paths.values())
        .filter_map(Value::as_object)
        .flat_map(|item| METHODS.iter().filter_map(move |method| item.get(*method)))
        .filter_map(|operation| operation["tags"].as_array())
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    let existing = document["tags"].as_array().cloned().unwrap_or_default();
    document["tags"] = Value::Array(used.into_iter().map(|name| {
        existing.iter().find(|tag| tag["name"] == name).cloned().unwrap_or_else(|| {
            let description = match name.as_str() {
                "responses v2" => "Durable Agent messages, streaming responses and conversation history",
                "response controls v2" => "Observed-turn cancellation, steering, approvals and independent queued-input edits",
                "agent events v2" => "Cursor-based Agent chronology snapshot and live events",
                _ => "Agent Manager operations",
            };
            json!({"name": name, "description": description})
        })
    }).collect());
}

type Component = (String, String);

fn reference_component(reference: &str) -> Option<Component> {
    let (kind, rest) = reference.strip_prefix("#/components/")?.split_once('/')?;
    let name = rest
        .split('/')
        .next()?
        .replace("~1", "/")
        .replace("~0", "~");
    Some((kind.to_owned(), name))
}

fn dependencies(value: &Value, found: &mut BTreeSet<Component>) {
    match value {
        Value::Array(values) => {
            for value in values {
                dependencies(value, found);
            }
        }
        Value::Object(object) => {
            if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                found.extend(reference_component(reference));
            }
            // Discriminator mappings may use references or bare schema names.
            if let Some(mapping) = object
                .get("discriminator")
                .and_then(|value| value.get("mapping"))
                .and_then(Value::as_object)
            {
                for target in mapping.values().filter_map(Value::as_str) {
                    if let Some(component) = reference_component(target) {
                        found.insert(component);
                    } else if !target.contains(['/', ':', '#']) {
                        found.insert(("schemas".into(), target.into()));
                    }
                }
            }
            if let Some(requirements) = object.get("security").and_then(Value::as_array) {
                for requirement in requirements.iter().filter_map(Value::as_object) {
                    found.extend(
                        requirement
                            .keys()
                            .map(|name| ("securitySchemes".into(), name.clone())),
                    );
                }
            }
            for value in object.values() {
                dependencies(value, found);
            }
        }
        _ => {}
    }
}

fn prune_components(document: &mut Value) {
    let components = document
        .as_object_mut()
        .expect("OpenAPI object")
        .remove("components")
        .unwrap_or_else(|| json!({}));
    let mut required = BTreeSet::new();
    dependencies(document, &mut required);
    let mut retained = BTreeSet::new();
    while let Some(component) = required
        .iter()
        .find(|key| !retained.contains(*key))
        .cloned()
    {
        let (kind, name) = &component;
        let value = components
            .get(kind)
            .and_then(|group| group.get(name))
            .expect("referenced OpenAPI component exists");
        dependencies(value, &mut required);
        retained.insert(component);
    }
    let mut result = Map::new();
    for (kind, name) in retained {
        result
            .entry(kind.clone())
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .expect("component group")
            .insert(name.clone(), components[&kind][&name].clone());
    }
    document["components"] = Value::Object(result);
}

#[cfg(test)]
#[path = "agent_manager_tests.rs"]
mod tests;
