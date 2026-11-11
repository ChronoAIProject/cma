use std::sync::atomic::{AtomicUsize, Ordering};

use axum::{
    body::{to_bytes, Body},
    http::{Method, Request},
};
use layer2_core::sandbox::ledger_maintenance_plan::LedgerMaintenanceSnapshotV1;
use layer2_core::sandbox::store::ledger_maintenance::{
    LedgerMaintenanceInspection, LedgerMaintenanceStoreError,
};
use layer2_core::{Authorization, Caller, Capability, SubjectId};
use tower::ServiceExt;

use super::*;

const ID: &str = "sbx_ledger_assessment_fixture";
const PATH: &str = "/api/v1/sandboxes/sbx_ledger_assessment_fixture/ledger-maintenance";
const OWNER: &str = "ledger-fixture-owner";

struct Reader {
    calls: AtomicUsize,
    inspection: LedgerMaintenanceInspection,
    error: Option<LedgerMaintenanceStoreError>,
}

#[async_trait::async_trait]
impl LedgerMaintenanceReader for Reader {
    async fn inspect(
        &self,
        _: &Caller,
        sandbox: &SandboxId,
    ) -> Result<LedgerMaintenanceInspection, LedgerMaintenanceStoreError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(sandbox.as_str(), ID);
        self.error.map_or_else(|| Ok(self.inspection.clone()), Err)
    }
}

fn reader() -> Reader {
    let mut sandbox = crate::rest::agents_tests::sandbox(ID, OWNER);
    let mut home = layer2_core::sandbox::SandboxHome::reserve(
        &sandbox.id,
        layer3_adapter::HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "fixture-home".into(),
            capacity_gib: 8,
            template_name: "fixture-seed".into(),
            template_generation: "fixture-generation".into(),
        },
    )
    .unwrap();
    home.bind(layer3_adapter::HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "fixture-volume".into(),
        storage_generation: "fixture-volume-generation".into(),
    })
    .unwrap();
    sandbox.lifecycle.home = Some(Box::new(home));
    Reader {
        calls: AtomicUsize::new(0),
        error: None,
        inspection: LedgerMaintenanceInspection {
            source: LedgerMaintenanceSnapshotV1 {
                source_documents_sha256: "a".repeat(64),
                sandbox,
                sandbox_event_head: 0,
                agents: vec![],
                workspaces: vec![],
                worktrees: vec![],
                command_lanes: vec![],
            },
            state: None,
        },
    }
}

async fn call(
    reader: Arc<Reader>,
    method: Method,
    path: &str,
    subject: Option<&str>,
    capability: Option<Capability>,
    enabled: bool,
) -> (StatusCode, serde_json::Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .unwrap();
    if let Some(subject) = subject {
        request
            .extensions_mut()
            .insert(crate::AuthenticatedSubject(SubjectId(subject.into())));
    }
    if let Some(capability) = capability {
        request
            .extensions_mut()
            .insert(Authorization::new([capability]));
    }
    let response = ledger_maintenance_router(reader, LedgerMaintenanceAssessmentPolicy { enabled })
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 16384).await.unwrap();
    let body = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, body)
}

#[tokio::test]
async fn assessment_requires_authenticated_current_authority_and_defaults_off() {
    let reader = Arc::new(reader());
    for (subject, capability, enabled, expected) in [
        (
            None,
            Some(Capability::UseOwnSandboxes),
            true,
            StatusCode::UNAUTHORIZED,
        ),
        (Some(OWNER), None, true, StatusCode::NOT_FOUND),
        (
            Some(OWNER),
            Some(Capability::ReadAllSandboxes),
            true,
            StatusCode::NOT_FOUND,
        ),
        (
            Some(OWNER),
            Some(Capability::UseOwnSandboxes),
            false,
            StatusCode::NOT_FOUND,
        ),
    ] {
        let (status, _) = call(
            reader.clone(),
            Method::GET,
            PATH,
            subject,
            capability,
            enabled,
        )
        .await;
        assert_eq!(status, expected);
    }
    assert_eq!(reader.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn owner_and_operator_receive_only_counts_hashes_and_explicit_unknowns() {
    let reader = Arc::new(reader());
    let before = serde_json::to_vec(&reader.inspection.source).unwrap();
    for (subject, capability) in [
        (OWNER, Capability::UseOwnSandboxes),
        ("fixture-operator", Capability::ControlAnySandbox),
    ] {
        let (status, body) = call(
            reader.clone(),
            Method::GET,
            PATH,
            Some(subject),
            Some(capability),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["agents"], 0);
        assert_eq!(body["source_sha256"], "a".repeat(64));
        assert_eq!(body["maintenance_phase"], serde_json::Value::Null);
        assert_eq!(body["native_effects_unknown"], true);
        assert_eq!(body["physical_exclusion_unknown"], true);
        assert_eq!(body["recovery_authorized"], false);
        let text = body.to_string();
        for private in [ID, OWNER, "runtime-route", "fixture-volume", "fixture-seed"] {
            assert!(!text.contains(private));
        }
    }
    assert_eq!(reader.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        serde_json::to_vec(&reader.inspection.source).unwrap(),
        before
    );
}

#[tokio::test]
async fn foreign_resources_and_authority_failures_have_fixed_refusals() {
    let (status, body) = call(
        Arc::new(reader()),
        Method::GET,
        PATH,
        Some("foreign-fixture-owner"),
        Some(Capability::UseOwnSandboxes),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
    for error in [
        LedgerMaintenanceStoreError::Unavailable,
        LedgerMaintenanceStoreError::Corrupt,
        LedgerMaintenanceStoreError::PendingCommit,
    ] {
        let mut reader = reader();
        reader.error = Some(error);
        let (status, body) = call(
            Arc::new(reader),
            Method::GET,
            PATH,
            Some(OWNER),
            Some(Capability::UseOwnSandboxes),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(!body.to_string().contains(OWNER));
    }
}

#[tokio::test]
async fn invalid_paths_and_write_methods_never_reach_the_reader() {
    let reader = Arc::new(reader());
    let path = format!("/api/v1/sandboxes/{}/ledger-maintenance", "x".repeat(257));
    assert_eq!(
        call(
            reader.clone(),
            Method::GET,
            &path,
            Some(OWNER),
            Some(Capability::UseOwnSandboxes),
            true
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    for method in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
        assert_eq!(
            call(
                reader.clone(),
                method,
                PATH,
                Some(OWNER),
                Some(Capability::UseOwnSandboxes),
                true
            )
            .await
            .0,
            StatusCode::METHOD_NOT_ALLOWED
        );
    }
    assert_eq!(reader.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn document_exposes_only_the_read_operation_and_explicit_assessment_shape() {
    let document = serde_json::to_value(ledger_maintenance_document()).unwrap();
    let path = &document["paths"]["/api/v1/sandboxes/{id}/ledger-maintenance"];
    assert!(path.get("get").is_some());
    assert_eq!(path.as_object().unwrap().len(), 1);
    let schema =
        &document["components"]["schemas"]["LedgerMaintenanceAssessmentView"]["properties"];
    for field in [
        "source_sha256",
        "native_effects_unknown",
        "physical_exclusion_unknown",
        "recovery_authorized",
    ] {
        assert!(schema.get(field).is_some());
    }
}
