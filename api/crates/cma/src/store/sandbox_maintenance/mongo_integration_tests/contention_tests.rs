use super::*;
use crate::store::settings::MongoPlatformSettingsStore;
use layer2_core::{PlatformSettings, PlatformSettingsStore, StoreError, StoredPlatformSettings};
use mongodb::{event::command::CommandEvent, event::EventHandler};
use std::{collections::BTreeSet, sync::Mutex, time::Duration};

#[derive(Default)]
struct Counts {
    attempts: usize,
    conflicts: usize,
    settings_write_conflicts: usize,
    aborts: usize,
    exhausted_windows: usize,
    paused: BTreeSet<String>,
    cutover_fence: Option<i64>,
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn two_replicas_admit_32_pauses_under_settings_contention_without_stale_policy() {
    for change_policy in [false, true] {
        let (admin, name, setup) = database().await;
        let (settings, policy) = policy_tests::enable(&admin, &name).await;
        let counts = Arc::new(Mutex::new(Counts::default()));
        let settings_write_failed = Arc::new(tokio::sync::Notify::new());
        let mut replicas = Vec::new();
        for _ in 0..2 {
            let mut options =
                ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
                    .await
                    .unwrap();
            let observed = counts.clone();
            let settings_write_failed = settings_write_failed.clone();
            options.command_event_handler = Some(EventHandler::callback(move |event| {
                let mut counts = observed.lock().unwrap();
                match event {
                    CommandEvent::Started(event) => {
                        if matches!(event.command.get_bool("startTransaction"), Ok(true)) {
                            counts.attempts += 1;
                        }
                        if event.command_name == "abortTransaction" {
                            counts.aborts += 1;
                        }
                    }
                    CommandEvent::Failed(event)
                        if event.command_name != "abortTransaction"
                            && is_transient(&event.failure) =>
                    {
                        counts.conflicts += 1;
                        // Pause admission uses update, while the settings CAS
                        // uses findAndModify. Count only a failed body write.
                        if event.command_name == "findAndModify" {
                            counts.settings_write_conflicts += 1;
                            settings_write_failed.notify_one();
                        }
                    }
                    CommandEvent::Succeeded(event) if event.command_name == "findAndModify" => {
                        if let Ok(value) = event.reply.get_document("value") {
                            if value
                                .get_document("settings")
                                .ok()
                                .and_then(|settings| settings.get_document("inactivity").ok())
                                .is_some_and(|policy| {
                                    policy.get_bool("auto_pause_enabled").ok() != Some(true)
                                })
                            {
                                counts.cutover_fence =
                                    Some(value.get_i64("inactivity_admission_fence").unwrap_or(0));
                            }
                        }
                    }
                    _ => {}
                }
            }));
            let client = Client::with_options(options).unwrap();
            replicas.push(Arc::new(
                MongoSandboxStore::open(&client, &name).await.unwrap(),
            ));
        }
        let mut ids = Vec::new();
        for i in 0..32 {
            ids.push(
                fixture(&setup, &format!("sbx_contention_{i:02}"))
                    .await
                    .0
                    .id,
            );
        }
        // Hold the actual singleton write, ensuring both replicas see real
        // transaction conflicts before release or a concurrent settings change.
        let mut holder = admin.start_session().await.unwrap();
        start_transaction(&mut holder).await.unwrap();
        setup
            .budget_settings
            .update_one(
                doc! {"_id":"platform"},
                doc! {"$inc":{"inactivity_admission_fence":1_i64}},
            )
            .session(&mut holder)
            .await
            .unwrap();
        let barrier = Arc::new(tokio::sync::Barrier::new(33));
        let cutover = Arc::new(tokio::sync::Notify::new());
        let deadline_anchor = Arc::new(Mutex::new(None));
        let mut tasks = Vec::new();
        for (i, id) in ids.iter().enumerate() {
            let store = replicas[i % 2].clone();
            let request = SandboxInactivityPauseRequest {
                auto_pause_enabled: true,
                expected_policy_revision: Some(policy.revision),
                ..pause(id)
            };
            let barrier = barrier.clone();
            // Reserve one stale-policy request to test after the cutover even
            // if all of the racing requests finish before the policy changes.
            let after_cutover = (change_policy && i + 1 == ids.len()).then(|| cutover.clone());
            let deadline_anchor = deadline_anchor.clone();
            let counts = counts.clone();
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                if let Some(cutover) = after_cutover {
                    cutover.notified().await;
                }
                admit_cohort_candidate(&store, request, deadline_anchor, &counts).await
            }));
        }
        barrier.wait().await;
        let contenders = ids.len() - usize::from(change_policy);
        tokio::time::timeout(Duration::from_secs(5), async {
            while counts.lock().unwrap().conflicts < contenders {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        // Use the production settings adapter on one independently monitored
        // replica. Its atomic returned fence is the exact serialization cutover.
        let policy_task = if change_policy {
            let settings = MongoPlatformSettingsStore::open(&replicas[0].client, &name)
                .await
                .unwrap();
            let mut updated = policy.settings.clone();
            updated.inactivity.auto_pause_enabled = false;
            let counts = counts.clone();
            Some(tokio::spawn(async move {
                replace_contended_settings(&settings, policy.revision, &updated, &counts)
                    .await
                    .unwrap()
            }))
        } else {
            None
        };
        if change_policy {
            // Exercise the writer's retry path before releasing the holder.
            tokio::time::timeout(Duration::from_secs(5), settings_write_failed.notified())
                .await
                .expect("the settings write did not observe the held fence");
        }
        holder.abort_transaction().await.unwrap();
        // This holder is artificial. Production retains and retries failed candidates:
        // api/crates/cma/src/sandbox/reconciliation_sweep.rs:364,376,383.
        // The 15 s sweep cadence is in
        // api/crates/cma/src/sandbox/reconciliation_supervision.rs:6,24.
        *deadline_anchor.lock().unwrap() = Some(tokio::time::Instant::now());
        if let Some(task) = policy_task {
            let updated = task.await.unwrap();
            assert_eq!(updated.revision, policy.revision + 1);
            assert!(!updated.settings.inactivity.auto_pause_enabled);
            assert_eq!(settings.read().await.unwrap(), updated);
            cutover.notify_one();
        }
        let mut scheduled = 0;
        let mut skipped = 0;
        let mut errors = Vec::new();
        for (id, task) in ids.iter().zip(tasks) {
            match task.await.unwrap() {
                Ok(SandboxMaintenanceOutcome::Scheduled) => {
                    scheduled += 1;
                    counts.lock().unwrap().paused.insert(id.to_string());
                }
                Ok(SandboxMaintenanceOutcome::NotEligible) => skipped += 1,
                other => errors.push(format!(
                    "{id}: {}",
                    if other.is_err() {
                        "error"
                    } else {
                        "unexpected outcome"
                    }
                )),
            }
            let value = setup.get_sandbox(id).await.unwrap().unwrap();
            assert_ne!(value.status, SandboxStatus::Failed);
            assert_eq!(
                value.status,
                if counts.lock().unwrap().paused.contains(id.as_str()) {
                    SandboxStatus::Pausing
                } else {
                    SandboxStatus::Active
                }
            );
        }
        let final_fence = setup
            .budget_settings
            .find_one(doc! {"_id":"platform"})
            .await
            .unwrap()
            .unwrap()
            .get_i64("inactivity_admission_fence")
            .unwrap_or(0);
        {
            let counts = counts.lock().unwrap();
            eprintln!(
                "contention exhausted_windows={} settings_write_conflicts={}",
                counts.exhausted_windows, counts.settings_write_conflicts
            );
            eprintln!("contention change_policy={change_policy}: attempts={} retries={} conflicts={} aborts={} scheduled={scheduled} not_eligible={skipped} errors={} cutover={:?} final_fence={final_fence}", counts.attempts, counts.attempts.saturating_sub(32), counts.conflicts, counts.aborts, errors.len(), counts.cutover_fence);
            assert!(errors.is_empty(), "{errors:?}");
            assert_eq!(scheduled + skipped, 32);
            assert_eq!(final_fence, scheduled);
            if change_policy {
                assert!(skipped > 0);
                assert_eq!(
                    counts.cutover_fence,
                    Some(final_fence),
                    "no stale pause may commit after the settings change"
                );
            } else {
                assert_eq!(scheduled, 32);
            }
        }
        for id in &ids {
            if counts.lock().unwrap().paused.contains(id.as_str()) {
                super::contention_completion::complete(&setup, id).await;
            }
        }
        admin.database(&name).drop().await.unwrap();
    }
}

async fn replace_contended_settings(
    settings: &MongoPlatformSettingsStore,
    revision: u64,
    updated: &PlatformSettings,
    counts: &Mutex<Counts>,
) -> Result<StoredPlatformSettings, StoreError> {
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let before = counts.lock().unwrap().settings_write_conflicts;
            let result = settings.replace(revision, updated).await;
            // A fence-only write keeps the CAS revision unchanged. Retry
            // only if Mongo reported a transient body failure on this CAS.
            // An equal-revision error alone could hide an uncertain commit.
            if !matches!(&result, Err(StoreError::RevisionConflict { expected, actual })
                if *expected == revision && *actual == revision)
                || counts.lock().unwrap().settings_write_conflicts == before
            {
                return result;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("settings cutover did not settle within its fixture bound")
}

async fn admit_cohort_candidate(
    store: &MongoSandboxStore,
    request: SandboxInactivityPauseRequest,
    deadline_anchor: Arc<Mutex<Option<tokio::time::Instant>>>,
    counts: &Mutex<Counts>,
) -> Result<SandboxMaintenanceOutcome, SandboxStoreError> {
    // A production sweep retains an unavailable candidate for a later attempt.
    // The cohort proves eventual admission and fencing, not that 32 serialized
    // transactions fit inside one five-second window on a loaded CI runner.
    tokio::time::timeout(Duration::from_secs(60), async {
        let original = store.get_sandbox(&request.sandbox_id).await?;
        let mut anchor = Some(deadline_anchor);
        loop {
            let result = super::super::retry::RETRY_DEADLINE_ANCHOR
                .scope(
                    anchor.take(),
                    store.schedule_inactivity_pause(request.clone()),
                )
                .await;
            if !matches!(&result, Err(SandboxStoreError::Unavailable(message))
                if message.starts_with("sandbox maintenance exhausted its retry budget:"))
            {
                return result;
            }
            counts.lock().unwrap().exhausted_windows += 1;
            // Retry only a proved unchanged candidate. In particular, an
            // ambiguous commit or any other store failure must still fail.
            assert_eq!(store.get_sandbox(&request.sandbox_id).await?, original);
            assert!(store
                .sandbox_events(&request.sandbox_id, 0, 10)
                .await?
                .is_empty());
        }
    })
    .await
    .expect("maintenance contention cohort did not settle within its fixture bound")
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn cohort_resubmits_an_exhausted_window_but_preserves_other_errors() {
    let (client, name, store) = database().await;
    let (sandbox, _) = fixture(&store, "sbx_cohort_retry").await;
    let (_, policy) = policy_tests::enable(&client, &name).await;
    let request = SandboxInactivityPauseRequest {
        auto_pause_enabled: true,
        expected_policy_revision: Some(policy.revision),
        ..pause(&sandbox.id)
    };
    let expired = || {
        Arc::new(Mutex::new(Some(
            tokio::time::Instant::now() - Duration::from_secs(6),
        )))
    };
    let counts = Mutex::new(Counts::default());
    let mut invalid = request.clone();
    invalid.requested_at = "invalid timestamp".into();
    let expected = store.schedule_inactivity_pause(invalid.clone()).await;
    assert!(expected.is_err());
    assert_eq!(
        admit_cohort_candidate(&store, invalid, expired(), &counts).await,
        expected
    );
    assert_eq!(counts.lock().unwrap().exhausted_windows, 0);
    assert_eq!(
        admit_cohort_candidate(&store, request, expired(), &counts)
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    assert_eq!(counts.lock().unwrap().exhausted_windows, 1);
    assert_eq!(
        store
            .get_sandbox(&sandbox.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SandboxStatus::Pausing
    );
    assert_eq!(
        store
            .sandbox_events(&sandbox.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .budget_settings
            .find_one(doc! {"_id":"platform"})
            .await
            .unwrap()
            .unwrap()
            .get_i64("inactivity_admission_fence")
            .unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn persistent_settings_contention_exhausts_bounded_retries_without_lifecycle_failure() {
    let (client, name, store) = database().await;
    let (sandbox, _) = fixture(&store, "sbx_persistent_contention").await;
    let (_, policy) = policy_tests::enable(&client, &name).await;
    let mut holder = client.start_session().await.unwrap();
    start_transaction(&mut holder).await.unwrap();
    store
        .budget_settings
        .update_one(
            doc! {"_id":"platform"},
            doc! {"$inc":{"inactivity_admission_fence":1_i64}},
        )
        .session(&mut holder)
        .await
        .unwrap();
    let started = tokio::time::Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(8),
        store.schedule_inactivity_pause(SandboxInactivityPauseRequest {
            auto_pause_enabled: true,
            expected_policy_revision: Some(policy.revision),
            ..pause(&sandbox.id)
        }),
    )
    .await
    .unwrap();
    holder.abort_transaction().await.unwrap();
    assert!(
        matches!(result, Err(SandboxStoreError::Unavailable(message)) if message.contains("retry budget"))
    );
    assert!(started.elapsed() >= Duration::from_secs(4));
    assert!(started.elapsed() < Duration::from_secs(8));
    assert_eq!(
        store.get_sandbox(&sandbox.id).await.unwrap(),
        Some(sandbox.clone())
    );
    assert!(store
        .sandbox_events(&sandbox.id, 0, 10)
        .await
        .unwrap()
        .is_empty());
    client.database(&name).drop().await.unwrap();
}
