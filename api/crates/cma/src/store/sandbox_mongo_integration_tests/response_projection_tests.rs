use super::*;
use crate::store::mongo_read_telemetry as reads;
use layer2_core::sandbox::store::AgentResponseLogStore;

mod fixture;
use fixture::Fixture;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn bounded_response_source_matches_the_turn_and_reduces_total_checkouts() {
    for count in [1_000, 10_000] {
        let turn = count - 9;
        let f = Fixture::new(count, turn, None, Some("retained-model"), 64).await;
        let before = f.reset();
        let (legacy, legacy_reads) = reads::isolated_test(async {
            let entries = f.store.agent_log_prefix(&f.agent.id, count).await.unwrap();
            assert!(f
                .store
                .agent_log(&f.agent.id, count, 1)
                .await
                .unwrap()
                .is_empty());
            (entries, reads::snapshot(AGENT_LOG))
        })
        .await;
        let legacy_checkouts = f.used(before);
        let before = f.reset();
        let (window, bounded_reads) = reads::isolated_test(async {
            let window = f
                .store
                .agent_response_log_window(&f.agent, Some(turn))
                .await
                .unwrap()
                .unwrap();
            (window, reads::snapshot(AGENT_LOG))
        })
        .await;
        let bounded_checkouts = f.used(before);
        assert_eq!(window.through, count);
        assert_eq!(window.entries, legacy[(turn - 1) as usize..]);
        assert_eq!(window.entries, f.entries[(turn - 1) as usize..]);
        assert_eq!(
            bounded_checkouts, 4,
            "shared head/tail/extra-tail proof plus interval"
        );
        assert!(legacy_checkouts > bounded_checkouts);
        let [_, _, legacy_rows, legacy_bytes] = legacy_reads;
        let [_, _, bounded_rows, bounded_bytes] = bounded_reads;
        assert_eq!(bounded_rows, 11, "ten turn rows plus shared tail proof");
        assert!(legacy_rows >= count);
        assert!(bounded_bytes < 40_000);
        assert!(legacy_bytes > bounded_bytes * 50);
        f.assert_snapshot();
        eprintln!("response_source history={count} turn_rows=10 legacy_checkouts={legacy_checkouts} bounded_checkouts={bounded_checkouts} legacy_rows={legacy_rows} bounded_rows={bounded_rows} legacy_bson_bytes={legacy_bytes} bounded_bson_bytes={bounded_bytes}");
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn response_window_retains_the_next_turn_witness_and_unbound_has_no_output() {
    let f = Fixture::new(1_000, 100, Some(107), Some("selected-model"), 64).await;
    f.reset();
    let window = f
        .store
        .agent_response_log_window(&f.agent, Some(100))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(window.entries, f.entries[99..107]);
    assert_eq!(window.entries.last().unwrap().seq, 107);
    f.assert_snapshot();
    let before = f.reset();
    let queued = f
        .store
        .agent_response_log_window(&f.agent, None)
        .await
        .unwrap()
        .unwrap();
    assert!(queued.entries.is_empty());
    assert_eq!(
        f.used(before),
        3,
        "unbound response only needs shared proof"
    );
    f.assert_snapshot();
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn response_window_budget_model_and_disabled_fallbacks_do_not_write() {
    for (turn, model, bytes) in [
        (100, Some("retained"), 64),
        (990, None, 64),
        (990, Some("retained"), 4 * 1024 * 1024),
    ] {
        let f = Fixture::new(1_000, turn, None, model, bytes).await;
        let original = f
            .store
            .agents
            .find_one(doc! { "_id": f.agent.id.as_str() })
            .await
            .unwrap();
        f.reset();
        assert!(f
            .store
            .agent_response_log_window(&f.agent, Some(turn))
            .await
            .unwrap()
            .is_none());
        f.assert_snapshot();
        assert_eq!(
            original,
            f.store
                .agents
                .find_one(doc! { "_id": f.agent.id.as_str() })
                .await
                .unwrap()
        );
        let disabled = f.store.clone().with_bounded_response_reads(false);
        let before = f.reset();
        assert!(disabled
            .agent_response_log_window(&f.agent, Some(turn))
            .await
            .unwrap()
            .is_none());
        assert_eq!(f.used(before), 0);
        f.assert_read_only();
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn response_window_refuses_quarantine_stale_capture_and_invalid_turn_without_writes() {
    use crate::store::agent_log_integrity::{INTEGRITY_FIELD, QUARANTINE_FIELD};
    for case in [
        "quarantine",
        "stale_capture",
        "invalid_turn",
        "missing_proof",
        "unsupported_proof",
    ] {
        let mut f = Fixture::new(1_000, 990, None, Some("retained-model"), 64).await;
        match case {
            "quarantine" => {
                f.store
                    .agents
                    .update_one(
                        doc! { "_id":f.agent.id.as_str() },
                        doc! { "$set":{ QUARANTINE_FIELD:{} }},
                    )
                    .await
                    .unwrap();
            }
            "stale_capture" => f.agent.conversation.last_seq -= 1,
            "missing_proof" => {
                f.store
                    .agents
                    .update_one(
                        doc! { "_id":f.agent.id.as_str() },
                        doc! { "$unset":{ INTEGRITY_FIELD:"" }},
                    )
                    .await
                    .unwrap();
            }
            "unsupported_proof" => {
                f.store
                    .agents
                    .update_one(
                        doc! { "_id":f.agent.id.as_str() },
                        doc! { "$set":{ "agent_log_integrity_v1.format":2_i64 }},
                    )
                    .await
                    .unwrap();
            }
            _ => {}
        }
        f.reset();
        let result = f
            .store
            .agent_response_log_window(
                &f.agent,
                Some(if case == "invalid_turn" { 1_001 } else { 990 }),
            )
            .await;
        if matches!(case, "missing_proof" | "unsupported_proof") {
            assert!(result.unwrap().is_none(), "{case}");
        } else {
            assert!(result.is_err(), "{case}");
        }
        f.assert_read_only();
        f.cleanup().await;
    }
}
