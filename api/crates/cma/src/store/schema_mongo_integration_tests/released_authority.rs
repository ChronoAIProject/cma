//! Every store copy handed out at startup shares the caller's one release.
use super::*;
use crate::store::mongo::MongoConnectionPolicy;
use cma_service_auth::mongodb_authority::MongoAuthorityMode;
use layer3_adapter::{PinnedReleasedRuntime, ReleasedRuntimeSource};
use std::sync::Arc;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn opened_stores_share_the_one_released_runtime_authority() {
    let (client, name) = database().await;
    let password = uuid::Uuid::new_v4().simple().to_string();
    client
        .database(&name)
        .run_command(doc! {
            "createUser": "released-authority-runtime", "pwd": &password,
            "roles": [{"role": "readWrite", "db": &name}, {"role": "dbAdmin", "db": &name}],
        })
        .await
        .unwrap();
    let mut uri = url::Url::parse(&std::env::var("CHRONO_TEST_MONGODB_URI").unwrap()).unwrap();
    uri.set_username("released-authority-runtime").unwrap();
    uri.set_password(Some(&password)).unwrap();
    uri.query_pairs_mut().append_pair("authSource", &name);
    let released: Arc<dyn ReleasedRuntimeSource> = Arc::new(PinnedReleasedRuntime::new(
        "registry.example/runtime@sha256:released",
        Some("registry.example/machine@sha256:released".into()),
    ));
    let stores = MongoStores::connect_with_policy(
        uri.as_str(),
        &name,
        "released-authority-fixture",
        &auth_key(),
        Duration::from_secs(86_400),
        ("registry.example/runtime@sha256:bootstrap", None),
        MongoConnectionPolicy {
            authority: MongoAuthorityMode::SharedSchema,
            agent_manager_replicas: false,
            startup_schema: Default::default(),
            shared_observation: false,
            initial_settings: Default::default(),
            released: Some(released.clone()),
            runtime_pointer_required: false,
        },
    )
    .await
    .unwrap();
    let same = |holder: &Arc<dyn ReleasedRuntimeSource>| {
        std::ptr::addr_eq(Arc::as_ptr(holder), Arc::as_ptr(&released))
    };
    assert!(same(&stores.released_runtime));
    assert!(stores.sandbox_model.released.as_ref().is_some_and(same));
    // The configured release, not the bootstrap pin, answers for both lanes.
    assert_eq!(
        stores.sandbox_model.managed_agent_release().as_deref(),
        Some("registry.example/runtime@sha256:released")
    );
    assert_eq!(
        stores.sandbox_model.released_runtime().machine.as_deref(),
        Some("registry.example/machine@sha256:released")
    );
    client.database(&name).drop().await.unwrap();
}
