//! Exact pending-set recovery, including all proof and fallback checkouts.
use super::*;
use layer2_core::sandbox::PendingAgentApprovals;

mod fixture;
mod refusals;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn approval_suffix_preserves_exact_requests_and_bounds_total_reads() {
    for history in [1_000, 10_000] {
        let f = Fixture::new(history)
            .await
            .prepare()
            .await
            .approvals(false, false)
            .await;
        telemetry::isolated_test(async {
            let before = f.reset_commands();
            let baseline = f.original_approvals().await;
            let baseline_checkouts = f.checkouts.load(Ordering::SeqCst) - before;
            let baseline_rows = telemetry::snapshot(AGENT_LOG)[2];
            let before = f.reset_commands();
            let bounded = f.store.pending_agent_approvals(&f.id).await.unwrap();
            let checkouts = f.checkouts.load(Ordering::SeqCst) - before;
            let rows = telemetry::snapshot(AGENT_LOG)[2] - baseline_rows;
            assert_eq!(bounded, baseline);
            assert_eq!(bounded.iter().map(|e| e.seq).collect::<Vec<_>>(),
                vec![history - 3, history, history - 1]);
            assert_eq!(checkouts, 4, "envelope, tail, extra-tail and bounded suffix");
            assert_eq!(rows, 201);
            assert!(checkouts < baseline_checkouts);
            f.assert_snapshot_reads();
            eprintln!("approval_suffix history={history} original_checkouts={baseline_checkouts} bounded_checkouts={checkouts} original_rows={baseline_rows} bounded_rows={rows}");
        }).await;
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn approval_suffix_fallbacks_preserve_pending_requests() {
    for mode in [
        "long",
        "bytes",
        "switch_off",
        "cohort_off",
        "missing",
        "stale",
        "unsupported",
    ] {
        let mut f = Fixture::new(1_000)
            .await
            .prepare()
            .await
            .approvals(mode == "long", mode == "bytes")
            .await;
        let expected = f.original_approvals().await;
        match mode {
            "switch_off" => f.store = f.store.with_bounded_approval_reads(false),
            "cohort_off" => f.store.bounded_admission_agents.clear(),
            "missing" => {
                f.change_proof(doc! { "$unset": { INTEGRITY_FIELD: "" }})
                    .await
            }
            "stale" => {
                f.change_proof(doc! { "$inc": { "revision": 1_i64, "record.revision": 1_i64 }})
                    .await
            }
            "unsupported" => {
                f.change_proof(doc! { "$set": { "agent_log_integrity_v1.format": 2_i64 }})
                    .await
            }
            _ => {}
        }
        let before = f.reset_commands();
        assert_eq!(
            f.store.pending_agent_approvals(&f.id).await.unwrap(),
            expected,
            "{mode}"
        );
        f.assert_snapshot_reads();
        eprintln!(
            "approval_fallback mode={mode} checkouts={}",
            f.checkouts.load(Ordering::SeqCst) - before
        );
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn approval_suffix_stays_in_the_captured_snapshot_after_resolution() {
    let f = Fixture::new(1_000)
        .await
        .prepare()
        .await
        .approvals(false, false)
        .await;
    let expected = f.store.pending_agent_approvals(&f.id).await.unwrap();
    let mut session = f.client.start_session().snapshot(true).await.unwrap();
    f.store
        .agents
        .find_one(doc! { "_id": f.id.as_str() })
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .unwrap();
    f.store
        .append_agent_log(
            &f.id,
            TWO_MINUTES.into(),
            AgentLogKind::ApprovalResolved {
                approval_id: "z".into(),
                decision: "denied".into(),
            },
        )
        .await
        .unwrap();
    f.reset_commands();
    let captured = crate::store::agent_conversation_store::reads::pending_approvals_snapshot(
        &f.store,
        &f.id,
        &mut session,
    )
    .await
    .unwrap();
    assert_eq!(captured, expected);
    f.assert_snapshot_reads();
    let current = f.store.pending_agent_approvals(&f.id).await.unwrap();
    assert_eq!(current, expected[..2]);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn small_and_completed_approval_histories_keep_empty_and_absent_semantics() {
    for history in [200, 1_000] {
        let mut f = Fixture::new(history).await.prepare().await;
        f.store = f.store.with_bounded_approval_reads(true);
        f.reset_commands();
        assert!(f
            .store
            .pending_agent_approvals(&f.id)
            .await
            .unwrap()
            .is_empty());
        f.assert_snapshot_reads();
        f.reset_commands();
        assert_eq!(
            f.store
                .pending_agent_approvals(&AgentId("agt_absent".into()))
                .await,
            Err(SandboxStoreError::NotFound)
        );
        f.assert_snapshot_reads();
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn fleet_selection_and_restart_keep_the_approval_switch_independent() {
    let mut f = Fixture::new(1_000)
        .await
        .prepare()
        .await
        .approvals(false, false)
        .await;
    let expected = f.original_approvals().await;
    // Reopening discards process-local activation state, preserving only the
    // durable proof. Neither the fleet selector nor the title switch enables
    // approval suffix reads; selected but unprepared Agents still read fully.
    for (percent, enabled, title, trusted, rows) in [
        (100, false, true, true, 1_001),
        (100, true, false, true, 201),
        (0, true, true, true, 1_001),
        (100, true, true, false, 1_000),
    ] {
        f.store = MongoSandboxStore::open(&f.client, &f.name)
            .await
            .unwrap()
            .with_fleet_log_percentage(percent)
            .with_bounded_title_reads(title)
            .with_bounded_approval_reads(enabled);
        if !trusted {
            f.change_proof(doc! { "$unset": { INTEGRITY_FIELD: "" }})
                .await;
        }
        let before = f
            .store
            .agents
            .find_one(doc! { "_id": f.id.as_str() })
            .await
            .unwrap()
            .unwrap();
        f.reset_commands();
        telemetry::isolated_test(async {
            assert_eq!(
                f.store.pending_agent_approvals(&f.id).await.unwrap(),
                expected
            );
            assert_eq!(telemetry::snapshot(AGENT_LOG)[2], rows);
        })
        .await;
        f.assert_snapshot_reads();
        assert_eq!(
            f.store
                .agents
                .find_one(doc! { "_id": f.id.as_str() })
                .await
                .unwrap()
                .unwrap(),
            before
        );
    }
    f.cleanup().await;
}
