use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

const AFTER_PROBE: &str = "2026-09-05T00:00:01Z";

mod refill_tests;

async fn health_lease(store: &MongoSandboxStore, record: &Record) -> NativePreparedClaim {
    store
        .native_prepared
        .update_one(
            doc! {"_id":record.payload.id.as_str()},
            doc! {"$set":{"check_after":NOW}},
        )
        .await
        .unwrap();
    store
        .claim_native_prepared_work(NOW, LATER)
        .await
        .unwrap()
        .unwrap()
}

fn created(result: SandboxAdmissionResult) -> Sandbox {
    let SandboxAdmissionResult::Created(sandbox) = result else {
        panic!("new admission")
    };
    sandbox
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn native_prepared_health_completion_after_admission_is_exclusive_and_replayable() {
    let (client, name, store) = database().await;
    let plan = plan();
    store
        .reconcile_native_prepared_target(Some(&plan), 1, NOW)
        .await
        .unwrap();
    let record = publish_ready(&store).await;
    let claim = health_lease(&store, &record).await;
    let a = admission(&store, &plan, "owner-a", "lease-a").await;
    let b = admission(&store, &plan, "owner-b", "lease-b").await;
    let after_probe = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::new();
    for request in [a.clone(), b.clone()] {
        let store = store.clone();
        let clock = after_probe.clone();
        let observed_busy = Arc::new(tokio::sync::Notify::new());
        let signal = observed_busy.clone();
        let attempts = AtomicUsize::new(0);
        let task = tokio::spawn(async move {
            store
                .commit_sandbox_admission_observing(request, || {
                    // The second observation follows a fully aborted busy transaction.
                    if attempts.fetch_add(1, Ordering::SeqCst) > 0 {
                        signal.notify_one();
                    }
                    if clock.load(Ordering::SeqCst) == 0 {
                        NOW.into()
                    } else {
                        AFTER_PROBE.into()
                    }
                })
                .await
        });
        tasks.push((task, observed_busy));
    }
    for (_, signal) in &tasks {
        tokio::time::timeout(Duration::from_secs(2), signal.notified())
            .await
            .unwrap();
    }
    assert_eq!(store.sandboxes.count_documents(doc! {}).await.unwrap(), 0);
    assert!(records(&store).await.iter().all(|r| r.ownership.is_none()));
    after_probe.store(1, Ordering::SeqCst);
    store
        .finish_native_prepared_work(&claim, claim.record.clone(), AFTER_PROBE, LATER)
        .await
        .unwrap();
    let mut results = Vec::new();
    for (task, _) in tasks {
        results.push(created(
            tokio::time::timeout(Duration::from_secs(3), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
        ));
    }
    assert_eq!(results.iter().filter(|s| s.runtime.is_some()).count(), 1);
    assert_ne!(results[0].id, results[1].id);
    let warm = results.iter().find(|s| s.runtime.is_some()).unwrap();
    assert_eq!(warm.id, record.payload.id);
    assert_eq!(warm.runtime.as_ref().unwrap().bound_at, AFTER_PROBE);
    assert_eq!(warm.lifecycle.home, record.payload.home);
    let original = if warm.owner == a.sandbox.owner { a } else { b };
    let replay = admit_at(&store, original, AFTER_PROBE).await.unwrap();
    let SandboxAdmissionResult::Existing(replay) = replay else {
        panic!("origin replay")
    };
    assert_eq!(&replay, warm);
    let events = store.sandbox_events(&warm.id, 0, 10).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].appended_at, NOW,
        "immutable admission event is retained"
    );
    assert_eq!(store.sandboxes.count_documents(doc! {}).await.unwrap(), 2);
    assert_eq!(
        records(&store)
            .await
            .iter()
            .filter(|r| r.ownership.is_some())
            .count(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn native_prepared_busy_budget_falls_back_without_stealing_the_health_lease() {
    let (client, name, store) = database().await;
    let plan = plan();
    store
        .reconcile_native_prepared_target(Some(&plan), 1, NOW)
        .await
        .unwrap();
    let record = publish_ready(&store).await;
    let claim = health_lease(&store, &record).await;
    let request = admission(&store, &plan, "owner", "busy-budget").await;
    let started = tokio::time::Instant::now();
    let sandbox = created(
        tokio::time::timeout(Duration::from_secs(8), admit_at(&store, request, NOW))
            .await
            .unwrap()
            .unwrap(),
    );
    assert!(started.elapsed() >= Duration::from_millis(2_500));
    assert!(sandbox.runtime.is_none());
    assert_ne!(sandbox.id, record.payload.id);
    let untouched = records(&store).await.pop().unwrap();
    assert_eq!(untouched, record);
    // The original worker still owns its exact token/revision and may finish.
    store
        .finish_native_prepared_work(&claim, claim.record.clone(), AFTER_PROBE, LATER)
        .await
        .unwrap();
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn native_prepared_available_second_candidate_does_not_wait_for_health_lease() {
    let (client, name, store) = database().await;
    let plan = plan();
    store
        .reconcile_native_prepared_target(Some(&plan), 2, NOW)
        .await
        .unwrap();
    publish_ready(&store).await;
    publish_ready(&store).await;
    let mut ready = records(&store).await;
    ready.sort_by(|a, b| a.payload.id.cmp(&b.payload.id));
    let held = health_lease(&store, &ready[0]).await;
    let request = admission(&store, &plan, "owner", "second-candidate").await;
    let observations = AtomicUsize::new(0);
    let sandbox = created(
        store
            .commit_sandbox_admission_observing(request, || {
                observations.fetch_add(1, Ordering::SeqCst);
                NOW.into()
            })
            .await
            .unwrap(),
    );
    assert_eq!(sandbox.id, ready[1].payload.id);
    assert_eq!(observations.load(Ordering::SeqCst), 1);
    store
        .finish_native_prepared_work(&held, held.record.clone(), AFTER_PROBE, LATER)
        .await
        .unwrap();
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn native_prepared_expired_stale_and_unmatched_capacity_never_waits_or_claims() {
    for case in ["stale", "expired", "target_mismatch", "disabled"] {
        let (client, name, store) = database().await;
        let plan = plan();
        store
            .reconcile_native_prepared_target(Some(&plan), 1, NOW)
            .await
            .unwrap();
        let mut record = publish_ready(&store).await;
        let request = admission(&store, &plan, "owner", case).await;
        let observed = match case {
            "stale" => LATER,
            "expired" => {
                record.payload.updated_at = EXPIRED.into();
                let mut value = document(&record).unwrap();
                value.insert("lease_until", "2026-09-05T00:31:00Z");
                value.insert("lease_token", "active-health-probe");
                store
                    .native_prepared
                    .replace_one(doc! {"_id":record.payload.id.as_str()}, value)
                    .await
                    .unwrap();
                EXPIRED
            }
            "target_mismatch" => {
                let mut other = plan.clone();
                other.warm.profile.content.cpu_millis += 1;
                other.warm.profile.digest = content_digest(&other.warm.profile.content).unwrap();
                other.validate().unwrap();
                store
                    .native_prepared
                    .update_one(
                        doc! {"_id":"target"},
                        doc! {"$set":{"plan":record_bson(&other).unwrap()}},
                    )
                    .await
                    .unwrap();
                NOW
            }
            _ => {
                store
                    .native_prepared
                    .update_one(doc! {"_id":"target"}, doc! {"$set":{"target":0}})
                    .await
                    .unwrap();
                NOW
            }
        };
        let mut session = store.client.start_session().await.unwrap();
        crate::store::sandbox_mongo::start_transaction(&mut session)
            .await
            .unwrap();
        let outcome = super::super::admission::claim_ready(
            &store,
            &mut request.sandbox.clone(),
            observed,
            &mut session,
        )
        .await
        .map_err(|_| ())
        .unwrap();
        session.abort_transaction().await.unwrap();
        let expected = match case {
            "stale" => "miss_stale_ready_proof",
            "expired" => "miss_expired_reservation",
            "target_mismatch" => "miss_profile_mismatch",
            _ => "miss_target_disabled",
        };
        assert_eq!(outcome, ClaimOutcome::Miss(expected));
        let observations = AtomicUsize::new(0);
        let sandbox = created(
            store
                .commit_sandbox_admission_observing(request, || {
                    observations.fetch_add(1, Ordering::SeqCst);
                    observed.into()
                })
                .await
                .unwrap(),
        );
        assert!(sandbox.runtime.is_none(), "{case}");
        assert_eq!(observations.load(Ordering::SeqCst), 1, "{case}");
        assert!(records(&store).await.iter().all(|r| r.ownership.is_none()));
        client.database(&name).drop().await.unwrap();
    }
}
