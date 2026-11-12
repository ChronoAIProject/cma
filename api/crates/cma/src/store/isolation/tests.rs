use super::*;
#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn real_agent_manager_credential_cannot_read_gateway_or_images() {
    use futures_util::TryStreamExt as _;
    use mongodb::{
        options::{ClientOptions, Credential},
        Client,
    };
    let uri = std::env::var("CMA_SERVICE_AUTH_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_am_contract_{}", uuid::Uuid::new_v4().simple());
    let db = admin.database(&name);
    db.run_command(role(&name)).await.unwrap();
    let password = uuid::Uuid::new_v4().to_string();
    db.run_command(doc!{"createUser":"agent-runtime","pwd":&password,"roles":[{"role":"cma_am_runtime","db":&name}]}).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.credential = Some(
        Credential::builder()
            .username("agent-runtime".to_owned())
            .password(password)
            .source(name.clone())
            .build(),
    );
    let runtime = Client::with_options(options).unwrap();
    let isolated = runtime.database(&name);
    verify(&isolated).await.unwrap();
    assert!(verify(&db).await.is_err());
    let key = crate::store::AuthEncryptionKey::from_base64(&base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        [7_u8; 32],
    ))
    .unwrap();
    let before = db.list_collection_names().await.unwrap();
    let broad = crate::store::MongoStores::connect(
        &uri,
        &name,
        "refused-bootstrap",
        &key,
        std::time::Duration::from_secs(86_400),
        ("runtime-isolation-contract", None),
    )
    .await;
    assert!(
        matches!(broad, Err(StoreError::Unavailable(ref detail)) if detail.contains("not isolated"))
    );
    assert_eq!(db.list_collection_names().await.unwrap(), before);
    for _ in 0..2 {
        let stores = crate::store::MongoStores::open(
            runtime.clone(),
            &name,
            "isolated-bootstrap",
            &key,
            std::time::Duration::from_secs(86_400),
            std::sync::Arc::new(super::super::checkpoint_storage::RetiredCheckpointStorage),
            ("runtime-isolation-contract", None),
        )
        .await
        .expect("the complete production store assembly must initialize with runtime grants");
        stores.ping().await.unwrap();
        let indexes: Vec<_> = isolated
            .collection::<Document>(super::super::sandbox_mongo::SANDBOX_BILLING)
            .list_indexes()
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        assert!(indexes.iter().any(|index| {
            index
                .options
                .as_ref()
                .and_then(|options| options.name.as_deref())
                == Some("sandbox_billing_due")
                && index.keys == doc! {"retry_at":1,"_id":1}
        }));
        assert!(stores.sandbox_model.warm_runtime_version.is_none());
        assert!(stores.sandbox_model.warm_home_policy.is_none());
        assert_eq!(
            stores.sandbox_model.managed_runtime_version.as_deref(),
            Some("runtime-isolation-contract")
        );
    }
    use layer3_adapter::hosted::RuntimeRoutingStore;
    let routing =
        super::super::runtime_routing::MongoRuntimeRouting::open(&isolated, "fixture", true)
            .await
            .unwrap();
    assert!(routing.select("new-sandbox", 1, true).await.unwrap());
    let restarted =
        super::super::runtime_routing::MongoRuntimeRouting::open(&isolated, "fixture", false)
            .await
            .unwrap();
    assert!(restarted.select("new-sandbox", 1, false).await.unwrap());
    assert!(!restarted.select("new-sandbox", 2, false).await.unwrap());
    assert!(restarted.select("overflow", u64::MAX, true).await.is_err());

    profile_internal_contract::verify(&runtime, &name).await;
    agent_lifecycle_contract::verify(&isolated).await;
    lifecycle_children_contract::verify(&runtime, &name).await;

    for foreign in [
        "gateway_channel_records",
        "gateway_connection_records",
        "gateway_interaction_records",
        "eg_channel_records",
        "eg_auth_sessions",
        "im_images",
        "im_builds",
        "foreign_auth_sessions",
    ] {
        db.collection::<Document>(foreign)
            .insert_one(doc! {"_id":"private"})
            .await
            .unwrap();
        let error = isolated
            .collection::<Document>(foreign)
            .find_one(doc! {})
            .await
            .unwrap_err();
        assert!(matches!(error.kind.as_ref(),mongodb::error::ErrorKind::Command(c) if c.code==13));
    }
    db.drop().await.unwrap();
}
#[test]
fn rejects_foreign_and_database_wide_privileges() {
    let grants = role("cma").get_array("privileges").unwrap().clone();
    let status = doc! {"authInfo":{"authenticatedUsers":[{"user":"agent-manager","db":"cma"}],"authenticatedUserPrivileges":grants.clone()}};
    validate(&status, "cma").unwrap();
    for collection in [
        "im_images",
        "eg_channel_records",
        "gateway_channel_records",
        "",
    ] {
        let mut changed = grants.clone();
        changed
            .push(doc! {"resource":{"db":"cma","collection":collection},"actions":["find"]}.into());
        assert!(validate(&doc!{"authInfo":{"authenticatedUsers":[{"user":"agent-manager"}],"authenticatedUserPrivileges":changed}},"cma").is_err());
    }
}

#[test]
fn durable_target_grants_are_required_without_database_wide_access() {
    for target in [
        super::super::native_warm_pools::COLLECTION,
        super::super::native_prepared::COLLECTION,
        super::super::media_uploads::COLLECTION,
        super::super::sandbox_mongo::SANDBOX_BILLING,
    ] {
        let mut grants = role("cma").get_array("privileges").unwrap().clone();
        let grant = grants
            .iter()
            .find(|grant| {
                grant
                    .as_document()
                    .unwrap()
                    .get_document("resource")
                    .unwrap()
                    .get_str("collection")
                    .unwrap()
                    == target
            })
            .expect("the generated runtime role includes every durable target");
        assert!(grant
            .as_document()
            .unwrap()
            .get_array("actions")
            .unwrap()
            .iter()
            .any(|action| action.as_str() == Some("createIndex")));
        grants.retain(|grant| {
            grant
                .as_document()
                .unwrap()
                .get_document("resource")
                .unwrap()
                .get_str("collection")
                .unwrap()
                != target
        });
        let status = doc! {"authInfo":{"authenticatedUsers":[{"user":"agent-manager"}],"authenticatedUserPrivileges":grants}};
        assert!(validate(&status, "cma").is_err());
    }
}

#[test]
fn index_removal_is_confined_to_the_binding_migration() {
    let grants = role("cma").get_array("privileges").unwrap().clone();
    for collection in [
        "sandboxes",
        "agents",
        super::super::sandbox_mongo::SANDBOX_BILLING,
        "",
    ] {
        let mut changed = grants.clone();
        changed.push(
            doc! {"resource":{"db":"cma","collection":collection},"actions":["dropIndex"]}.into(),
        );
        let status = doc! {"authInfo":{"authenticatedUsers":[{"user":"agent-manager"}],"authenticatedUserPrivileges":changed}};
        assert!(validate(&status, "cma").is_err());
    }
    let mut missing = grants;
    for grant in &mut missing {
        let grant = grant.as_document_mut().unwrap();
        grant
            .get_array_mut("actions")
            .unwrap()
            .retain(|action| action.as_str() != Some("dropIndex"));
    }
    let status = doc! {"authInfo":{"authenticatedUsers":[{"user":"agent-manager"}],"authenticatedUserPrivileges":missing}};
    assert!(validate(&status, "cma").is_err());
}

fn progress_grant(database: &str) -> Document {
    doc! {"resource":{"db":database,"collection":super::super::provenance_progress::COLLECTION},
    "actions":super::super::provenance_progress::ACTIONS}
}
fn status(grants: Vec<mongodb::bson::Bson>) -> Document {
    doc! {"authInfo":{"authenticatedUsers":[{"user":"fixture"}],"authenticatedUserPrivileges":grants}}
}

#[test]
fn progress_grants_are_optional_but_exact_and_confined() {
    let base = role("cma").get_array("privileges").unwrap().clone();
    validate(&status(base.clone()), "cma").unwrap();
    let mut full = base.clone();
    full.push(progress_grant("cma").into());
    validate(&status(full), "cma").unwrap();
    for forbidden in [
        "remove",
        "dropCollection",
        "dropIndex",
        "collMod",
        "createIndex",
    ] {
        let mut grant = progress_grant("cma");
        grant
            .get_array_mut("actions")
            .unwrap()
            .push(forbidden.into());
        let mut grants = base.clone();
        grants.push(grant.into());
        assert!(validate(&status(grants), "cma").is_err());
    }
    for missing in super::super::provenance_progress::ACTIONS {
        let mut grant = progress_grant("cma");
        grant
            .get_array_mut("actions")
            .unwrap()
            .retain(|a| a.as_str() != Some(missing));
        let mut grants = base.clone();
        grants.push(grant.into());
        assert!(validate(&status(grants), "cma").is_err());
    }
    let mut foreign = base;
    foreign.push(progress_grant("foreign").into());
    assert!(validate(&status(foreign), "cma").is_err());
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn real_progress_grants_cannot_delete_receipts_or_access_another_service() {
    use mongodb::{
        options::{ClientOptions, Credential},
        Client,
    };
    let uri = std::env::var("CMA_SERVICE_AUTH_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_progress_auth_{}", uuid::Uuid::new_v4().simple());
    let db = admin.database(&name);
    db.run_command(role(&name)).await.unwrap();
    db.run_command(doc! {"createRole":"progress","privileges":[progress_grant(&name)],"roles":[]})
        .await
        .unwrap();
    let password = uuid::Uuid::new_v4().to_string();
    db.run_command(
        doc! {"createUser":"progress-fixture","pwd":&password,"roles":[
        {"role":"cma_am_runtime","db":&name},{"role":"progress","db":&name}]},
    )
    .await
    .unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.credential = Some(
        Credential::builder()
            .username("progress-fixture".to_owned())
            .password(password)
            .source(name.clone())
            .build(),
    );
    let client = Client::with_options(options).unwrap();
    let scoped = client.database(&name);
    verify(&scoped).await.unwrap();
    super::super::provenance_progress::initialize(&scoped)
        .await
        .unwrap();
    let rows = scoped.collection::<Document>(super::super::provenance_progress::COLLECTION);
    assert_eq!(rows.count_documents(doc! {}).await.unwrap(), 1);
    let error = rows.delete_many(doc! {}).await.unwrap_err();
    assert!(matches!(error.kind.as_ref(),mongodb::error::ErrorKind::Command(c) if c.code == 13));
    for foreign in ["eg_channel_records", "im_images"] {
        let error = scoped
            .collection::<Document>(foreign)
            .find_one(doc! {})
            .await
            .unwrap_err();
        assert!(
            matches!(error.kind.as_ref(),mongodb::error::ErrorKind::Command(c) if c.code == 13)
        );
    }
    assert_eq!(rows.count_documents(doc! {}).await.unwrap(), 1);
    db.drop().await.unwrap();
}
