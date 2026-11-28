use super::*;
use crate::rest::admin::warm_pools::*;
use layer2_core::Caller;

#[derive(Default)]
struct Pools(Mutex<Vec<String>>);
fn view() -> WarmPoolView {
    WarmPoolView {
        id: "wp_original".into(),
        revision: 1,
        config_sha256: "c".repeat(64),
        image: "image@sha256:original".into(),
        cpu_millis: 1000,
        memory_mib: 1024,
        target_replicas: 3,
        provider_target_replicas: None,
        ready_replicas: None,
        observed: false,
        status: "pending".into(),
        retirement_status: None,
        resource_name: None,
        error: None,
        machine: false,
        current_runtime: None,
    }
}
#[async_trait]
impl WarmPoolAdministration for Pools {
    async fn list(&self, _: &Caller) -> Result<WarmPoolList, WarmPoolError> {
        self.0.lock().unwrap().push("list".into());
        Ok(WarmPoolList {
            items: vec![view()],
            default_image: "default".into(),
            default_cpu_millis: 1000,
            default_memory_mib: 1024,
            default_config_sha256: Some("d".repeat(64)),
            available: false,
            machine_image: None,
            machine_config_sha256: None,
            default_claim_pool_id: None,
            machine_claim_pool_id: None,
            default_capacity_pool_id: None,
            machine_capacity_pool_id: None,
            runtime_versions: vec![],
        })
    }
    async fn configure(
        &self,
        _: &Caller,
        key: &str,
        _: ConfigureWarmPoolBody,
    ) -> Result<WarmPoolView, WarmPoolError> {
        self.0.lock().unwrap().push(key.into());
        Ok(view())
    }
    async fn resize(
        &self,
        _: &Caller,
        _: &str,
        body: ResizeWarmPoolBody,
    ) -> Result<WarmPoolView, WarmPoolError> {
        self.0.lock().unwrap().push("resize".into());
        if body.expected_revision != 1 {
            Err(WarmPoolError::Conflict)
        } else {
            Ok(view())
        }
    }
}

#[tokio::test]
async fn warm_pool_routes_authorize_before_service_and_require_durable_command_key() {
    let pools = Arc::new(Pools::default());
    let fixture = fixture_with_pools(false, Some(pools.clone()));
    for (method, path, body) in [
        (Method::GET, "/api/v1/admin/warm-pools", None),
        (
            Method::POST,
            "/api/v1/admin/warm-pools",
            Some(json!({"cpu_millis":1000,"memory_mib":1024,"target_replicas":3})),
        ),
        (
            Method::PUT,
            "/api/v1/admin/warm-pools/wp_original",
            Some(json!({"expected_revision":1,"target_replicas":0})),
        ),
    ] {
        let response = send(
            &fixture.app,
            method,
            path,
            body,
            [Capability::ReadRuntimeInventory],
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    assert!(pools.0.lock().unwrap().is_empty());
    let body = json!({"cpu_millis":1000,"memory_mib":1024,"target_replicas":3});
    let missing_key = send(
        &fixture.app,
        Method::POST,
        "/api/v1/admin/warm-pools",
        Some(body.clone()),
        [Capability::ConfigurePlatform],
    )
    .await;
    assert_eq!(missing_key.status(), StatusCode::BAD_REQUEST);
    assert!(pools.0.lock().unwrap().is_empty());
    let mut configure = request(
        Method::POST,
        "/api/v1/admin/warm-pools",
        Some(body),
        [Capability::ConfigurePlatform],
    );
    configure
        .headers_mut()
        .insert("Idempotency-Key", "create-original".parse().unwrap());
    let accepted = fixture.app.clone().oneshot(configure).await.unwrap();
    assert_eq!(accepted.status(), StatusCode::ACCEPTED);
    assert_eq!(response_json(accepted).await["id"], "wp_original");
    assert_eq!(*pools.0.lock().unwrap(), ["create-original"]);
    let stale = send(
        &fixture.app,
        Method::PUT,
        "/api/v1/admin/warm-pools/wp_original",
        Some(json!({"expected_revision":2,"target_replicas":0})),
        [Capability::ConfigurePlatform],
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn unavailable_inventory_is_explicit_and_pool_configuration_refuses_client_image_or_disk_policy(
) {
    let pools = Arc::new(Pools::default());
    let fixture = fixture_with_pools(false, Some(pools.clone()));
    let list = send(
        &fixture.app,
        Method::GET,
        "/api/v1/admin/warm-pools",
        None,
        [Capability::ConfigurePlatform],
    )
    .await;
    assert_eq!(list.status(), StatusCode::OK);
    let list = response_json(list).await;
    assert_eq!(list["available"], false);
    assert_eq!(list["items"][0]["ready_replicas"], Value::Null);
    // Without a configured desktop image the list says so explicitly.
    assert_eq!(list["items"][0]["machine"], false);
    assert_eq!(list["machine_image"], Value::Null);
    for extra in ["image", "home", "profile"] {
        let mut body = json!({"cpu_millis":1000,"memory_mib":1024,"target_replicas":1});
        body[extra] = json!("untrusted");
        let mut configure = request(
            Method::POST,
            "/api/v1/admin/warm-pools",
            Some(body),
            [Capability::ConfigurePlatform],
        );
        configure
            .headers_mut()
            .insert("Idempotency-Key", "create-original".parse().unwrap());
        assert_eq!(
            fixture
                .app
                .clone()
                .oneshot(configure)
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(*pools.0.lock().unwrap(), ["list"]);
}
