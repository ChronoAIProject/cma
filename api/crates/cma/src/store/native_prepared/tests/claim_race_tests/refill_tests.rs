use super::*;
use crate::store::admission_wake_test_support::CommitGate;
use futures_util::FutureExt;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn native_prepared_refill_wakes_only_after_consumption_commit() {
    let gate = Arc::new(CommitGate::default());
    let client = gate.client().await;
    let name = format!("cma_native_refill_{}", uuid::Uuid::new_v4().simple());
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let plan = plan();
    store
        .reconcile_native_prepared_target(Some(&plan), 1, NOW)
        .await
        .unwrap();
    let ready = publish_ready(&store).await;
    let request = admission(&store, &plan, "owner", "refill").await;
    let consumed = store.native_prepared_changed.clone();
    gate.arm();
    let writer = store.clone();
    let input = request.clone();
    let task = tokio::spawn(async move { admit_at(&writer, input, NOW).await });
    tokio::time::timeout(Duration::from_secs(5), gate.entered.notified())
        .await
        .unwrap();
    let premature = consumed.notified().now_or_never().is_some();
    let before_commit = store.get_sandbox(&ready.payload.id).await.unwrap();
    gate.release();
    let result = task.await.unwrap().unwrap();
    assert!(!premature, "uncommitted consumption must not refill");
    assert!(before_commit.is_none());
    assert_eq!(created(result).id, ready.payload.id);
    // Legacy cleanup may consume its own permit without stealing native refill.
    assert!(store.warmup_changed.notified().now_or_never().is_some());
    assert!(consumed.notified().now_or_never().is_some());
    assert!(consumed.notified().now_or_never().is_none());
    assert!(matches!(
        admit_at(&store, request, NOW).await.unwrap(),
        SandboxAdmissionResult::Existing(_)
    ));
    assert!(consumed.notified().now_or_never().is_none());
    store
        .reconcile_native_prepared_target(Some(&plan), 1, NOW)
        .await
        .unwrap();
    let reservations = records(&store).await;
    assert_eq!(
        reservations
            .iter()
            .filter(|r| r.payload.state == WarmSandboxState::Preparing)
            .count(),
        1
    );
    assert_eq!(
        reservations
            .iter()
            .filter(|r| r.payload.state == WarmSandboxState::Claimed)
            .count(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn native_prepared_refill_ignores_misses_replays_and_failed_admission() {
    let (client, name, store) = database().await;
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let metrics = recorder.handle();
    let _recording = metrics::set_default_local_recorder(&recorder);
    let plan = plan();
    let request = admission(&store, &plan, "owner", "no-target").await;
    assert!(matches!(
        admit_at(&store, request.clone(), NOW).await.unwrap(),
        SandboxAdmissionResult::Created(_)
    ));
    assert!(store
        .native_prepared_changed
        .notified()
        .now_or_never()
        .is_none());
    assert!(matches!(
        admit_at(&store, request, NOW).await.unwrap(),
        SandboxAdmissionResult::Existing(_)
    ));
    assert!(store
        .native_prepared_changed
        .notified()
        .now_or_never()
        .is_none());
    let mut denied = admission(&store, &plan, "owner", "denied").await;
    denied.live_limit = 0;
    assert!(admit_at(&store, denied, NOW).await.is_err());
    assert!(store
        .native_prepared_changed
        .notified()
        .now_or_never()
        .is_none());
    let rendered = metrics.render();
    let counters = rendered
        .lines()
        .filter(|line| line.starts_with("cma_native_prepared_admissions_total{"))
        .collect::<Vec<_>>();
    assert_eq!(counters.len(), 2, "{rendered}");
    for outcome in ["miss_target_disabled", "replay"] {
        assert!(
            counters
                .iter()
                .any(|line| line.contains(&format!("outcome=\"{outcome}\""))
                    && line.contains("class=\"plain\"")
                    && line.contains("origin=\"lifecycle\"")
                    && line.ends_with(" 1")),
            "{rendered}"
        );
    }
    assert!(rendered.contains("cma_native_prepared_admission_seconds_count"));
    assert!(!rendered.contains("sbx_"));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn native_prepared_hit_metrics_and_refill_cover_plain_and_machine() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let metrics = recorder.handle();
    let _recording = metrics::set_default_local_recorder(&recorder);
    for machine in [false, true] {
        let (client, name, store) = database().await;
        let desired = if machine {
            super::super::machine_tests::machine_plan()
        } else {
            plan()
        };
        let store = store
            .with_machine_runtime_version(machine.then(|| desired.warm.runtime_version.clone()));
        store
            .reconcile_native_prepared_class(Some(&desired), 1, NOW, machine)
            .await
            .unwrap();
        let reservation = if machine {
            super::super::machine_tests::ready_machine(&store).await
        } else {
            publish_ready(&store).await
        };
        let request = admission(&store, &desired, "owner", "metric-hit").await;
        let result = created(admit_at(&store, request, NOW).await.unwrap());
        assert_eq!(result.id, reservation.payload.id);
        assert!(store
            .native_prepared_changed
            .notified()
            .now_or_never()
            .is_some());
        client.database(&name).drop().await.unwrap();
    }
    let rendered = metrics.render();
    let counters = rendered
        .lines()
        .filter(|line| line.starts_with("cma_native_prepared_admissions_total{"))
        .collect::<Vec<_>>();
    assert_eq!(counters.len(), 2, "{rendered}");
    for class in ["plain", "machine"] {
        assert!(
            counters
                .iter()
                .any(|line| line.contains(&format!("class=\"{class}\""))
                    && line.contains("outcome=\"hit\"")
                    && line.contains("origin=\"lifecycle\"")
                    && line.ends_with(" 1")),
            "{rendered}"
        );
    }
}
