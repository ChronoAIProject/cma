use super::*;
use crate::store::{
    agent_log::log_document, agent_log_integrity::INTEGRITY_FIELD,
    mongo_read_telemetry as telemetry,
};
use layer2_core::sandbox::{store::AgentLogStore, AgentTranscriptPageRequest};
mod fixture;
use fixture::Fixture;

fn first(limit: u16) -> AgentTranscriptPageRequest {
    AgentTranscriptPageRequest {
        turn: None,
        after: None,
        through: None,
        limit,
    }
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn privacy_export_pages_legacy_history_without_the_opening_index() {
    let fixture = Fixture::new(50, 1, true).await;
    let indexed = fixture
        .store
        .agent_log_export_page(&fixture.agent, &first(7))
        .await
        .unwrap()
        .unwrap();
    fixture
        .store
        .agent_log
        .drop_index("agent_log_kind_sequence")
        .await
        .unwrap();
    let mut request = first(7);
    let mut messages = Vec::new();
    loop {
        let page = fixture
            .store
            .agent_log_export_page(&fixture.agent, &request)
            .await
            .unwrap()
            .unwrap();
        if request.turn.is_none() {
            assert_eq!(page, indexed);
        }
        assert_eq!(
            page.transcript.through_seq,
            fixture.agent.conversation.last_seq
        );
        messages.extend(
            page.transcript
                .turns
                .into_iter()
                .flat_map(|turn| turn.messages),
        );
        let Some(next) = page.next else {
            break;
        };
        request = next;
    }
    assert_eq!(messages, fixture.expected().turns.remove(0).messages);
    fixture
        .store
        .agent_log
        .delete_one(doc! {
            "agent_id": fixture.agent.id.as_str(), "seq": 5_i64,
        })
        .await
        .unwrap();
    assert_eq!(
        fixture
            .store
            .agent_log_export_page(&fixture.agent, &first(7))
            .await,
        Err(SandboxStoreError::SequenceConflict)
    );
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn opening_first_window_and_usage_are_bounded_with_real_prefix_proof() {
    for messages in [1_000, 9_000] {
        let fixture = Fixture::new(messages, 1, true).await;
        telemetry::isolated_test(async {
            let page = fixture.store.agent_log_transcript_page(&fixture.agent,&first(32)).await.unwrap().unwrap();
            let transcript_counts = telemetry::snapshot(AGENT_LOG);
            assert!(transcript_counts[2] <= 72, "{transcript_counts:?}");
            assert!(page.next.is_some());
            let mut expected = fixture.expected().turns.remove(0);
            expected.messages.retain(|row| row.sequence <= 34);
            expected.agent_messages = expected.messages.iter().filter(|row| row.role == layer2_core::sandbox::AgentTranscriptRole::Agent).map(|row| row.content.clone()).collect();
            assert_eq!(page.transcript.turns,vec![expected]);
            let usage = fixture.store.agent_log_usage_projection(&fixture.agent).await.unwrap().unwrap();
            let total = telemetry::snapshot(AGENT_LOG);
            assert_eq!(total[2] - transcript_counts[2],2, "tail plus cumulative observation");
            assert_eq!(usage.usage.input_tokens,123);
            assert_eq!(usage.through_seq,fixture.agent.conversation.last_seq);
            let commands = fixture.commands.lock().unwrap().clone();
            assert!(commands.iter().all(|command| !["insert","update","delete","createIndexes","commitTransaction","abortTransaction"].iter().any(|key| command.contains_key(*key))));
            let mut examined = 0_i64;
            let mut keys = 0_i64;
            for command in commands.iter().filter(|cmd| cmd.contains_key("find")) {
                let mut query = command.clone();
                for key in ["lsid","$clusterTime","$db","readConcern","$readPreference"] { query.remove(key); }
                let explain = fixture.client.database(&fixture.name).run_command(doc! {"explain":query,"verbosity":"executionStats"}).await.unwrap();
                let stats = explain.get_document("executionStats").unwrap();
                let number = |key| stats.get_i64(key).or_else(|_| stats.get_i32(key).map(i64::from)).unwrap();
                examined += number("totalDocsExamined");
                keys += number("totalKeysExamined");
            }
            assert!(examined <= 72, "{examined}");
            assert!(keys <= 100, "{keys}");
            eprintln!("opening history={} transcript_documents={} transcript_bytes={} usage_documents={} usage_bytes={} all_find_documents_examined={examined} all_find_keys_examined={keys} command_count={} index_inventory_commands={}", fixture.entries.len(), transcript_counts[2], transcript_counts[3],total[2]-transcript_counts[2],total[3]-transcript_counts[3], commands.len(), commands.iter().filter(|row| row.contains_key("listIndexes")).count());
        }).await;
        fixture.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn opening_pages_join_to_complete_output_and_usage_reads_run_independently() {
    let mut fixture = Fixture::new(90, 2, true).await;
    fixture.append_turn(false).await;
    fixture.append_turn(true).await;
    for limit in [1, 7, 32] {
        let mut request = first(limit);
        let mut joined = fixture.expected();
        joined.turns.clear();
        loop {
            let (page, usage) = tokio::join!(
                fixture
                    .store
                    .agent_log_transcript_page(&fixture.agent, &request),
                fixture.store.agent_log_usage_projection(&fixture.agent)
            );
            let page = page.unwrap().unwrap();
            assert_eq!(usage.unwrap().unwrap().usage.input_tokens, 123);
            for mut turn in page.transcript.turns {
                if let Some(last) = joined
                    .turns
                    .last_mut()
                    .filter(|last| last.turn_sequence == turn.turn_sequence)
                {
                    last.messages.append(&mut turn.messages);
                    last.agent_messages.append(&mut turn.agent_messages);
                } else {
                    joined.turns.push(turn);
                }
            }
            match page.next {
                Some(next) => request = next,
                None => break,
            }
        }
        assert_eq!(joined, fixture.expected());
    }
    let commands = fixture.commands.lock().unwrap().clone();
    let sessions: std::collections::HashSet<_> = commands
        .iter()
        .filter_map(|row| row.get_document("lsid").ok())
        .map(|id| format!("{id:?}"))
        .collect();
    assert!(
        sessions.len() >= 2,
        "parallel reads use independent sessions"
    );
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn opening_missing_prerequisites_preserve_compatibility_and_damaged_proof_refuses() {
    let mut fixture = Fixture::new(50, 1, true).await;
    fixture.store = fixture
        .store
        .clone()
        .with_bounded_transcript_reads(false)
        .with_bounded_usage_reads(false);
    assert!(fixture
        .store
        .agent_log_transcript_page(&fixture.agent, &first(32))
        .await
        .unwrap()
        .is_none());
    assert!(fixture
        .store
        .agent_log_usage_projection(&fixture.agent)
        .await
        .unwrap()
        .is_none());
    fixture.store = fixture
        .store
        .clone()
        .with_bounded_transcript_reads(true)
        .with_bounded_usage_reads(true);
    fixture
        .store
        .agent_log
        .drop_index("agent_log_kind_sequence")
        .await
        .unwrap();
    assert!(fixture
        .store
        .agent_log_transcript_page(&fixture.agent, &first(32))
        .await
        .unwrap()
        .is_none());
    assert!(fixture
        .store
        .agent_log_usage_projection(&fixture.agent)
        .await
        .unwrap()
        .is_none());
    fixture.install_index().await;
    let mut stale = fixture.agent.clone();
    stale.owner = SubjectId::new("other");
    assert!(fixture
        .store
        .agent_log_transcript_page(&stale, &first(32))
        .await
        .is_err());
    assert!(fixture
        .store
        .agent_log_usage_projection(&stale)
        .await
        .is_err());
    let mut request = first(32);
    request.turn = Some(3);
    request.after = Some(4);
    request.through = Some(5);
    assert!(fixture
        .store
        .agent_log_transcript_page(&fixture.agent, &request)
        .await
        .is_err());
    fixture.store.agent_log.delete_one(doc! {"agent_id":fixture.agent.id.as_str(),"seq":fixture.agent.conversation.last_seq as i64}).await.unwrap();
    assert!(fixture
        .store
        .agent_log_transcript_page(&fixture.agent, &first(32))
        .await
        .is_err());
    assert!(fixture
        .store
        .agent_log_usage_projection(&fixture.agent)
        .await
        .is_err());
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn opening_missing_summary_and_excessive_gap_evidence_keep_full_fallback() {
    let fixture = Fixture::new(50, 33, false).await;
    assert!(fixture
        .store
        .agent_log_usage_projection(&fixture.agent)
        .await
        .unwrap()
        .is_none());
    assert!(fixture
        .store
        .agent_log_transcript_page(&fixture.agent, &first(32))
        .await
        .unwrap()
        .is_none());
    assert!(fixture
        .store
        .agent_log_transcript(&fixture.agent)
        .await
        .unwrap()
        .is_some());
    fixture
        .store
        .agents
        .update_one(
            doc! {"_id":fixture.agent.id.as_str()},
            doc! {"$unset":{INTEGRITY_FIELD:""}},
        )
        .await
        .unwrap();
    assert!(fixture
        .store
        .agent_log_usage_projection(&fixture.agent)
        .await
        .unwrap()
        .is_none());
    assert!(fixture
        .store
        .agent_log_transcript_page(&fixture.agent, &first(32))
        .await
        .unwrap()
        .is_none());
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn opening_byte_budget_refuses_oversized_response_and_accepts_smaller_window() {
    let fixture = Fixture::with_payload(32, 0, true, 200_000).await;
    assert_eq!(
        fixture
            .store
            .agent_log_transcript_page(&fixture.agent, &first(32))
            .await,
        Err(SandboxStoreError::AgentLogCapacityExceeded)
    );
    let small = fixture
        .store
        .agent_log_transcript_page(&fixture.agent, &first(8))
        .await
        .unwrap()
        .unwrap();
    assert!(small.next.is_some());
    assert!(serde_json::to_vec(&small).unwrap().len() < 4 * 1024 * 1024);
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn opening_excluded_cohort_adds_no_optional_log_or_index_reads() {
    let mut fixture = Fixture::new(50, 1, true).await;
    fixture.store.bounded_admission_agents.clear();
    fixture.store = fixture.store.with_fleet_log_percentage(0);
    assert!(fixture
        .store
        .agent_log_transcript_page(&fixture.agent, &first(32))
        .await
        .unwrap()
        .is_none());
    assert!(fixture
        .store
        .agent_log_usage_projection(&fixture.agent)
        .await
        .unwrap()
        .is_none());
    let commands = fixture.commands.lock().unwrap().clone();
    assert_eq!(commands.len(), 2, "existing format dispatch only");
    assert!(commands
        .iter()
        .all(|row| row.get_str("find").ok() == Some(AGENTS)));
    fixture.cleanup().await;
}
