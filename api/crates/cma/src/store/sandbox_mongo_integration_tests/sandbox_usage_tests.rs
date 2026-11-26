use super::*;
use crate::store::sandbox_mongo_integration_tests::work_lease_deadline;
use layer2_core::sandbox_usage::{timestamp, UsageTotals};
use layer2_core::sandbox_usage_service::SandboxUsageStore;

mod continuation;
mod terminal;

async fn fixture(prefix: &str) -> (Client, String, MongoSandboxStore, Sandbox) {
    let (client, name) = database(prefix).await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut parent = sandbox("usage-sandbox");
    parent.runtime = Some(layer2_core::sandbox::RuntimeBinding {
        runtime_id: "usage-runtime".into(),
        runtime_version: "usage-image".into(),
        bound_at: NOW.into(),
        boot_id: None,
        provision_generation: None,
    });
    store.create_sandbox(parent.clone()).await.unwrap();
    store
        .append_sandbox_event(&parent.id, NOW.into(), SandboxEventKind::Created)
        .await
        .unwrap();
    (client, name, store, parent)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_usage_checkpoint_concurrency_restart_and_summary_are_exact() {
    let (client, name, store, parent) = fixture("usage_concurrent").await;
    let other = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (first, second) = tokio::join!(
        store.checkpoint_usage_one(parent.id.as_str(), MINUTE),
        other.checkpoint_usage_one(parent.id.as_str(), MINUTE),
    );
    first.unwrap();
    second.unwrap();
    let saved = other.usage(&parent.id).await.unwrap().unwrap();
    assert_eq!(saved.totals.uptime_seconds, 60);
    assert_eq!(saved.totals.cpu_millis_seconds, 60_000);
    assert_eq!(saved.revision, 2);
    let summary = store
        .usage_summary(Some(&parent.owner), timestamp(TWO_MINUTES).unwrap())
        .await
        .unwrap();
    assert_eq!(summary.sandbox_count, 1);
    assert_eq!(summary.running_count, 1);
    assert_eq!(summary.totals.uptime_seconds, 120);
    assert_eq!(summary.totals.memory_mib_seconds, 120 * 2_048);
    assert_eq!(
        store
            .usage_summary(
                Some(&SubjectId("other".into())),
                timestamp(TWO_MINUTES).unwrap()
            )
            .await
            .unwrap()
            .sandbox_count,
        0
    );
    let first_page = store
        .usage_events(&parent.id, &PageRequest::new(1, None).unwrap())
        .await
        .unwrap();
    let second_page = store
        .usage_events(&parent.id, &PageRequest::new(1, first_page.next).unwrap())
        .await
        .unwrap();
    assert_eq!(second_page.items[0].quantity.uptime_seconds, 60);
    assert!(second_page.next.is_none());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_usage_aborted_lifecycle_transaction_cannot_leave_billable_usage() {
    let (client, name, store, mut parent) = fixture("usage_abort").await;
    let before = store.usage(&parent.id).await.unwrap().unwrap();
    parent.runtime = None;
    parent.status = SandboxStatus::Paused;
    parent.revision += 1;
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    assert!(crate::store::sandbox_usage::record(
        &store,
        &mut session,
        &parent,
        2,
        MINUTE,
        (1, parent.revision - 1),
        false
    )
    .await
    .is_ok());
    session.abort_transaction().await.unwrap();
    assert_eq!(store.usage(&parent.id).await.unwrap().unwrap(), before);
    assert_eq!(
        store
            .usage_events(&parent.id, &PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert!(store
        .get_sandbox(&parent.id)
        .await
        .unwrap()
        .unwrap()
        .runtime
        .is_some());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_usage_legacy_writer_gap_is_excluded_before_new_lifecycle_append() {
    let (client, name, store, mut parent) = fixture("usage_legacy_gap").await;
    store
        .checkpoint_usage_one(parent.id.as_str(), MINUTE)
        .await
        .unwrap();
    let confirmed = store.usage(&parent.id).await.unwrap().unwrap();
    // Reproduce the old binary's direct aggregate save: its ledger stays behind.
    parent.runtime = None;
    parent.status = SandboxStatus::Paused;
    parent.lifecycle.checkpoints.latest = Some(
        layer2_core::sandbox::SandboxCheckpointRef::seal(
            &layer3_adapter::CheckpointReceipt {
                epoch: 1,
                object_reference: "checkpoints/usage".into(),
                sha256: "b".repeat(64),
                writers: layer3_adapter::WriterState::Held,
            },
            &[],
            NOW,
        )
        .unwrap(),
    );
    parent.credential.delivered = false;
    parent.credential.current = None;
    parent.revision += 1;
    parent.updated_at = TWO_MINUTES.into();
    store
        .sandboxes
        .update_one(
            doc! { "_id": parent.id.as_str() },
            doc! {
                "$set": crate::store::sandboxes::sandbox_replacement_fields(&parent).unwrap()
            },
        )
        .await
        .unwrap();
    store
        .append_sandbox_event(
            &parent.id,
            TWO_MINUTES.into(),
            SandboxEventKind::StatusChanged {
                from: SandboxStatus::Active,
                to: SandboxStatus::Paused,
            },
        )
        .await
        .unwrap();
    let recovered = store.usage(&parent.id).await.unwrap().unwrap();
    assert!(!recovered.complete_lifecycle);
    assert_eq!(recovered.totals, confirmed.totals);
    assert_eq!(
        (recovered.source_seq, recovered.source_revision),
        (2, parent.revision)
    );
    let events = store
        .usage_events(&parent.id, &PageRequest::default())
        .await
        .unwrap();
    let gap = events.items.last().unwrap();
    assert_eq!(
        gap.kind,
        layer2_core::sandbox_usage::UsageEventKind::Baseline
    );
    assert_eq!(gap.quantity, UsageTotals::default());
    assert_eq!(gap.started_at, gap.ended_at);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_usage_legacy_writer_gap_is_excluded_by_checkpoint() {
    let (client, name, store, mut parent) = fixture("usage_legacy_checkpoint").await;
    store
        .checkpoint_usage_one(parent.id.as_str(), MINUTE)
        .await
        .unwrap();
    parent.revision += 1;
    store
        .sandboxes
        .update_one(
            doc! { "_id": parent.id.as_str() },
            doc! {
                "$set": crate::store::sandboxes::sandbox_replacement_fields(&parent).unwrap()
            },
        )
        .await
        .unwrap();
    store
        .checkpoint_usage_one(parent.id.as_str(), TWO_MINUTES)
        .await
        .unwrap();
    let recovered = store.usage(&parent.id).await.unwrap().unwrap();
    assert!(!recovered.complete_lifecycle);
    assert_eq!(recovered.totals.uptime_seconds, 60);
    assert_eq!(
        recovered
            .totals_at(timestamp(TWO_MINUTES).unwrap() + 30)
            .unwrap()
            .uptime_seconds,
        90
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_usage_pause_retirement_and_purge_retain_history() {
    let (client, name, store, mut parent) = fixture("usage_retained").await;
    let mut current = parent.clone();
    current.runtime = None;
    current.status = SandboxStatus::Paused;
    current.lifecycle.checkpoints.latest = Some(
        layer2_core::sandbox::SandboxCheckpointRef::seal(
            &layer3_adapter::CheckpointReceipt {
                epoch: 1,
                object_reference: "checkpoints/usage".into(),
                sha256: "b".repeat(64),
                writers: layer3_adapter::WriterState::Held,
            },
            &[],
            NOW,
        )
        .unwrap(),
    );
    current.credential.delivered = false;
    current.credential.current = None;
    current.revision += 1;
    current.updated_at = MINUTE.into();
    store
        .commit(SandboxTransaction::SaveSandboxAndAppend {
            expected_revision: parent.revision,
            sandbox: Box::new(current.clone()),
            event_at: MINUTE.into(),
            event: SandboxEventKind::StatusChanged {
                from: parent.status,
                to: current.status,
            },
            command_revival: None,
            released_runtime: None,
        })
        .await
        .unwrap();
    let paused = store.usage(&parent.id).await.unwrap().unwrap();
    assert_eq!(
        paused.totals_at(timestamp(TWO_MINUTES).unwrap()).unwrap(),
        UsageTotals {
            uptime_seconds: 60,
            cpu_millis_seconds: 60_000,
            memory_mib_seconds: 60 * 2_048,
            disk_gib_seconds: 0,
        }
    );
    parent = current.clone();
    current.status = SandboxStatus::Retired;
    current.retired_at = Some(TWO_MINUTES.into());
    current.purge_after = Some(TWO_MINUTES.into());
    current.updated_at = TWO_MINUTES.into();
    current.revision += 1;
    store
        .commit(SandboxTransaction::SaveSandboxAndAppend {
            expected_revision: parent.revision,
            sandbox: Box::new(current.clone()),
            event_at: TWO_MINUTES.into(),
            event: SandboxEventKind::StatusChanged {
                from: parent.status,
                to: current.status,
            },
            command_revival: None,
            released_runtime: None,
        })
        .await
        .unwrap();
    store
        .purge_sandbox(&current.id, current.revision, TWO_MINUTES)
        .await
        .unwrap();
    assert!(store.get_sandbox(&current.id).await.unwrap().is_none());
    let retained = store.usage(&current.id).await.unwrap().unwrap();
    assert_eq!(retained.totals, paused.totals);
    assert_eq!(retained.status, "retired");
    assert_eq!(
        store
            .usage_events(&current.id, &PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        3
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_usage_raw_attachment_save_is_metered_atomically() {
    let (client, name, store, mut parent) = fixture("usage_raw").await;
    parent.runtime = None;
    parent.revision += 1;
    store.save_sandbox(1, parent.clone()).await.unwrap();
    let metered = store.usage(&parent.id).await.unwrap().unwrap();
    assert!(!metered.allocation.running);
    assert_eq!(metered.source_revision, parent.revision);
    let before = metered.clone();
    assert!(store.save_sandbox(1, parent.clone()).await.is_err());
    assert_eq!(store.usage(&parent.id).await.unwrap().unwrap(), before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_usage_metadata_revision_then_pause_keeps_the_entire_active_interval() {
    let (client, name, store, mut parent) = fixture("usage_metadata").await;
    // Lease/activity updates do not change allocation, but still advance revision.
    parent.revision += 1;
    parent.updated_at = MINUTE.into();
    parent.last_activity_at = MINUTE.into();
    store.save_sandbox(1, parent.clone()).await.unwrap();
    let saved = store.usage(&parent.id).await.unwrap().unwrap();
    assert!(saved.complete_lifecycle);
    assert_eq!(saved.source_revision, parent.revision);
    assert_eq!(saved.totals.uptime_seconds, 60);

    let expected_revision = parent.revision;
    parent.begin_pause(TWO_MINUTES.into(), None).unwrap();
    store
        .commit(SandboxTransaction::SaveSandboxAndAppend {
            command_revival: None,
            released_runtime: None,
            expected_revision,
            sandbox: Box::new(parent.clone()),
            event_at: TWO_MINUTES.into(),
            event: SandboxEventKind::StatusChanged {
                from: SandboxStatus::Active,
                to: SandboxStatus::Pausing,
            },
        })
        .await
        .unwrap();
    let paused = store.usage(&parent.id).await.unwrap().unwrap();
    assert!(paused.complete_lifecycle);
    assert_eq!(paused.totals.uptime_seconds, 120);
    assert_eq!(paused.totals.cpu_millis_seconds, 120_000);
    assert_eq!(paused.totals.memory_mib_seconds, 120 * 2_048);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_usage_checkpoint_and_multi_event_phase_preserve_watermarks() {
    use super::sandbox_lifecycle_tests::{
        checkpoint_for_agents, checkpoint_mutation, claimed_pause_fixture, pause_agents_mutation,
    };
    use layer2_core::sandbox::store::SandboxLifecycleExecutionStore;
    use layer2_core::sandbox_usage::UsageEventKind;
    let (client, name) = database("usage_lifecycle_phase").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (claimed, agents) = claimed_pause_fixture(&store, "usage_phase").await;
    assert!(
        store
            .usage(&claimed.id)
            .await
            .unwrap()
            .unwrap()
            .complete_lifecycle
    );
    store
        .publish_sandbox_checkpoint(checkpoint_mutation(
            &claimed,
            &agents,
            checkpoint_for_agents(&agents, 1),
        ))
        .await
        .unwrap();
    let claimed = store
        .claim_sandbox_work(&claimed.id, "worker", MINUTE.into(), work_lease_deadline())
        .await
        .unwrap()
        .unwrap();
    store
        .commit_sandbox_lifecycle(pause_agents_mutation(&claimed, &agents))
        .await
        .unwrap();
    store
        .checkpoint_usage_one(claimed.id.as_str(), TWO_MINUTES)
        .await
        .unwrap();
    let current = store.get_sandbox(&claimed.id).await.unwrap().unwrap();
    let usage = store.usage(&claimed.id).await.unwrap().unwrap();
    assert!(usage.complete_lifecycle);
    assert_eq!(usage.source_revision, current.revision);
    assert_eq!(usage.totals.uptime_seconds, 120);
    assert_eq!(usage.totals.cpu_millis_seconds, 120_000);
    assert_eq!(usage.totals.memory_mib_seconds, 120 * 2_048);
    let events = store
        .usage_events(&claimed.id, &PageRequest::default())
        .await
        .unwrap();
    assert!(events
        .items
        .iter()
        .all(|event| event.kind != UsageEventKind::Baseline));
    assert_eq!(
        events
            .items
            .iter()
            .map(|event| event.quantity.uptime_seconds)
            .sum::<u64>(),
        120
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_usage_duplicate_metadata_save_preserves_store_error_and_ledger() {
    let (client, name, store, mut parent) = fixture("usage_duplicate").await;
    let before = store.usage(&parent.id).await.unwrap().unwrap();
    let mut other = sandbox("usage-other");
    other.name = "occupied-name".into();
    store.create_sandbox(other.clone()).await.unwrap();
    parent.name = other.name;
    parent.revision += 1;
    parent.updated_at = MINUTE.into();
    assert_eq!(
        store.save_sandbox(1, parent.clone()).await,
        Err(SandboxStoreError::Duplicate)
    );
    assert_eq!(store.usage(&parent.id).await.unwrap().unwrap(), before);
    assert_eq!(
        store
            .get_sandbox(&parent.id)
            .await
            .unwrap()
            .unwrap()
            .revision,
        1
    );
    client.database(&name).drop().await.unwrap();
}
