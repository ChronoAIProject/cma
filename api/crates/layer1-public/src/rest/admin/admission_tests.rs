use super::*;
use layer2_core::admission::{AdmissionControlService, MemoryAdmissionControlStore};

const CONTROL: &str = "/api/v1/admin/admission";
const STOP: &str = "/api/v1/admin/admission/stop";
const RESUME: &str = "/api/v1/admin/admission/resume";

pub(crate) fn admission_fixture(writes: bool) -> (Router, Arc<MemoryAdmissionControlStore>) {
    admission_fixture_with_ceilings(writes, false)
}

pub(crate) fn admission_fixture_with_ceilings(
    writes: bool,
    ceilings_writes: bool,
) -> (Router, Arc<MemoryAdmissionControlStore>) {
    admission_fixture_with_ceilings_store(
        writes,
        ceilings_writes,
        Arc::new(layer2_core::admission::MemoryAdmissionCeilingsStore::default()),
    )
}

pub(crate) fn admission_fixture_with_ceilings_store(
    writes: bool,
    ceilings_writes: bool,
    ceilings_store: Arc<dyn layer2_core::admission::AdmissionCeilingsStore>,
) -> (Router, Arc<MemoryAdmissionControlStore>) {
    let activity = ActivityService::new(Arc::new(MemoryActivity::default()));
    let settings = Arc::new(PlatformSettingsService::new(
        Arc::new(MemorySettings::default()),
        activity.clone(),
    ));
    let sandboxes = Arc::new(InMemorySandboxStore::default());
    let profiles = Arc::new(InMemoryProfileStore::default());
    let retirement = Arc::new(StoredSandboxRetirement::new(sandboxes.clone()));
    let operators = Arc::new(layer2_core::sandbox::SandboxOperatorService::new(
        sandboxes,
        profiles,
        retirement,
        activity.clone(),
    ));
    let store = Arc::new(MemoryAdmissionControlStore::default());
    let service = AdmissionControlService::new(store.clone()).with_writes_enabled(writes);
    (
        router(
            AdminState::new(settings, operators, activity)
                .with_admission_control(Arc::new(service))
                .with_admission_ceilings(Arc::new(
                    layer2_core::admission::AdmissionCeilingsService::new(ceilings_store)
                        .with_writes_enabled(ceilings_writes),
                )),
        ),
        store,
    )
}

async fn body(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

#[tokio::test]
async fn admission_reads_are_authorized_and_report_the_independent_gate() {
    let (app, _) = admission_fixture(false);
    let denied = send(&app, Method::GET, CONTROL, None, []).await;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let read = send(
        &app,
        Method::GET,
        CONTROL,
        None,
        [Capability::ConfigurePlatform],
    )
    .await;
    assert_eq!(read.status(), StatusCode::OK);
    let read = body(read).await;
    assert_eq!(read["revision"], 0);
    assert_eq!(read["operator_stop"], Value::Null);
    assert_eq!(read["last_change"], Value::Null);
    assert_eq!(read["writes_available"], false);
    assert_eq!(read["history"], json!([]));
    assert_eq!(read["ceilings"]["revision"], 0);
    assert_eq!(read["ceilings"]["writes_available"], false);
}

#[tokio::test]
async fn admission_reads_keep_the_twenty_newest_full_changes_when_writes_are_closed() {
    use layer2_core::admission::AdmissionControlStore;
    let (app, store) = admission_fixture(false);
    for revision in 0..26 {
        store
            .change(
                revision,
                &layer2_core::SubjectId::new(format!("operator-{revision}")),
                &format!("reason-{revision}"),
                revision % 2 == 0,
                100 + revision,
            )
            .await
            .unwrap();
    }
    let response = send(
        &app,
        Method::GET,
        CONTROL,
        None,
        [Capability::ConfigurePlatform],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body(response).await;
    assert_eq!(body["writes_available"], false);
    assert_eq!(body["history"].as_array().unwrap().len(), 20);
    assert_eq!(body["history"][0], body["last_change"]);
    assert_eq!(body["history"][0]["revision"], 26);
    assert_eq!(body["history"][0]["actor"], "operator-25");
    assert_eq!(body["history"][0]["reason"], "reason-25");
    assert_eq!(body["history"][0]["old_stop"]["revision"], 25);
    assert!(body["history"][0]["new_stop"].is_null());
    assert_eq!(body["history"][19]["revision"], 7);
    assert_eq!(store.audit_records().unwrap().len(), 26);
}

#[tokio::test]
async fn admission_stop_resume_and_first_writer_wins_use_their_own_revision() {
    let (app, store) = admission_fixture(true);
    let payload = json!({"reason":"contain incident","expected_revision":0});
    let stopped = send(
        &app,
        Method::POST,
        STOP,
        Some(payload.clone()),
        [Capability::ConfigurePlatform],
    )
    .await;
    assert_eq!(stopped.status(), StatusCode::OK);
    let stopped = body(stopped).await;
    assert_eq!(stopped["revision"], 1);
    assert_eq!(stopped["operator_stop"]["reason"], "contain incident");
    let repeated = send(
        &app,
        Method::POST,
        STOP,
        Some(json!({"reason":"second operator","expected_revision":0})),
        [Capability::ConfigurePlatform],
    )
    .await;
    assert_eq!(repeated.status(), StatusCode::OK);
    assert_eq!(body(repeated).await, stopped);
    let stale = send(
        &app,
        Method::POST,
        RESUME,
        Some(payload),
        [Capability::ConfigurePlatform],
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(body(stale).await["code"], "admission_control_conflict");
    let resumed = send(
        &app,
        Method::POST,
        RESUME,
        Some(json!({"reason":"resolved","expected_revision":1})),
        [Capability::ConfigurePlatform],
    )
    .await;
    assert_eq!(resumed.status(), StatusCode::OK);
    let resumed = body(resumed).await;
    assert_eq!(resumed["revision"], 2);
    assert!(resumed["operator_stop"].is_null());
    assert_eq!(resumed["last_change"]["reason"], "resolved");
    assert_eq!(store.audit_records().unwrap().len(), 2);
}

#[tokio::test]
async fn admission_control_closed_gate_and_invalid_input_never_change_state() {
    let (closed, store) = admission_fixture(false);
    for path in [STOP, RESUME] {
        let response = send(
            &closed,
            Method::POST,
            path,
            Some(json!({"reason":"incident","expected_revision":0})),
            [Capability::ConfigurePlatform],
        )
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            body(response).await["code"],
            "admission_control_writes_unavailable"
        );
    }
    assert!(store.audit_records().unwrap().is_empty());
    let (app, store) = admission_fixture(true);
    for payload in [
        json!({"reason":" ","expected_revision":0}),
        json!({"reason":"incident","expected_revision":-1}),
        json!({"reason":"incident","expected_revision":0,"scope":"all"}),
    ] {
        let response = send(
            &app,
            Method::POST,
            STOP,
            Some(payload),
            [Capability::ConfigurePlatform],
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
    assert!(store.audit_records().unwrap().is_empty());
}

#[tokio::test]
async fn pending_homes_reconciliation_defaults_to_read_only_and_requires_the_ceilings_gate() {
    let path = "/api/v1/admin/admission/pending-homes/reconcile";
    let request = json!({"reason": "investigate pending homes", "observed_pending": 0});
    let (closed, _) = admission_fixture_with_ceilings(false, false);
    let denied = send(&closed, Method::POST, path, Some(request.clone()), []).await;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let gated = send(
        &closed,
        Method::POST,
        path,
        Some(request.clone()),
        [Capability::ConfigurePlatform],
    )
    .await;
    assert_eq!(gated.status(), StatusCode::SERVICE_UNAVAILABLE);
    let (open, _) = admission_fixture_with_ceilings(false, true);
    let response = send(
        &open,
        Method::POST,
        path,
        Some(request),
        [Capability::ConfigurePlatform],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let result = body(response).await;
    assert_eq!(
        result,
        json!({
            "previous_pending_launch_homes": 0, "expected_pending_launch_homes": 0,
            "pending_launch_homes": 0, "dry_run": true, "corrected": false
        })
    );
}

#[tokio::test]
async fn pending_homes_requests_reject_invalid_reasons_counts_and_preview_values() {
    let path = "/api/v1/admin/admission/pending-homes/reconcile";
    let (app, _) = admission_fixture_with_ceilings(false, true);
    for request in [
        json!({"reason":" ","observed_pending":0}),
        json!({"reason":"line\nbreak","observed_pending":0}),
        json!({"reason":"a".repeat(513),"observed_pending":0}),
        json!({"reason":"inspect","observed_pending":-1}),
        json!({"reason":"inspect","observed_pending":1.5}),
        json!({"reason":"inspect","observed_pending":9223372036854775808_u64}),
        json!({"reason":"inspect","observed_pending":0,"dry_run":null}),
        json!({"reason":"inspect","observed_pending":0,"dry_run":"false"}),
        json!({"reason":"inspect","observed_pending":0,"unknown":true}),
    ] {
        let response = send(
            &app,
            Method::POST,
            path,
            Some(request),
            [Capability::ConfigurePlatform],
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body(response).await["code"], "invalid_request");
    }
}

#[derive(Default)]
pub(crate) struct PendingHomesFailure(layer2_core::admission::MemoryAdmissionCeilingsStore);
#[async_trait]
impl layer2_core::admission::AdmissionCeilingsStore for PendingHomesFailure {
    async fn read(&self) -> Result<layer2_core::admission::AdmissionCeilingsState, StoreError> {
        self.0.read().await
    }
    async fn read_with_history(
        &self,
    ) -> Result<layer2_core::admission::AdmissionCeilingsRead, StoreError> {
        self.0.read_with_history().await
    }
    async fn replace(
        &self,
        revision: u64,
        actor: &SubjectId,
        reason: &str,
        ceilings: layer2_core::admission::AdmissionCeilings,
        now: u64,
    ) -> Result<layer2_core::admission::AdmissionCeilingsState, StoreError> {
        self.0.replace(revision, actor, reason, ceilings, now).await
    }
    async fn reconcile_pending_homes(
        &self,
        observed: u64,
        _: &SubjectId,
        _: &str,
        _: bool,
        _: u64,
    ) -> Result<layer2_core::admission::PendingHomesReconciliation, StoreError> {
        if observed == 1 {
            Err(StoreError::RevisionConflict {
                expected: 1,
                actual: 2,
            })
        } else {
            Err(StoreError::Unavailable(
                "transaction retry budget exhausted".into(),
            ))
        }
    }
}
#[tokio::test]
async fn pending_homes_conflicts_are_typed_and_retry_exhaustion_is_retryable() {
    let (app, _) = admission_fixture_with_ceilings_store(
        false,
        true,
        Arc::new(PendingHomesFailure::default()),
    );
    let path = "/api/v1/admin/admission/pending-homes/reconcile";
    for (observed, status, code) in [
        (1, StatusCode::CONFLICT, "pending_homes_conflict"),
        (
            2,
            StatusCode::SERVICE_UNAVAILABLE,
            "pending_homes_unavailable",
        ),
    ] {
        let response = send(
            &app,
            Method::POST,
            path,
            Some(json!({"reason":"inspect drift","observed_pending":observed,"dry_run":false})),
            [Capability::ConfigurePlatform],
        )
        .await;
        assert_eq!(response.status(), status);
        if status == StatusCode::SERVICE_UNAVAILABLE {
            assert_eq!(response.headers()[header::RETRY_AFTER], "60");
        }
        assert_eq!(body(response).await["code"], code);
    }
}
