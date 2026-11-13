use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn historical_target_cost_is_stable_despite_lifetime_and_large_later_output() {
    let mut previous = None;
    for count in [1_000, 10_000] {
        let f = Fixture::with_later_payload(
            count,
            100,
            Some(107),
            Some("retained-model"),
            64,
            4 * 1024 * 1024,
        )
        .await;
        let baseline = f.store.agent_log_prefix(&f.agent.id, count).await.unwrap();
        let before = f.reset();
        let (window, consumed) = reads::isolated_test(async {
            let window = f
                .store
                .agent_response_log_window(&f.agent, Some(100))
                .await
                .unwrap()
                .expect("later payload cannot force target fallback");
            (window, reads::snapshot(AGENT_LOG))
        })
        .await;
        assert_eq!(window.entries, baseline[99..107]);
        assert_eq!(window.through, count);
        assert_eq!(f.used(before), 5);
        assert_eq!(
            consumed[2], 10,
            "eight source rows, witness coordinate and tail proof"
        );
        assert!(consumed[3] < 40_000);
        if let Some(previous) = previous {
            assert_eq!(
                consumed, previous,
                "lifetime rows do not increase consumed work"
            );
        }
        previous = Some(consumed);
        f.assert_snapshot();
        let examined = boundary_documents_examined(&f).await;
        assert_eq!(examined, 7, "lookup stops at the first next-turn witness");
        eprintln!("historical_response history={count} target_rows=8 checkouts=5 consumed_rows={} consumed_bson_bytes={} boundary_documents_examined={examined}", consumed[2], consumed[3]);
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn next_turn_at_row_budget_is_included_but_one_row_beyond_falls_back() {
    for next in [299, 300] {
        let f = Fixture::new(1_000, 100, Some(next), Some("retained"), 64).await;
        f.reset();
        let window = f
            .store
            .agent_response_log_window(&f.agent, Some(100))
            .await
            .unwrap();
        if next == 299 {
            assert_eq!(window.unwrap().entries, f.entries[99..299]);
        } else {
            assert!(window.is_none());
        }
        f.assert_snapshot();
        assert_eq!(boundary_documents_examined(&f).await, 199);
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn target_byte_budget_includes_witness_and_preserves_complete_fallback() {
    let base = Fixture::new(1_000, 100, Some(107), Some("retained"), 64).await;
    let source_bytes: usize = base.entries[99..107]
        .iter()
        .map(|entry| log_document(entry).unwrap().to_vec().unwrap().len())
        .sum();
    base.cleanup().await;
    for excess in [0, 1] {
        let payload = 64 + 4 * 1024 * 1024 - source_bytes + excess;
        let f = Fixture::new(1_000, 100, Some(107), Some("retained"), payload).await;
        f.reset();
        let window = f
            .store
            .agent_response_log_window(&f.agent, Some(100))
            .await
            .unwrap();
        if excess == 0 {
            assert_eq!(window.unwrap().entries, f.entries[99..107]);
        } else {
            assert!(window.is_none());
        }
        f.assert_snapshot();
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn boundary_coordinate_does_not_bypass_source_identity_or_contiguity() {
    for case in ["witness_turn", "witness_identity", "missing_target_row"] {
        let f = Fixture::new(1_000, 100, Some(107), Some("retained"), 64).await;
        if case == "missing_target_row" {
            f.store
                .agent_log
                .delete_one(doc! {"agent_id":f.agent.id.as_str(),"seq":103_i64})
                .await
                .unwrap();
        } else {
            let field = if case == "witness_turn" {
                "record.kind.turn_sequence"
            } else {
                "record.seq"
            };
            f.store
                .agent_log
                .update_one(
                    doc! {"agent_id":f.agent.id.as_str(),"seq":107_i64},
                    doc! {"$set":{field:108_i64}},
                )
                .await
                .unwrap();
        }
        f.reset();
        assert!(
            f.store
                .agent_response_log_window(&f.agent, Some(100))
                .await
                .is_err(),
            "{case}"
        );
        f.assert_snapshot();
        f.cleanup().await;
    }
}

async fn boundary_documents_examined(f: &Fixture) -> i64 {
    let query = {
        let commands = f.commands.lock().unwrap();
        let command = commands
            .iter()
            .find(|command| {
                command.get_document("filter").is_ok_and(|filter| {
                    filter.get_str("record.kind.type").ok() == Some("turn_started")
                })
            })
            .expect("bounded boundary query");
        assert_eq!(command.get_str("hint").unwrap(), "agent_log_sequence");
        assert_eq!(
            command.get_document("projection").unwrap(),
            &doc! {"_id":0,"seq":1}
        );
        let mut query = Document::new();
        for key in [
            "find",
            "filter",
            "hint",
            "sort",
            "projection",
            "limit",
            "batchSize",
        ] {
            query.insert(key, command.get(key).unwrap().clone());
        }
        query
    };
    let explained = f
        .client
        .database(&f.name)
        .run_command(doc! {
            "explain":query,"verbosity":"executionStats"
        })
        .await
        .unwrap();
    let stats = explained.get_document("executionStats").unwrap();
    assert!(stats.get_bool("executionSuccess").unwrap());
    stats
        .get_i32("totalDocsExamined")
        .map(i64::from)
        .or_else(|_| stats.get_i64("totalDocsExamined"))
        .unwrap()
}
