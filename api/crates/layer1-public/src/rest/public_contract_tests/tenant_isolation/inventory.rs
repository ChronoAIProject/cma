//! This inventory selects actual replayed contracts, not just OpenAPI assertions.
use super::super::{paths, Case, Corpus};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Entry {
    operation: String,
    positive: String,
    negative: String,
    proof: String,
}
fn entries() -> &'static [Entry] {
    static ENTRIES: OnceLock<Vec<Entry>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../../../../tests/fixtures/tenant-isolation-matrix.json"
        ))
        .unwrap()
    })
}
pub(super) fn known_not_found(code: &str) -> bool {
    static INVENTORY: OnceLock<serde_json::Value> = OnceLock::new();
    let inventory = INVENTORY.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../../../../tests/fixtures/public-contract-inventory.json"
        ))
        .unwrap()
    });
    // required_problems lists mandatory examples, not every legal refusal of
    // each route. Require a known public concealment code; the probe separately
    // requires exact status/body equivalence for foreign and absent resources.
    (code == "not_found" || code.ends_with("_not_found"))
        && inventory["operations"]
            .as_object()
            .unwrap()
            .values()
            .any(|operation| {
                operation["required_problems"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|expected| expected == code)
            })
}

pub(super) fn included(case: &Case) -> bool {
    entries()
        .iter()
        .any(|e| e.positive == case.id || e.negative == case.id)
}
fn in_scope(path: &str) -> bool {
    let path = path
        .strip_prefix("/api/v1/")
        .or_else(|| path.strip_prefix("/api/v2/"));
    let Some(path) = path else {
        return false;
    };
    if path.starts_with("cmaeg/bindings/") && path.contains("/conversations/") {
        return true;
    }
    let path = path.strip_prefix("admin/").unwrap_or(path);
    matches!(
        path.split('/').next().unwrap(),
        "agents"
            | "sandboxes"
            | "sandbox-creation-outcomes"
            | "sandbox-commands"
            | "commands"
            | "agent-delegations"
            | "agent-tools"
            | "workspaces"
            | "worktrees"
            | "documents"
            | "usage"
            | "resource-access"
            | "resource-shares"
            | "agent-share-links"
            | "responses"
            | "conversations"
            | "agui"
            | "agent-lifecycle"
            | "desktop-capability"
            | "runtime-inventory"
    ) && path != "agui/capabilities"
}

#[test]
fn every_scoped_operation_has_an_explicit_executable_isolation_contract() {
    let cases: BTreeMap<_, _> = paths()
        .into_iter()
        .flat_map(|p| {
            serde_json::from_slice::<Corpus>(&std::fs::read(p).unwrap())
                .unwrap()
                .cases
        })
        .map(|c| {
            c.response.expected_headers();
            (c.id.clone(), c)
        })
        .collect();
    let openapi = serde_json::to_value(super::super::super::openapi::document()).unwrap();
    let documented: BTreeSet<_> = openapi["paths"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(path, _)| in_scope(path))
        .flat_map(|(path, methods)| {
            methods
                .as_object()
                .unwrap()
                .keys()
                .filter(|m| ["get", "post", "put", "patch", "delete"].contains(&m.as_str()))
                .map(move |m| format!("{} {path}", m.to_ascii_uppercase()))
        })
        .collect();
    let declared: BTreeSet<_> = entries().iter().map(|e| e.operation.clone()).collect();
    assert_eq!(
        declared.len(),
        entries().len(),
        "duplicate isolation operation"
    );
    assert_eq!(
        documented, declared,
        "new resource operation requires isolation coverage"
    );
    for entry in entries() {
        let positive = &cases[&entry.positive];
        assert_eq!(positive.operation, entry.operation);
        if entry.proof == "probe" {
            assert!(
                super::selected(positive),
                "{} must actually be probed",
                entry.operation
            );
        }
        if entry.proof == "unsupported" {
            assert_eq!(positive.response.status, 422);
            continue;
        }
        assert!(
            (200..300).contains(&positive.response.status)
                || positive.response.status == 101
                || (entry.operation.ends_with("/terminal") && positive.response.status == 501)
        );
        let negative = &cases[&entry.negative];
        assert_eq!(negative.operation, entry.operation);
        assert!(
            [403, 404].contains(&negative.response.status),
            "{} must deny",
            entry.negative
        );
        assert!(matches!(
            entry.proof.as_str(),
            "probe"
                | "contract"
                | "receipt-contract"
                | "delegation"
                | "binding-contract"
                | "operator"
                | "filtered-inventory"
                | "mongo-report"
                | "mongo-service-access"
                | "mongo-lifecycle"
                | "context-creation"
                | "own-creation"
                | "profile-creation"
        ));
    }
    // History export is mounted but deliberately absent from OpenAPI. Its real
    // owner/shared/foreign and 1,000 controlled interleavings run separately.
    assert!(include_str!("../../agent_reads.rs")
        .contains(".route(\"/api/v1/agents/{id}/history\", get(history))"));
}

#[test]
fn mounted_resource_paths_cannot_hide_outside_the_openapi_inventory() {
    fn walk(directory: &std::path::Path, sources: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.to_string_lossy().contains("tests") {
                continue;
            }
            if path.is_dir() {
                walk(&path, sources);
            } else if path.extension().is_some_and(|e| e == "rs") {
                sources.push(path);
            }
        }
    }
    let known: BTreeSet<_> = entries()
        .iter()
        .map(|e| e.operation.split_once(' ').unwrap().1)
        .chain([
            "/api/v1/agents/{id}/history",
            "/api/v1/agent-tools",
            // The reviewed retirement executor uses a private operator contract.
            // Its routes are exercised by the authorization tests in admin/always_on_tests.rs.
            "/api/v1/admin/sandboxes/{id}/retire-reviewed",
            "/api/v1/admin/sandboxes/{id}/retire-reviewed/{batch}/{member}",
        ])
        .collect();
    let mut sources = Vec::new();
    walk(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut sources,
    );
    walk(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../cma/src/bot_setups"),
        &mut sources,
    );
    for source in sources {
        let text = std::fs::read_to_string(&source).unwrap();
        for mounted in text.split(".route(").skip(1) {
            let Some(literal) = mounted.trim_start().strip_prefix('"') else {
                continue;
            };
            let path = literal.split('"').next().unwrap();
            // Nested relative routes are covered by the full OpenAPI operation
            // comparison above. This finds absolute supplemental/alias mounts.
            if in_scope(path) {
                assert!(
                    known.contains(path),
                    "unclassified mount {path} in {}",
                    source.display()
                );
            }
        }
    }
}
