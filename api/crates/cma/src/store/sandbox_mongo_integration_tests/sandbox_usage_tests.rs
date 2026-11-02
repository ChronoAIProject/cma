use super::*;
use layer2_core::sandbox_usage::{timestamp, UsageTotals};
use layer2_core::sandbox_usage_service::SandboxUsageStore;

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
    assert!(
        crate::store::sandbox_usage::record(&store, &mut session, &parent, 2, MINUTE, false)
            .await
            .is_ok()
    );
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
