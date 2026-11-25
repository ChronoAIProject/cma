use super::*;

#[path = "transaction_opening_tests.rs"]
mod real_transaction_opening;

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn real_erasure_optional_authority_preserves_disabled_deployments() {
    crate::store::account_erasure::optional_authority_tests::verify_runtime().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn real_allocation_buckets_activate_only_with_confined_namespace_grants() {
    super::allocation_tests::verify_runtime().await;
}
#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn real_cancelled_terminal_command_can_end_its_own_session_under_the_exact_runtime_grant() {
    use mongodb::{
        options::{ClientOptions, Credential},
        Client,
    };
    let uri = std::env::var("CMA_IM_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_terminal_grant_{}", uuid::Uuid::new_v4().simple());
    let db = admin.database(&name);
    db.run_command(role(&name)).await.unwrap();
    let password = uuid::Uuid::new_v4().to_string();
    db.run_command(doc! {"createUser":"terminal-runtime","pwd":&password,
    "roles":[{"role":"cma_am_runtime","db":&name}]})
        .await
        .unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.credential = Some(
        Credential::builder()
            .username("terminal-runtime".to_owned())
            .password(password)
            .source(name.clone())
            .build(),
    );
    let runtime = Client::with_options(options).unwrap();
    let agents = runtime.database(&name).collection::<Document>("agents");
    let mut session = runtime.start_session().await.unwrap();
    session.start_transaction().await.unwrap();
    agents
        .find_one(doc! {})
        .session(&mut session)
        .await
        .expect("the runtime grant reads agents");
    // Control: the credential really is confined to its grant.
    let foreign = runtime
        .database(&format!("{name}_foreign"))
        .collection::<Document>("agents")
        .find_one(doc! {})
        .await
        .unwrap_err();
    assert!(
        matches!(foreign.kind.as_ref(), mongodb::error::ErrorKind::Command(c) if c.code == 13),
        "the runtime credential must be refused outside its grant"
    );
    // The guard's recovery command needs no privilege beyond owning the session (#3288).
    crate::store::transaction_terminal::kill_session(&runtime, session.id())
        .await
        .expect("a runtime credential may end its own session");
    assert!(
        agents
            .find_one(doc! {})
            .session(&mut session)
            .await
            .is_err(),
        "ending the session aborts its open transaction"
    );
    drop(session);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn real_transaction_barrier_is_authorized_by_the_exact_runtime_grant() {
    use mongodb::{
        bson::Document,
        options::{ClientOptions, Credential},
        Client,
    };
    let uri = std::env::var("CMA_IM_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_barrier_grant_{}", uuid::Uuid::new_v4().simple());
    let db = admin.database(&name);
    db.run_command(role(&name)).await.unwrap();
    let password = uuid::Uuid::new_v4().to_string();
    db.run_command(doc! {"createUser":"barrier-runtime","pwd":&password,
    "roles":[{"role":"cma_am_runtime","db":&name}]})
        .await
        .unwrap();
    let (commands, mut observed) = tokio::sync::mpsc::unbounded_channel();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.credential = Some(
        Credential::builder()
            .username("barrier-runtime".to_owned())
            .password(password)
            .source(name.clone())
            .build(),
    );
    options.command_event_handler = Some(mongodb::event::EventHandler::callback(move |event| {
        if let mongodb::event::command::CommandEvent::Started(event) = event {
            if event.command.get_bool("startTransaction").ok() == Some(true) {
                let _ = commands.send(event);
            }
        }
    }));
    crate::store::transaction_barrier::bind(&mut options, &name);
    let runtime = Client::with_options(options).unwrap();
    let mut session = runtime.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .expect("the barrier must run under the exact runtime grant");
    let opening = observed.try_recv().unwrap();
    assert_eq!(opening.db, name);
    assert_eq!(opening.command_name, "aggregate");
    assert_eq!(opening.command.get_i32("aggregate").unwrap(), 1);
    assert_eq!(
        opening.command.get_array("pipeline").unwrap(),
        doc! {"pipeline": [{"$documents": [doc! {}]}]}
            .get_array("pipeline")
            .unwrap()
    );
    session.commit_transaction().await.unwrap();
    // The literal command grants no access to data in other databases.
    let foreign = runtime.database(&format!("{name}_foreign"));
    let error = foreign
        .collection::<Document>("agents")
        .find_one(doc! {})
        .await
        .unwrap_err();
    assert!(
        matches!(error.kind.as_ref(), mongodb::error::ErrorKind::Command(c) if c.code == 13),
        "an actual read must be refused outside the runtime grant"
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn real_agent_manager_credential_cannot_read_gateway_or_images() {
    use futures_util::TryStreamExt as _;
    use mongodb::{
        options::{ClientOptions, Credential},
        Client,
    };
    let uri = std::env::var("CMA_IM_TEST_MONGODB_URI").unwrap();
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
            stores.sandbox_model.managed_agent_release().as_deref(),
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
        "im_auth_sessions",
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
    // Existing isolated grants remain sufficient while the switch is off.
    // Operators can install the additive grants before enabling ownership.
    use cma_service_auth::mongodb_authority::MongoAuthorityMode;
    assert!(
        replicas::verify(&isolated, MongoAuthorityMode::Isolated, true)
            .await
            .is_err()
    );
    let mut upgraded = doc! {"updateRole": "cma_am_runtime"};
    let mut grants = replicas::role(&name, true);
    grants.remove("createRole");
    upgraded.extend(grants);
    db.run_command(upgraded).await.unwrap();
    verify(&isolated).await.unwrap();
    replicas::verify(&isolated, MongoAuthorityMode::Isolated, true)
        .await
        .unwrap();
    super::super::agent_ownership::MongoAgentOwnership::open(&runtime, &name)
        .await
        .unwrap();
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

#[test]
fn activity_metadata_read_is_scoped_to_the_activity_collection() {
    let grants = activity_index_inspection_role("cma")
        .get_array("privileges")
        .unwrap()
        .clone();
    validate_activity_index_inspection(&status(grants.clone()), "cma").unwrap();

    let mut missing = grants.clone();
    let activity = missing
        .iter_mut()
        .find(|grant| {
            grant
                .as_document()
                .unwrap()
                .get_document("resource")
                .unwrap()
                .get_str("collection")
                .unwrap()
                == "activity"
        })
        .unwrap()
        .as_document_mut()
        .unwrap();
    activity
        .get_array_mut("actions")
        .unwrap()
        .retain(|action| action.as_str() != Some("collStats"));
    assert!(validate_activity_index_inspection(&status(missing), "cma").is_err());

    let mut foreign = grants;
    let agents = foreign
        .iter_mut()
        .find(|grant| {
            grant
                .as_document()
                .unwrap()
                .get_document("resource")
                .unwrap()
                .get_str("collection")
                .unwrap()
                == "agents"
        })
        .unwrap()
        .as_document_mut()
        .unwrap();
    agents
        .get_array_mut("actions")
        .unwrap()
        .push("collStats".into());
    assert!(validate_activity_index_inspection(&status(foreign), "cma").is_err());
}

#[test]
fn ordinary_runtime_role_does_not_require_activity_operation_grants() {
    let runtime = role("cma").get_array("privileges").unwrap().clone();
    validate(&status(runtime.clone()), "cma").unwrap();
    let activity = runtime
        .iter()
        .find(|grant| {
            grant
                .as_document()
                .unwrap()
                .get_document("resource")
                .unwrap()
                .get_str("collection")
                .unwrap()
                == "activity"
        })
        .unwrap()
        .as_document()
        .unwrap();
    assert!(activity
        .get_array("actions")
        .unwrap()
        .iter()
        .all(|action| action.as_str() != Some("collStats")));
    assert!(runtime.iter().all(|grant| {
        grant
            .as_document()
            .unwrap()
            .get_document("resource")
            .unwrap()
            .get_str("collection")
            .unwrap()
            != "agent_manager_migrations"
    }));
}

#[test]
fn activity_operation_roles_require_their_explicit_additional_grants() {
    let inspection = activity_index_inspection_role("cma")
        .get_array("privileges")
        .unwrap()
        .clone();
    assert!(validate_activity_index_inspection(&status(inspection.clone()), "cma").is_ok());
    assert!(validate(&status(inspection), "cma").is_err());

    let job = activity_index_job_role("cma")
        .get_array("privileges")
        .unwrap()
        .clone();
    assert!(validate_activity_index_job(&status(job), "cma").is_ok());
}

#[test]
fn activity_operation_authority_accepts_app_user_drop_index_in_shared_mode() {
    let mut grants = activity_index_job_role("cma")
        .get_array("privileges")
        .unwrap()
        .clone();
    grants.push(
        doc! {
            "resource":{"db":"cma","collection":"activity"},
            "actions":["dropIndex"]
        }
        .into(),
    );
    let status = status(grants);
    assert!(verify_activity_index_authority(
        &status,
        "cma",
        cma_service_auth::mongodb_authority::MongoAuthorityMode::SharedSchema,
        true,
    )
    .is_ok());
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
    let uri = std::env::var("CMA_IM_TEST_MONGODB_URI").unwrap();
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

mod observation_grants_tests {
    use super::*;
    #[test]
    fn observation_grants_are_optional_when_closed_and_required_when_enabled() {
        let mut grants = role("cma").get_array("privileges").unwrap().clone();
        let status = |grants| doc! {"authInfo": {"authenticatedUsers": [{"user": "manager", "db": "cma"}], "authenticatedUserPrivileges": grants}};
        validate_observation_grants(&status(grants.clone()), "cma").unwrap();
        grants.retain(|grant| {
            grant
                .as_document()
                .unwrap()
                .get_document("resource")
                .unwrap()
                .get_str("collection")
                .unwrap()
                != crate::store::agent_observation::COLLECTION
        });
        validate(&status(grants.clone()), "cma").unwrap();
        assert!(validate_observation_grants(&status(grants), "cma").is_err());
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn real_measured_custody_role_can_be_added_and_revoked_with_capture_off() {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use mongodb::{
        options::{ClientOptions, Credential},
        Client,
    };
    use std::{sync::Arc, time::Duration};

    let uri = std::env::var("CMA_IM_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_measured_role_{}", uuid::Uuid::new_v4().simple());
    let db = admin.database(&name);
    db.run_command(required_role(&name)).await.unwrap();
    let password = uuid::Uuid::new_v4().to_string();
    db.run_command(doc! {"createUser": "runtime", "pwd": &password,
    "roles": [{"role": "cma_am_runtime", "db": &name}]})
        .await
        .unwrap();
    db.create_collection(MEASURED_CUSTODY).await.unwrap();
    let privilege = doc! {"resource": {"db": &name, "collection": MEASURED_CUSTODY},
    "actions": MEASURED_CUSTODY_ACTIONS};
    let key = crate::store::AuthEncryptionKey::from_base64(&STANDARD.encode([7_u8; 32])).unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.credential = Some(
        Credential::builder()
            .username("runtime".to_owned())
            .password(password)
            .source(name.clone())
            .build(),
    );
    for (command, widened) in [
        (None, false),
        (Some("grantPrivilegesToRole"), true),
        (Some("revokePrivilegesFromRole"), false),
    ] {
        if let Some(command) = command {
            db.run_command(doc! {command: "cma_am_runtime", "privileges": [privilege.clone()]})
                .await
                .unwrap();
        }
        let runtime = Client::with_options(options.clone()).unwrap();
        let stores = crate::store::MongoStores::open(
            runtime.clone(),
            &name,
            "measured-role-fixture",
            &key,
            Duration::from_secs(86_400),
            Arc::new(crate::store::checkpoint_storage::RetiredCheckpointStorage),
            ("measured-role-fixture", None),
        )
        .await
        .unwrap();
        stores.ping().await.unwrap();
        let rows = runtime
            .database(&name)
            .collection::<Document>(MEASURED_CUSTODY);
        assert_eq!(rows.find_one(doc! {}).await.is_ok(), widened);
        assert!(rows.delete_one(doc! {}).await.is_err());
    }
    assert_eq!(
        db.collection::<Document>(MEASURED_CUSTODY)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    db.drop().await.unwrap();
}
