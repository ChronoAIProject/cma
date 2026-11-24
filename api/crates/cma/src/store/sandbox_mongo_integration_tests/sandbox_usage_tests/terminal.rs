use super::*;
use futures_util::TryStreamExt;
use layer2_core::sandbox::{SandboxHome, TypedFailure};
use layer2_core::sandbox_usage::{Allocation, UsageEventKind};
use layer3_adapter::{HomeVolumePolicy, HomeVolumeReceipt};

const THREE_MINUTES: &str = "2026-09-05T00:03:00Z";

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn terminal_zero_compute_cannot_authorize_purging_a_retained_runtime() {
    let (client, name, store, mut parent) = fixture("usage_terminal_purge").await;
    let expected = parent.revision;
    parent.status = SandboxStatus::Retired;
    parent.retired_at = Some(MINUTE.into());
    parent.purge_after = Some(MINUTE.into());
    parent.updated_at = MINUTE.into();
    parent.revision += 1;
    store.save_sandbox(expected, parent.clone()).await.unwrap();
    assert_eq!(
        store.usage(&parent.id).await.unwrap().unwrap().allocation,
        Allocation::default()
    );
    assert!(matches!(
        store
            .purge_sandbox(&parent.id, parent.revision, MINUTE)
            .await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(
        store
            .get_sandbox(&parent.id)
            .await
            .unwrap()
            .unwrap()
            .runtime,
        parent.runtime
    );
    client.database(&name).drop().await.unwrap();
}

async fn retained_fixture(prefix: &str) -> (Client, String, MongoSandboxStore, Sandbox) {
    let (client, name, store, mut parent) = fixture(prefix).await;
    let mut home = SandboxHome::reserve(
        &parent.id,
        HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "test-home".into(),
            capacity_gib: 10,
            template_name: "test-seed".into(),
            template_generation: "test-generation".into(),
        },
    )
    .unwrap();
    home.bind(HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "test-home-volume".into(),
        storage_generation: "test-home-generation".into(),
    })
    .unwrap();
    parent.lifecycle.home = Some(Box::new(home));
    let expected = parent.revision;
    parent.revision += 1;
    store.save_sandbox(expected, parent.clone()).await.unwrap();
    (client, name, store, parent)
}

fn fail(parent: &mut Sandbox) {
    parent
        .transition(
            SandboxStatus::Failed,
            MINUTE.into(),
            Some(TypedFailure {
                class: "terminal_validation".into(),
                detail: "synthetic setup refusal".into(),
            }),
        )
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn terminal_failure_stops_compute_retains_disk_and_does_not_release_quota() {
    let (client, name, store, mut parent) = retained_fixture("usage_terminal").await;
    let runtime = parent.runtime.clone();
    let expected = parent.revision;
    fail(&mut parent);
    store
        .commit(SandboxTransaction::SaveSandboxAndAppend {
            expected_revision: expected,
            sandbox: Box::new(parent.clone()),
            event_at: MINUTE.into(),
            event: SandboxEventKind::WorkStepFailed {
                step: parent.work.step,
                failure: parent.failure.clone().unwrap(),
            },
            command_revival: None,
            released_runtime: None,
        })
        .await
        .unwrap();
    let failed = store.usage(&parent.id).await.unwrap().unwrap();
    assert_eq!(failed.totals.uptime_seconds, 60);
    assert_eq!(failed.totals.cpu_millis_seconds, 60_000);
    assert_eq!(failed.totals.memory_mib_seconds, 122_880);
    assert_eq!(
        failed.allocation,
        Allocation {
            disk_gib: 10,
            ..Default::default()
        }
    );
    let other = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (one, two) = tokio::join!(
        store.checkpoint_usage_one(parent.id.as_str(), TWO_MINUTES),
        other.checkpoint_usage_one(parent.id.as_str(), TWO_MINUTES),
    );
    one.unwrap();
    two.unwrap();
    let checkpoint = other.usage(&parent.id).await.unwrap().unwrap();
    assert_eq!(checkpoint.revision, failed.revision + 1);
    let summary = store
        .usage_summary(Some(&parent.owner), timestamp(THREE_MINUTES).unwrap())
        .await
        .unwrap();
    assert_eq!(summary.running_count, 0);
    assert_eq!(
        summary.totals,
        UsageTotals {
            disk_gib_seconds: 1_800,
            ..failed.totals.clone()
        }
    );
    assert_eq!(
        store
            .get_sandbox(&parent.id)
            .await
            .unwrap()
            .unwrap()
            .runtime,
        runtime
    );
    assert_eq!(
        store
            .sandboxes
            .count_documents(doc! {"owner":parent.owner.as_str(), "retired_at":null})
            .await
            .unwrap(),
        1
    );

    let expected = parent.revision;
    parent.begin_retirement(TWO_MINUTES.into()).unwrap();
    store.save_sandbox(expected, parent.clone()).await.unwrap();
    store
        .checkpoint_usage_one(parent.id.as_str(), THREE_MINUTES)
        .await
        .unwrap();
    let retiring = store.usage(&parent.id).await.unwrap().unwrap();
    assert!(!retiring.allocation.running);
    assert_eq!(retiring.totals.uptime_seconds, failed.totals.uptime_seconds);
    assert_eq!(retiring.totals.disk_gib_seconds, 1_800);
    let events = store
        .usage_events(&parent.id, &PageRequest::default())
        .await
        .unwrap();
    let closing = events
        .items
        .iter()
        .find(|event| event.kind == UsageEventKind::Terminated)
        .unwrap();
    assert_eq!(closing.ended_at, timestamp(MINUTE).unwrap());
    assert!(closing.allocation.running);
    assert!(!closing.next_allocation.running);
    assert!(events
        .items
        .iter()
        .filter(|event| event.started_at >= timestamp(MINUTE).unwrap())
        .all(|event| event.quantity.uptime_seconds == 0
            && event.quantity.cpu_millis_seconds == 0
            && event.quantity.memory_mib_seconds == 0));
    let expected = parent.revision;
    parent.runtime = None;
    parent.lifecycle.home.as_mut().unwrap().released = true;
    parent.work = SandboxWorkState::new(SandboxWorkStep::None, THREE_MINUTES.into());
    parent
        .transition(SandboxStatus::Retired, THREE_MINUTES.into(), None)
        .unwrap();
    store.save_sandbox(expected, parent.clone()).await.unwrap();
    let retired = store.usage(&parent.id).await.unwrap().unwrap();
    assert_eq!(retired.allocation, Allocation::default());
    assert_eq!(retired.totals, retiring.totals);
    assert_eq!(
        retired
            .totals_at(timestamp(THREE_MINUTES).unwrap() + 60)
            .unwrap(),
        retired.totals
    );
    assert_eq!(
        store
            .sandboxes
            .count_documents(doc! {"owner":parent.owner.as_str(), "retired_at":null})
            .await
            .unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn existing_failed_usage_stops_projection_and_repairs_only_the_current_allocation() {
    let (client, name, store, mut parent) = retained_fixture("usage_terminal_legacy").await;
    store
        .checkpoint_usage_one(parent.id.as_str(), MINUTE)
        .await
        .unwrap();
    let mut legacy = store.usage(&parent.id).await.unwrap().unwrap();
    fail(&mut parent);
    legacy.status = "failed".into();
    legacy.source_revision = parent.revision;
    store
        .sandboxes
        .update_one(
            doc! {"_id":parent.id.as_str()},
            doc! {"$set":crate::store::sandboxes::sandbox_replacement_fields(&parent).unwrap()},
        )
        .await
        .unwrap();
    store
        .sandbox_usage
        .replace_one(
            doc! {"_id":parent.id.as_str()},
            crate::store::sandbox_usage::state_document(&legacy).unwrap(),
        )
        .await
        .unwrap();
    let history: Vec<Document> = store
        .sandbox_usage_events
        .find(doc! {"sandbox_id":parent.id.as_str()})
        .sort(doc! {"revision":1})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let restored = MongoSandboxStore::open(&client, &name).await.unwrap();
    let projected = restored
        .usage(&parent.id)
        .await
        .unwrap()
        .unwrap()
        .totals_at(timestamp(TWO_MINUTES).unwrap())
        .unwrap();
    assert_eq!(
        projected,
        UsageTotals {
            disk_gib_seconds: 1_200,
            ..legacy.totals.clone()
        }
    );
    let summary = restored
        .usage_summary(Some(&parent.owner), timestamp(TWO_MINUTES).unwrap())
        .await
        .unwrap();
    assert_eq!(summary.running_count, 0);
    assert_eq!(summary.totals, projected);
    assert_eq!(
        restored
            .usage_summary(
                Some(&SubjectId("unrelated-owner".into())),
                timestamp(TWO_MINUTES).unwrap()
            )
            .await
            .unwrap()
            .sandbox_count,
        0
    );
    restored
        .checkpoint_usage_one(parent.id.as_str(), TWO_MINUTES)
        .await
        .unwrap();
    restored
        .checkpoint_usage_one(parent.id.as_str(), TWO_MINUTES)
        .await
        .unwrap();
    let settled = restored.usage(&parent.id).await.unwrap().unwrap();
    assert_eq!(settled.totals, projected);
    assert!(!settled.allocation.running);
    assert_eq!(settled.source_revision, legacy.source_revision);
    assert_eq!(settled.source_seq, legacy.source_seq);
    assert_eq!(settled.revision, legacy.revision + 1);
    let after: Vec<Document> = store
        .sandbox_usage_events
        .find(doc! {"sandbox_id":parent.id.as_str()})
        .sort(doc! {"revision":1})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert_eq!(&after[..history.len()], history);
    assert_eq!(after.len(), history.len() + 1);
    let closing = crate::store::sandbox_usage::read_event(after.last().unwrap()).unwrap();
    assert_eq!(
        closing.quantity,
        UsageTotals {
            disk_gib_seconds: 600,
            ..Default::default()
        }
    );
    assert_eq!(
        closing.quantity,
        closing
            .allocation
            .usage(closing.ended_at - closing.started_at)
            .unwrap()
    );
    client.database(&name).drop().await.unwrap();
}
