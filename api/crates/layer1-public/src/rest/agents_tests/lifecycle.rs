//! The domain operation ledger, not a cached HTTP 202, handles lifecycle replay.
use super::*;
use crate::rest::{
    AgentLifecycleCreateReceipt, AgentLifecycleCreateRequest, AgentLifecycleProvisioner,
    AgentLifecycleProvisioningError,
};
use layer2_core::{resource_access::CallerContext, Caller};

type LifecycleCall = (String, String, String, Option<String>);

#[derive(Default)]
struct Provisioner {
    calls: Mutex<Vec<LifecycleCall>>,
    requests: Mutex<Vec<AgentLifecycleCreateRequest>>,
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
        self.requests.lock().unwrap().push(request.clone());
        self.calls.lock().unwrap().push((
            caller.subject.to_string(),
            key.into(),
            request.name,
            request.workspace_id,
        ));
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
            (
                OWNER.into(),
                "persisted-intent".into(),
                "Research".into(),
                None
            ),
            (
                OWNER.into(),
                "persisted-intent".into(),
                "Research".into(),
                None
            ),
        ]
    );
}

#[tokio::test]
async fn contextual_agent_creation_forwards_workspace_through_the_same_durable_route() {
    let fixture = fixture().await;
    let port = Arc::new(Provisioner::default());
    let router = agent_router(fixture.state.with_lifecycle(port.clone()));
    let mut call = request(
        Method::POST,
        "/api/v1/agents",
        OWNER,
        None,
        Some(
            json!({"name":"Second Agent","workspace_id":"wk_existing","profile":{"model":"model"}}),
        ),
    );
    call.extensions_mut()
        .insert(layer2_core::Authorization::new([
            layer2_core::Capability::UseOwnSandboxes,
        ]));
    call.headers_mut()
        .insert("Idempotency-Key", "same-context".parse().unwrap());
    let response = router.oneshot(call).await.unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(
        *port.calls.lock().unwrap(),
        [(
            OWNER.into(),
            "same-context".into(),
            "Second Agent".into(),
            Some("wk_existing".into())
        )]
    );
}

#[tokio::test]
async fn lifecycle_forwards_distinct_browser_and_machine_intents_without_aliasing() {
    let fixture = fixture().await;
    let port = Arc::new(Provisioner::default());
    let router = agent_router(fixture.state.with_lifecycle(port.clone()));
    for (body, desktop, machine_included) in [
        (json!({"name":"Legacy"}), false, false),
        (
            json!({"name":"Machine","machine_included":true}),
            false,
            true,
        ),
        (
            json!({"name":"Task","desktop":true,"initial_message":"  Keep this task\n"}),
            true,
            false,
        ),
        (
            json!({"name":"Both","desktop":true,"machine_included":true,"initial_message":"  Keep this task\n"}),
            true,
            true,
        ),
    ] {
        let mut call = request(
            Method::POST,
            "/api/v1/agents",
            OWNER,
            None,
            Some(body.clone()),
        );
        call.headers_mut().insert(
            "Idempotency-Key",
            format!("intent-{desktop}-{machine_included}")
                .parse()
                .unwrap(),
        );
        let response = router.clone().oneshot(call).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let requests = port.requests.lock().unwrap();
        let forwarded = requests.last().unwrap();
        assert_eq!(forwarded.desktop, desktop);
        assert_eq!(forwarded.machine_included, machine_included);
        assert_eq!(
            forwarded.initial_message.as_deref(),
            body["initial_message"].as_str()
        );
    }
    let mut call = request(
        Method::POST,
        "/api/v1/agents",
        OWNER,
        None,
        Some(json!({"name":"Missing task","desktop":true})),
    );
    call.headers_mut()
        .insert("Idempotency-Key", "missing-task".parse().unwrap());
    let response = router.oneshot(call).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(port.requests.lock().unwrap().len(), 4);
}
