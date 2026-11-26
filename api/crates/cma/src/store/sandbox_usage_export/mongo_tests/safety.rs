use super::*;
use crate::store::sandbox_mongo::start_transaction;
use layer2_core::sandbox::store::SandboxWorkerStore;
use layer2_core::sandbox::{SandboxHome, SandboxStatus};
use layer3_adapter::{HomeVolumePolicy, HomeVolumeReceipt};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn terminal_purge_retains_capture_and_late_projection_does_not_recreate_owner() {
    let fixture = Fixture::new(true).await;
    let mut parent = fixture.parent("deleted-owner").await;
    parent.revision += 1;
    parent.updated_at = MINUTE.into();
    parent.status = SandboxStatus::Retired;
    parent.runtime = None;
    parent.credential.delivered = false;
    parent.credential.current = None;
    parent.work = layer2_core::sandbox::SandboxWorkState::new(
        layer2_core::sandbox::SandboxWorkStep::Purge,
        MINUTE.into(),
    );
    parent.retired_at = Some(MINUTE.into());
    parent.purge_after = Some(MINUTE.into());
    fixture.store.save_sandbox(1, parent.clone()).await.unwrap();
    fixture
        .store
        .purge_sandbox(&parent.id, parent.revision, MINUTE)
        .await
        .unwrap();
    assert_eq!(fixture.count("sandboxes").await, 0);
    let capture = fixture.capture();
    assert_eq!(fixture.count(CAPTURES).await, 2);
    for revision in 1..=2 {
        assert!(capture
            .project_one(&source_id(&parent, revision), now())
            .await
            .unwrap());
    }
    assert_eq!(fixture.count(EVIDENCE).await, 1);
    assert_eq!(fixture.count(OUTBOX).await, 3);
    assert_eq!(fixture.count("auth_subjects").await, 0);
    assert_eq!(fixture.count("sandboxes").await, 0);
    fixture.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn producer_identity_change_refuses_without_advancing_source_or_capture() {
    let fixture = Fixture::new(true).await;
    let parent = fixture.parent("stable-owner").await;
    for change_namespace in [true, false] {
        let mut changed = config();
        if change_namespace {
            changed.namespace.push_str("-changed");
        } else {
            changed.owner_issuer.push_str("/changed");
        }
        let store = fixture
            .store
            .clone()
            .with_usage_capture(Some(changed))
            .await
            .unwrap();
        assert!(store
            .checkpoint_usage_one(parent.id.as_str(), MINUTE)
            .await
            .is_err());
        assert_eq!(fixture.count(CAPTURES).await, 1);
        assert_eq!(fixture.count(STREAMS).await, 1);
        assert_eq!(fixture.count(crate::store::sandbox_usage::EVENTS).await, 1);
    }
    fixture.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn deletion_between_source_read_and_capture_cannot_commit_or_resurrect_parent() {
    let fixture = Fixture::new(true).await;
    let parent = fixture.parent("erased-owner").await;
    let mut session = fixture.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    fixture
        .db
        .collection::<Document>("sandboxes")
        .find_one(doc! {"_id":parent.id.as_str()})
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    fixture
        .db
        .collection::<Document>("sandboxes")
        .delete_one(doc! {"_id":parent.id.as_str()})
        .await
        .unwrap();
    assert!(crate::store::sandbox_usage::record(
        &fixture.store,
        &mut session,
        &parent,
        1,
        MINUTE,
        (1, 1),
        true
    )
    .await
    .is_err());
    let _ = session.abort_transaction().await;
    assert_eq!(fixture.count(CAPTURES).await, 1);
    assert_eq!(fixture.count(crate::store::sandbox_usage::EVENTS).await, 1);
    assert_eq!(fixture.count("sandboxes").await, 0);
    fixture.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn outbox_failure_rolls_back_facts_sequence_and_projection_cursor() {
    let fixture = Fixture::new(true).await;
    let parent = fixture.parent("outside-rollout").await;
    let capture = fixture.capture();
    capture
        .project_one(&source_id(&parent, 1), now())
        .await
        .unwrap();
    fixture
        .store
        .checkpoint_usage_one(parent.id.as_str(), MINUTE)
        .await
        .unwrap();
    let id = source_id(&parent, 2);
    let before = fixture
        .db
        .collection::<Document>(CAPTURES)
        .find_one(doc! {"_id":&id})
        .await
        .unwrap()
        .unwrap();
    fixture.db.run_command(doc! {"collMod":OUTBOX,"validator":{"reject_new_event":true},"validationLevel":"strict"}).await.unwrap();
    assert!(capture.project_one(&id, now()).await.is_err());
    assert_eq!(fixture.count(EVIDENCE).await, 0);
    assert_eq!(fixture.count(OUTBOX).await, 2);
    assert_eq!(
        before,
        fixture
            .db
            .collection::<Document>(CAPTURES)
            .find_one(doc! {"_id":&id})
            .await
            .unwrap()
            .unwrap()
    );
    fixture
        .db
        .run_command(doc! {"collMod":OUTBOX,"validator":{}})
        .await
        .unwrap();
    assert!(capture.project_one(&id, now()).await.unwrap());
    assert_eq!(fixture.count(EVIDENCE).await, 1);
    assert_eq!(fixture.count(OUTBOX).await, 3);
    fixture.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn terminal_compute_stops_while_original_home_generation_and_disk_remain() {
    for runtime_generation in [Some(1), None] {
        terminal_retained_home(runtime_generation).await;
    }
}

async fn terminal_retained_home(runtime_generation: Option<u64>) {
    let fixture = Fixture::new(true).await;
    let mut parent = fixture
        .parent_with_generation("retained-home-owner", runtime_generation)
        .await;
    let (failed_at, checkpoint_at) = if runtime_generation.is_some() {
        (MINUTE, TWO_MINUTES)
    } else {
        (NOW, MINUTE)
    };
    let mut home = SandboxHome::reserve(
        &parent.id,
        HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "fixture-home".into(),
            capacity_gib: 10,
            template_name: "fixture-seed".into(),
            template_generation: "fixture-generation".into(),
        },
    )
    .unwrap();
    home.bind(HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "fixture-volume".into(),
        storage_generation: "opaque-home-generation".into(),
    })
    .unwrap();
    parent.lifecycle.home = Some(Box::new(home));
    parent.revision = 2;
    fixture.store.save_sandbox(1, parent.clone()).await.unwrap();
    parent.revision = 3;
    parent.status = SandboxStatus::Failed;
    parent.failure = Some(layer2_core::sandbox::TypedFailure {
        class: "terminal_validation".into(),
        detail: "synthetic setup refusal".into(),
    });
    parent.updated_at = failed_at.into();
    fixture.store.save_sandbox(2, parent.clone()).await.unwrap();
    fixture
        .store
        .checkpoint_usage_one(parent.id.as_str(), checkpoint_at)
        .await
        .unwrap();
    let capture = fixture.capture();
    for revision in 1..=4 {
        assert!(capture
            .project_one(&source_id(&parent, revision), now())
            .await
            .unwrap());
    }
    let row = fixture
        .db
        .collection::<Document>(OUTBOX)
        .find_one(doc! {"source_id":source_id(&parent, 4)})
        .await
        .unwrap()
        .unwrap();
    let value: serde_json::Value =
        serde_json::from_slice(row.get_binary_generic("payload").unwrap()).unwrap();
    let fact = &value["facts"][0];
    assert_eq!(fact["quantities"]["sandbox_cpu_millis_seconds"], "0");
    assert_eq!(fact["quantities"]["sandbox_memory_mib_seconds"], "0");
    assert_eq!(fact["quantities"]["sandbox_disk_gib_seconds"], "600");
    assert_eq!(fact["audit_time"]["compute_uptime_ms"], "0");
    assert_eq!(fact["audit_time"]["retained_home_ms"], "60000");
    assert_eq!(
        fact["resource"]["runtime_id"],
        serde_json::json!(runtime_generation.map(|_| "runtime-fixture"))
    );
    assert_eq!(
        fact["resource"]["runtime_generation"],
        serde_json::json!(runtime_generation.map(|value| value.to_string()))
    );
    assert_eq!(
        fact["resource"]["home_generation"],
        "opaque-home-generation"
    );
    let retained = fixture
        .db
        .collection::<Document>(CAPTURES)
        .find_one(doc! {"_id":source_id(&parent, 4)})
        .await
        .unwrap()
        .unwrap();
    let source: records::Captured = records::decode(&retained).unwrap();
    assert_eq!(
        source.resource.runtime_id.as_deref(),
        Some("runtime-fixture")
    );
    assert_eq!(source.resource.runtime_generation, runtime_generation);
    assert_eq!(
        value["source_digest"],
        records::hash(&serde_json::to_vec(&source).unwrap())
    );
    assert_eq!(
        value["source_digest"],
        retained.get_str("source_digest").unwrap()
    );
    assert!(!capture
        .project_one(&source_id(&parent, 4), now() + 1)
        .await
        .unwrap());
    assert_eq!(
        Some(row),
        fixture
            .db
            .collection::<Document>(OUTBOX)
            .find_one(doc! {"source_id":source_id(&parent, 4)})
            .await
            .unwrap()
    );
    assert_eq!(fixture.wire_events().await.len(), 5);
    super::delivery_contract::recover_actual_events(&fixture).await;
    fixture.db.drop().await.unwrap();
}
