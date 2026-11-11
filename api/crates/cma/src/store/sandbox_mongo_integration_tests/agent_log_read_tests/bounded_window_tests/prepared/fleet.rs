use super::*;
use crate::store::agent_log_telemetry::{isolated_test, snapshot, ScanCaller};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn fleet_window_reads_never_prepare_and_measure_first_and_steady_append_cost() {
    // Leave room for both measured appends below the 10,000-entry normal cap.
    for history in [1_000, 9_990] {
        let mut f = Fixture::new(history).await.prepare().await;
        f.store.bounded_admission_agents.clear();
        f.store
            .agents
            .update_one(
                doc! { "_id": f.id.as_str() },
                doc! { "$unset": { INTEGRITY_FIELD: "" }},
            )
            .await
            .unwrap();
        f.store = f.store.with_fleet_log_percentage(100);
        let before_row = f
            .store
            .agents
            .find_one(doc! { "_id": f.id.as_str() })
            .await
            .unwrap()
            .unwrap();
        let before = f.reset_commands();
        let legacy_page = f.store.agent_log(&f.id, history - 200, 200).await.unwrap();
        let unprepared_read_checkouts = f.checkouts.load(Ordering::SeqCst) - before;
        f.assert_read_only();
        assert_eq!(
            f.store
                .agents
                .find_one(doc! { "_id": f.id.as_str() })
                .await
                .unwrap()
                .unwrap(),
            before_row,
            "read-only traffic cannot prepare the fleet"
        );
        isolated_test(async {
            let before = f.reset_commands();
            f.store.append_agent_log(&f.id, NOW.into(), AgentLogKind::WorkStepCompleted { step: AgentWorkStep::None }).await.unwrap();
            let first_write = f.checkouts.load(Ordering::SeqCst) - before;
            assert_eq!(snapshot(ScanCaller::RecoveryProof, true)[0], 1);
            assert_eq!(snapshot(ScanCaller::Other, true)[0], 0);
            let before = f.reset_commands();
            f.store.append_agent_log(&f.id, NOW.into(), AgentLogKind::WorkStepCompleted { step: AgentWorkStep::None }).await.unwrap();
            let steady_write = f.checkouts.load(Ordering::SeqCst) - before;
            assert!(first_write > steady_write, "initial audit cost must be reported");
            assert_eq!(snapshot(ScanCaller::RecoveryProof, true)[0], 1);
            assert_eq!(snapshot(ScanCaller::Other, true)[0], 0);
            let before = f.reset_commands();
            let page = f.store.agent_log_range(&f.id, history - 200, history, 200).await.unwrap();
            let bounded_read = f.checkouts.load(Ordering::SeqCst) - before;
            assert_eq!(page, legacy_page);
            assert_eq!(bounded_read, 4);
            f.assert_read_only();
            eprintln!("fleet_total history={history} unprepared_read_checkouts={unprepared_read_checkouts} first_append_checkouts={first_write} steady_append_checkouts={steady_write} prepared_window_checkouts={bounded_read} preparation_scans=1 steady_append_scans=0");
        }).await;
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn fleet_quarantined_public_read_never_mutates_the_marker() {
    let mut f = Fixture::new(1_000).await.prepare().await;
    f.store.bounded_admission_agents.clear();
    f.store = f.store.with_fleet_log_percentage(100);
    f.store
        .agents
        .update_one(
            doc! { "_id": f.id.as_str() },
            doc! { "$set": { QUARANTINE_FIELD: {} }},
        )
        .await
        .unwrap();
    let filter = doc! { "_id": f.id.as_str() };
    let before = f
        .store
        .agents
        .find_one(filter.clone())
        .await
        .unwrap()
        .unwrap();
    f.reset_commands();
    assert!(matches!(
        f.store.agent_log(&f.id, 800, 200).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    f.assert_read_only();
    assert_eq!(
        f.store.agents.find_one(filter).await.unwrap().unwrap(),
        before
    );
    f.cleanup().await;
}
