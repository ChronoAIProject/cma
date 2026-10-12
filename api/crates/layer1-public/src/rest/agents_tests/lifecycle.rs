//! The domain operation ledger, not a cached HTTP 202, handles lifecycle replay.
use super::*;
use crate::rest::{
    AgentLifecycleCreateReceipt, AgentLifecycleCreateRequest, AgentLifecycleProvisioner,
    AgentLifecycleProvisioningError,
};
use layer2_core::{resource_access::CallerContext, Caller};

#[derive(Default)]
struct Provisioner {
    calls: Mutex<Vec<(String, String, String)>>,
}
#[async_trait]
impl AgentLifecycleProvisioner for Provisioner {
    async fn create(
        &self,
        caller: &Caller,
        _: Option<&CallerContext>,
        _: Option<&str>,
        request: AgentLifecycleCreateRequest,
        key: &str,
    ) -> Result<AgentLifecycleCreateReceipt, AgentLifecycleProvisioningError> {
        self.calls
            .lock()
            .unwrap()
            .push((caller.subject.to_string(), key.into(), request.name));
        Ok(AgentLifecycleCreateReceipt {
            id: "same-operation".into(),
            status: "provisioning".into(),
        })
    }
}
#[tokio::test]
async fn connection_retry_reaches_the_durable_operation_and_requires_a_key() {
    let fixture = fixture().await;
    let port = Arc::new(Provisioner::default());
    let router = agent_router(fixture.state.with_lifecycle(port.clone()));
    let command = || {
        request(
            Method::POST,
            "/api/v1/agents",
            OWNER,
            None,
            Some(json!({"name":"Research"})),
        )
    };
    let missing = router.clone().oneshot(command()).await.unwrap();
    assert_eq!(missing.status(), StatusCode::BAD_REQUEST);
    assert!(port.calls.lock().unwrap().is_empty());
    for _ in 0..2 {
        let mut call = command();
        call.headers_mut()
            .insert("Idempotency-Key", "persisted-intent".parse().unwrap());
        let response = router.clone().oneshot(call).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(body["id"], "same-operation");
    }
    assert_eq!(
        port.calls.lock().unwrap().as_slice(),
        &[
            (OWNER.into(), "persisted-intent".into(), "Research".into()),
            (OWNER.into(), "persisted-intent".into(), "Research".into()),
        ]
    );
}
