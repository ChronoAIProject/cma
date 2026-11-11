use super::*;
use crate::store::mongo_read_telemetry as reads;
use std::sync::atomic::AtomicU64;

mod fallback;
mod fixture;
mod refusals;
use fixture::Fixture;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn trusted_title_prefix_preserves_input_and_bounds_total_checkouts() {
    for count in [1_000, 10_000] {
        let f = Fixture::new(count, 6, false, 64).await;
        let disabled = MongoSandboxStore::open(&f.client, &f.name).await.unwrap();
        let before = f.reset();
        let (legacy, legacy_rows) = reads::isolated_test(async {
            let transcript = disabled.agent_title_transcript(&f.agent.id).await.unwrap();
            (transcript, reads::snapshot(AGENT_LOG)[2])
        })
        .await;
        let legacy_checkouts = f.used(before);
        let before = f.reset();
        let (bounded, bounded_rows) = reads::isolated_test(async {
            let transcript = f.store.agent_title_transcript(&f.agent.id).await.unwrap();
            (transcript, reads::snapshot(AGENT_LOG)[2])
        })
        .await;
        let bounded_checkouts = f.used(before);
        assert_eq!(bounded, legacy);
        assert_eq!(bounded, f.expected());
        assert_eq!(
            bounded.as_ref().unwrap().assistant_message(),
            "First task answer"
        );
        assert_eq!(
            bounded_checkouts, 5,
            "title envelope + shared proof + prefix"
        );
        assert_eq!(legacy_checkouts, if count == 1_000 { 6 } else { 8 });
        assert_eq!(legacy_rows, count + 1);
        assert_eq!(bounded_rows, 201);
        f.assert_snapshot_read_only();
        eprintln!("title_prefix history={count} prefix=200 legacy_checkouts={legacy_checkouts} bounded_checkouts={bounded_checkouts} legacy_rows={legacy_rows} bounded_rows={bounded_rows}");
        f.reset();
        let page = f
            .store
            .agent_title_candidates(&PageRequest {
                cursor: None,
                limit: 1,
            })
            .await
            .unwrap();
        assert_eq!(page.items, vec![f.agent.id.clone()]);
        f.assert_snapshot_read_only();
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn title_prefix_preserves_setup_exclusion_and_complete_fallbacks() {
    for (count, end, setup, owner_bytes, bounded) in [
        (1_000, 10, true, 64, true),
        (1_000, 200, false, 64, true),
        (1_000, 201, false, 64, false),
        (1_000, 600, true, 64, false),
        (1_000, 6, false, 4 * 1024 * 1024, false),
        (200, 6, false, 64, false),
    ] {
        let f = Fixture::new(count, end, setup, owner_bytes).await;
        let before = f.reset();
        let result = reads::isolated_test(async {
            let transcript = f.store.agent_title_transcript(&f.agent.id).await.unwrap();
            let rows = reads::snapshot(AGENT_LOG)[2];
            if bounded {
                assert_eq!(rows, 201);
            } else {
                assert!(rows >= count);
            }
            transcript
        })
        .await;
        if count == 200 {
            assert_eq!(
                f.used(before),
                6,
                "small histories skip the new prefix proof"
            );
        }
        assert_eq!(
            result,
            f.expected(),
            "count={count} end={end} setup={setup}"
        );
        assert_eq!(
            result.as_ref().unwrap().turn_sequence(),
            if setup { 7 } else { 3 }
        );
        f.assert_snapshot_read_only();
        eprintln!("title_prefix_fallback history={count} task_end={end} setup={setup} owner_bytes={owner_bytes} bounded={bounded} checkouts={}", f.used(before));
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn an_interrupted_first_task_does_not_borrow_a_later_completed_turn() {
    use crate::store::agent_log_integrity::INTEGRITY_FIELD;
    let mut f = Fixture::new(1_000, 6, false, 64).await;
    f.entries[5].kind = AgentLogKind::TurnInterrupted { turn_sequence: 3 };
    f.store
        .agent_log
        .replace_one(
            doc! { "agent_id": f.agent.id.as_str(), "seq": 6_i64 },
            log_document(&f.entries[5]).unwrap(),
        )
        .await
        .unwrap();
    f.store
        .agents
        .update_one(
            doc! { "_id": f.agent.id.as_str() },
            doc! {
                "$unset": { INTEGRITY_FIELD: "" }
            },
        )
        .await
        .unwrap();
    f.store = f
        .store
        .prepare_bounded_admission(std::slice::from_ref(&f.agent.id))
        .await
        .unwrap();
    assert_eq!(f.expected(), None);
    reads::isolated_test(async {
        assert_eq!(
            f.store.agent_title_transcript(&f.agent.id).await.unwrap(),
            None
        );
        assert!(reads::snapshot(AGENT_LOG)[2] >= 1_000);
    })
    .await;
    assert!(f
        .store
        .agent_title_candidates(&PageRequest {
            cursor: None,
            limit: 1
        })
        .await
        .unwrap()
        .items
        .is_empty());
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn title_prefix_keeps_the_existing_snapshot_after_an_append() {
    let f = Fixture::new(1_000, 6, false, 64).await;
    let mut session = f.client.start_session().snapshot(true).await.unwrap();
    let captured = f
        .store
        .agents
        .find_one(doc! { "_id": f.agent.id.as_str() })
        .read_concern(mongodb::options::ReadConcern::snapshot())
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    let agent = crate::store::agents::read_agent(&captured).unwrap();
    f.store
        .append_agent_log(
            &agent.id,
            NOW.into(),
            AgentLogKind::WorkStepCompleted {
                step: AgentWorkStep::None,
            },
        )
        .await
        .unwrap();
    f.reset();
    let transcript =
        crate::store::agent_titles::transcript_at_snapshot(&f.store, &agent, &mut session)
            .await
            .unwrap();
    assert_eq!(transcript, f.expected());
    f.assert_snapshot_read_only();
    assert_eq!(
        f.store
            .get_agent(&agent.id)
            .await
            .unwrap()
            .unwrap()
            .conversation
            .last_seq,
        1_001
    );
    f.cleanup().await;
}
