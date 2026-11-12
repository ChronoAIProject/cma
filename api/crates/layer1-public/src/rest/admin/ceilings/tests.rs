use super::*;
use layer2_core::admission::{
    AdmissionCeilings, AdmissionCeilingsRead, AdmissionCeilingsState, AdmissionCeilingsStore,
    MemoryAdmissionCeilingsStore,
};

#[derive(Default)]
struct PendingHomes(MemoryAdmissionCeilingsStore);

#[async_trait]
impl AdmissionCeilingsStore for PendingHomes {
    async fn read(&self) -> Result<AdmissionCeilingsState, StoreError> {
        self.0.read().await
    }

    async fn read_with_history(&self) -> Result<AdmissionCeilingsRead, StoreError> {
        let mut read = self.0.read_with_history().await?;
        read.counts.pending_launch_homes = 9;
        Ok(read)
    }

    async fn replace(
        &self,
        expected: u64,
        actor: &SubjectId,
        reason: &str,
        ceilings: AdmissionCeilings,
        now: u64,
    ) -> Result<AdmissionCeilingsState, StoreError> {
        self.0.replace(expected, actor, reason, ceilings, now).await
    }
}

#[tokio::test]
async fn admin_read_exposes_exact_pending_homes_without_an_effective_home_ceiling() {
    let (app, _) = super::admission_control_tests::admission_fixture_with_ceilings_store(
        false,
        false,
        Arc::new(PendingHomes::default()),
    );
    let response = request(&app, true, Method::GET, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let read = value(response).await;
    assert!(read["effective_limits"]["retained_homes"].is_null());
    assert!(read["capacity"]["retained_homes"].is_null());
    assert_eq!(read["capacity"]["pending_launch_homes"], 9);
}

fn fixture(writes: bool) -> Router {
    super::admission_control_tests::admission_fixture_with_ceilings(false, writes).0
}
async fn request(app: &Router, authorized: bool, method: Method, body: Option<Value>) -> Response {
    let capabilities = if authorized {
        vec![Capability::ConfigurePlatform]
    } else {
        vec![]
    };
    send(
        app,
        method,
        if body.is_some() {
            "/api/v1/admin/admission/ceilings"
        } else {
            "/api/v1/admin/admission"
        },
        body,
        capabilities,
    )
    .await
}
async fn value(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}
#[tokio::test]
async fn ceilings_api_is_independently_gated_authorized_and_revision_checked() {
    let payload = json!({"ceilings":{"active_sessions":3,"existing_reserve":{"active_sessions":1}},"reason":"bound capacity","expected_revision":0});
    let denied = request(&fixture(true), false, Method::PUT, Some(payload.clone())).await;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let closed = request(&fixture(false), true, Method::PUT, Some(payload.clone())).await;
    assert_eq!(closed.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        value(closed).await["code"],
        "admission_ceilings_writes_unavailable"
    );
    let app = fixture(true);
    let saved = request(&app, true, Method::PUT, Some(payload.clone())).await;
    assert_eq!(saved.status(), StatusCode::OK);
    assert_eq!(value(saved).await["revision"], 1);
    let stale = request(&app, true, Method::PUT, Some(payload)).await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    let read = value(request(&app, true, Method::GET, None).await).await;
    assert_eq!(read["revision"], 0);
    assert_eq!(read["ceilings"]["revision"], 1);
    assert_eq!(read["ceilings_history"].as_array().unwrap().len(), 1);
    assert_eq!(read["effective_limits"]["active_sessions"], 3);
    assert_eq!(read["capacity"]["pending_launch_homes"], 0);
}
#[tokio::test]
async fn ceilings_api_rejects_reserve_overflow_and_unknown_fields() {
    for ceilings in [
        json!({"active_sessions":2,"existing_reserve":{"active_sessions":3}}),
        json!({"unknown":1}),
    ] {
        let response = request(
            &fixture(true),
            true,
            Method::PUT,
            Some(json!({"ceilings":ceilings,"reason":"invalid policy","expected_revision":0})),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
