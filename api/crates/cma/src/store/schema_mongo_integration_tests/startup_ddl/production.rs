use super::*;
use crate::store::{mongo::MongoConnectionPolicy, MongoStores};
use cma_service_auth::mongodb_authority::MongoAuthorityMode;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn production_connection_enforces_both_converted_contracts_before_returning_stores() {
    let (admin, name) = database().await;
    let db = admin.database(&name);
    let password = uuid::Uuid::new_v4().simple().to_string();
    db.run_command(doc! {"createUser":"schema-runtime","pwd":&password,
    "roles":[{"role":"readWrite","db":&name},{"role":"dbAdmin","db":&name}]})
        .await
        .unwrap();
    let mut uri = url::Url::parse(&std::env::var("CHRONO_TEST_MONGODB_URI").unwrap()).unwrap();
    uri.set_username("schema-runtime").unwrap();
    uri.set_password(Some(&password)).unwrap();
    uri.query_pairs_mut().append_pair("authSource", &name);
    let connect = |mode| {
        let key = auth_key();
        let uri = uri.to_string();
        let name = name.clone();
        async move {
            MongoStores::connect_with_policy(
                &uri,
                &name,
                "schema-fixture",
                &key,
                RETENTION,
                ("runtime-schema-fixture", None),
                MongoConnectionPolicy {
                    shared_observation: false,
                    authority: MongoAuthorityMode::SharedSchema,
                    agent_manager_replicas: false,
                    startup_schema: mode,
                    initial_settings: Default::default(),
                    released: None,
                },
            )
            .await
        }
    };
    let empty_strict = connect(StartupSchemaMode::Enforce).await.err();
    let empty_observe = connect(StartupSchemaMode::Observe).await;
    let observation_succeeded = empty_observe.is_ok();
    drop(empty_observe);
    let collections = db.list_collection_names().await.unwrap();
    for name in ["account_deletions", "account_deletion_events"] {
        assert!(
            !collections.iter().any(|existing| existing == name),
            "disabled deletion admission must not create {name} at startup"
        );
    }
    crate::store::migrations::prepare_bindings_for_test(&db).await;
    let missing_retention = connect(StartupSchemaMode::Enforce).await.err();
    super::super::activity_retention::prepare_activity_schema(&db, RETENTION).await;
    let prepared = connect(StartupSchemaMode::Enforce).await;
    let prepared_succeeded = prepared.is_ok();
    if let Ok(stores) = &prepared {
        stores.ping().await.unwrap();
    }
    drop(prepared);
    db.run_command(doc! {"dropUser":"schema-runtime"})
        .await
        .unwrap();
    db.drop().await.unwrap();
    assert!(
        matches!(empty_strict, Some(StoreError::Unavailable(message)) if message.starts_with("binding schema drift"))
    );
    assert!(
        observation_succeeded,
        "observation mode added a readiness failure"
    );
    assert!(
        matches!(missing_retention, Some(StoreError::Unavailable(message)) if message.starts_with("activity schema drift"))
    );
    assert!(prepared_succeeded, "prepared enforced connection failed");
}
