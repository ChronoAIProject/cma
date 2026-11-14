use super::*;
use layer2_core::sandbox::store::{
    runtime_migration_now_ms, RuntimeMigrationAdmissionBasis, RuntimeMigrationClearance,
    SandboxCustomRuntimeMigrationAdmission,
};
use layer2_core::sandbox::{
    RuntimeMigrationIdentity, SandboxCustomRuntimeMigration, SandboxCustomRuntimeOrigin,
};
use mongodb::bson::Document;

fn image(digit: char) -> String {
    format!(
        "registry.example/runtime@sha256:{}",
        digit.to_string().repeat(64)
    )
}

fn paused(id: &str) -> Sandbox {
    let mut value = active_sandbox(id);
    value.lifecycle.home = Some(Box::new(
        crate::store::sandbox_unpublished_recovery::mongo_integration_tests::home(&value.id),
    ));
    value.profile.content.runtime_image = Some(image('a'));
    value.profile.digest = layer2_core::profiles::content_digest(&value.profile.content).unwrap();
    let binding = value.runtime.as_mut().unwrap();
    binding.runtime_id = "agent-sandbox:member:sandbox-uid".into();
    binding.runtime_version = image('a');
    binding.boot_id = Some("b".repeat(32));
    value.begin_pause(IDLE_AT.into(), None).unwrap();
    let step = |value: &mut Sandbox, apply: fn(&mut Sandbox, u64)| {
        let fence = value
            .work
            .claim("migration-fixture".into(), IDLE_AT, LATER.into())
            .unwrap();
        apply(value, fence);
    };
    step(&mut value, |v, f| {
        v.complete_lifecycle_persistent_pause("migration-fixture", f, IDLE_AT.into())
            .unwrap();
    });
    step(&mut value, |v, f| {
        v.complete_lifecycle_agent_pause("migration-fixture", f, IDLE_AT.into())
            .unwrap();
    });
    step(&mut value, |v, f| {
        v.complete_lifecycle_runtime_suspension("migration-fixture", f, IDLE_AT.into())
            .unwrap();
    });
    step(&mut value, |v, f| {
        v.publish_lifecycle_paused("migration-fixture", f, IDLE_AT.into())
            .unwrap();
    });
    value.revision = 1;
    value.validate().unwrap();
    value
}

fn admission(value: &Sandbox) -> SandboxCustomRuntimeMigrationAdmission {
    let now = runtime_migration_now_ms().unwrap();
    let migration = SandboxCustomRuntimeMigration {
        request_key: "first-migration".into(),
        expected_revision: value.revision,
        expected_image: image('a'),
        target_image: image('b'),
        origin: SandboxCustomRuntimeOrigin::capture(value).unwrap(),
    };
    SandboxCustomRuntimeMigrationAdmission {
        migration,
        expected_current_revision: value.revision,
        restart_failed: false,
        basis: RuntimeMigrationAdmissionBasis {
            authority_id: "approved-migration".into(),
            policy_id: "qualified-fixture".into(),
            request_key: "first-migration".into(),
            identity: RuntimeMigrationIdentity::capture(value).unwrap(),
            authority_issued_at_ms: now,
            authority_expires_at_ms: now + 30_000,
            approved_target: image('b'),
            source_runtime: value.retained_runtime_binding().cloned(),
            clearance: RuntimeMigrationClearance::Paused,
            admitted_at: layer2_core::now_rfc3339(),
            previous_request_keys: Vec::new(),
            earlier_admissions: Vec::new(),
        },
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn custom_migration_mongo_is_atomic_and_retains_writer_floor_and_exact_replay() {
    let (client, name, mut store) = database().await;
    store.runtime_migration_enabled = true;
    let original = paused("sbx_custom_atomic");
    store.create_sandbox(original.clone()).await.unwrap();
    let request = admission(&original);
    let mut expired = request.clone();
    expired.basis.authority_expires_at_ms = expired.basis.authority_issued_at_ms - 1;
    assert!(store.admit_custom_runtime_migration(expired).await.is_err());
    assert_eq!(
        store.get_sandbox(&original.id).await.unwrap(),
        Some(original.clone())
    );
    let admitted = store
        .admit_custom_runtime_migration(request.clone())
        .await
        .unwrap();
    assert_eq!(admitted.revision, original.revision + 1);
    assert_eq!(admitted.status, SandboxStatus::Rotating);
    assert_eq!(admitted.profile, original.profile);
    assert_eq!(admitted.lifecycle.home, original.lifecycle.home);
    let row = store
        .sandboxes
        .find_one(doc! {"_id":original.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get_i64("document_schema_version").unwrap(), 5);
    // A running old reader/CAS writer cannot overwrite a v5 aggregate.
    let old = store
        .sandboxes
        .update_one(
            doc! {"_id":original.id.as_str(),"document_schema_version":{"$in":[1_i64,3_i64,4_i64]}},
            doc! {"$unset":{"record.lifecycle.custom_runtime":""}},
        )
        .await
        .unwrap();
    assert_eq!(old.matched_count, 0);
    let floors = client
        .database(&name)
        .collection::<Document>("sandbox_credentials");
    assert!(floors
        .find_one(doc! {"_id":"runtime-migration-writer-floor-v1","document_schema_version":3_i64})
        .await
        .unwrap()
        .is_some());
    // Previous startup floor rejects an extra v3 sentinel even after all v5
    // aggregate rows are removed. Compatible readers accept it with writes off.
    assert!(floors
        .find_one(
            doc! {"document_schema_version":3_i64,"_id":{"$ne":"ordinary-work-writer-floor-v1"}}
        )
        .await
        .unwrap()
        .is_some());
    store.runtime_migration_enabled = false;
    assert_eq!(
        store
            .admit_custom_runtime_migration(request.clone())
            .await
            .unwrap(),
        admitted
    );
    let mut changed = request;
    changed.basis.approved_target = image('c');
    assert!(store.admit_custom_runtime_migration(changed).await.is_err());
    let mut stripped = admitted.clone();
    stripped.revision += 1;
    stripped.lifecycle.custom_runtime = None;
    assert!(store
        .save_sandbox(admitted.revision, stripped)
        .await
        .is_err());
    assert_eq!(
        store
            .sandbox_events(&original.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    MongoSandboxStore::open(&client, &name).await.unwrap();
    store
        .sandboxes
        .delete_one(doc! {"_id":original.id.as_str()})
        .await
        .unwrap();
    crate::store::work_schema::floor::verify(&client.database(&name))
        .await
        .unwrap();
    assert!(floors
        .find_one(doc! {"_id":"runtime-migration-writer-floor-v1"})
        .await
        .unwrap()
        .is_some());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn custom_migration_mongo_conflicts_with_parent_change_after_snapshot() {
    let (client, name, mut store) = database().await;
    store.runtime_migration_enabled = true;
    let original = paused("sbx_custom_conflict");
    store.create_sandbox(original.clone()).await.unwrap();
    let request = admission(&original);
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    store
        .sandboxes
        .find_one(doc! {"_id":original.id.as_str()})
        .session(&mut session)
        .await
        .unwrap();
    store
        .set_sandbox_always_on(original.id.clone(), original.revision, true)
        .await
        .unwrap();
    let result = transaction::apply(
        &store,
        &MaintenanceRequest::Custom(Box::new(request)),
        &mut session,
    )
    .await;
    assert!(matches!(result, Err(ApplyError::Driver(error)) if is_transient(&error)));
    let _ = session.abort_transaction().await;
    let actual = store.get_sandbox(&original.id).await.unwrap().unwrap();
    assert_eq!(actual.status, SandboxStatus::Paused);
    assert!(actual.lifecycle.custom_runtime.is_none());
    assert!(client
        .database(&name)
        .collection::<Document>("sandbox_credentials")
        .find_one(doc! {"_id":"runtime-migration-writer-floor-v1"})
        .await
        .unwrap()
        .is_none());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn custom_migration_mongo_unknown_commit_reconciles_one_admission() {
    let uri =
        std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").expect("local failpoint replica set");
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let app_name = format!("cma-custom-migration-{suffix}");
    let name = format!("cma_custom_migration_{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    let client = Client::with_options(options).unwrap();
    let mut store = MongoSandboxStore::open(&client, &name).await.unwrap();
    store.runtime_migration_enabled = true;
    let original = paused("sbx_custom_unknown");
    store.create_sandbox(original.clone()).await.unwrap();
    let request = admission(&original);
    admin.database("admin").run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":5_i32},"data":{
        "failCommands":["commitTransaction"],"appName":&app_name,"writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected ambiguous custom migration commit"}
    }}).await.unwrap();
    let result = store.admit_custom_runtime_migration(request.clone()).await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    let admitted = result.unwrap();
    assert_eq!(admitted.revision, original.revision + 1);
    store.runtime_migration_enabled = false;
    assert_eq!(
        store.admit_custom_runtime_migration(request).await.unwrap(),
        admitted
    );
    assert_eq!(
        store
            .sandbox_events(&original.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    client.database(&name).drop().await.unwrap();
}
