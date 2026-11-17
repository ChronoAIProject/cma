use super::*;
use layer2_core::PlatformSettingsStore;
use mongodb::{event::command::CommandEvent, event::EventHandler};
use std::{collections::BTreeSet, sync::Mutex, time::Duration};

#[path = "contention_diagnostics.rs"]
mod diagnostics;

#[derive(Default)]
struct Counts {
    attempts: usize,
    conflicts: usize,
    aborts: usize,
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
        let mut replicas = Vec::new();
        for _ in 0..2 {
            let mut options =
                ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
                    .await
                    .unwrap();
            let observed = counts.clone();
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
                        counts.conflicts += 1
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
            let diagnostic_anchor = deadline_anchor.clone();
            tasks.push(tokio::spawn(
                super::super::retry::RETRY_DEADLINE_ANCHOR.scope(
                    Some(deadline_anchor.clone()),
                    async move {
                        barrier.wait().await;
                        let started = tokio::time::Instant::now();
                        let result = store.schedule_inactivity_pause(request).await;
                        let elapsed_ms = started.elapsed().as_millis();
                        let post_release_elapsed_ms = diagnostic_anchor
                            .lock()
                            .unwrap()
                            .map(|released| released.elapsed().as_millis());
                        (result, elapsed_ms, post_release_elapsed_ms)
                    },
                ),
            ));
        }
        barrier.wait().await;
        tokio::time::timeout(Duration::from_secs(5), async {
            while counts.lock().unwrap().conflicts < 32 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        // Use the production settings adapter on one independently monitored
        // replica. Its atomic returned fence is the exact serialization cutover.
        let policy_task = if change_policy {
            let settings = crate::store::settings::MongoPlatformSettingsStore::open(
                &replicas[0].client,
                &name,
            )
            .await
            .unwrap();
            let mut updated = policy.settings.clone();
            updated.inactivity.auto_pause_enabled = false;
            Some(tokio::spawn(async move {
                settings.replace(policy.revision, &updated).await.unwrap()
            }))
        } else {
            None
        };
        holder.abort_transaction().await.unwrap();
        // This holder is artificial. Production retains and retries failed candidates:
        // api/crates/cma/src/sandbox/reconciliation_sweep.rs:364,376,383.
        // The 15 s sweep cadence is in
        // api/crates/cma/src/sandbox/reconciliation_supervision.rs:6,24.
        *deadline_anchor.lock().unwrap() = Some(tokio::time::Instant::now());
        if let Some(task) = policy_task {
            task.await.unwrap();
            assert!(
                !settings
                    .read()
                    .await
                    .unwrap()
                    .settings
                    .inactivity
                    .auto_pause_enabled
            );
        }
        let mut scheduled = 0;
        let mut skipped = 0;
        let mut errors = Vec::new();
        for (id, task) in ids.iter().zip(tasks) {
            let (result, elapsed_ms, post_release_elapsed_ms) = task.await.unwrap();
            let classification = match &result {
                Ok(outcome) => format!("{outcome:?}"),
                Err(error) => diagnostics::classify_error(error).into(),
            };
            eprintln!("maintenance_candidate id={id} elapsed_ms={elapsed_ms} post_release_elapsed_ms={post_release_elapsed_ms:?} result={classification}");
            match result {
                Ok(SandboxMaintenanceOutcome::Scheduled) => {
                    scheduled += 1;
                    counts.lock().unwrap().paused.insert(id.to_string());
                }
                Ok(SandboxMaintenanceOutcome::NotEligible) => skipped += 1,
                _ => errors.push(format!("{id}: {classification}")),
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
