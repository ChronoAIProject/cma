//! Reproducible M-STAB-03 measurement overlay; no production policy changes.
use super::*;
use crate::store::agent_log_segments::{writer_control, writer_gate};
use sha2::{Digest, Sha256};
use std::time::Instant;

fn emit(value: serde_json::Value) {
    println!("HISTORY_SCALE {}", value);
}

fn add_hash(hash: &mut Sha256, rows: &[AgentLogEntry]) {
    for row in rows {
        let bytes = serde_json::to_vec(row).unwrap();
        hash.update((bytes.len() as u64).to_be_bytes());
        hash.update(bytes);
    }
}

async fn head(store: &MongoSandboxStore, id: &str) -> Document {
    store
        .client
        .database(&store.agents.namespace().db)
        .collection::<Document>(segments::HEADS)
        .find_one(doc! {"agent_id": id})
        .await
        .unwrap()
        .unwrap()
}

async fn gate(store: &MongoSandboxStore, id: &str, revision: i64, enabled: bool) {
    writer_control::execute(
        &store.client,
        &store.client.database(&store.agents.namespace().db),
        writer_gate::GateName::Segments,
        writer_control::Action::Replace {
            expected_revision: revision,
            policy: writer_gate::Policy {
                enabled,
                agents: vec![id.into()],
                ..Default::default()
            },
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "owned local kind replica set; M-STAB-03 full scale measurement"]
async fn same_agent_100k_640mib_continuation_replay_and_settlement() {
    let start = Instant::now();
    let (client, name, mut store, mut current, command) = working("history_scale").await;
    let target: u64 = std::env::var("HISTORY_SCALE_EVENTS")
        .unwrap_or("100000".into())
        .parse()
        .unwrap();
    let initial_seq = current.conversation.last_seq;
    let initial = store.agent_log(&current.id, 0, 256).await.unwrap();
    assert_eq!(initial.len() as u64, initial_seq);
    let mut expected = Sha256::new();
    add_hash(&mut expected, &initial);
    emit(
        serde_json::json!({"stage":"setup","database":name,"agent":current.id,
        "initial_seq":initial_seq,"target_added_events":target,"content_bytes":7168,
        "seed_method":"real_transaction_batches_64","pid":std::process::id()}),
    );
    let content = "h".repeat(7168);
    let mut added = 0_u64;
    let mut boundary_probed = false;
    while added < target {
        let n = (target - added).min(64);
        let before = head(&store, current.id.as_str()).await;
        let mut mutation = output(&current, &content);
        mutation.agent.conversation.last_seq = current.conversation.last_seq + n;
        mutation.agent_logs = (0..n)
            .map(|i| AgentLogKind::AgentMessage {
                // Same fixed byte length, distinct deterministic payload per event.
                content: format!("{:016x}{}", added + i, &content[16..]),
            })
            .collect();
        let expected_rows = conversation::conversation_logs(&mutation).unwrap();
        let batch_bytes: usize = expected_rows
            .iter()
            .map(|r| {
                mongodb::bson::serialize_to_vec(&crate::store::agent_log::log_document(r).unwrap())
                    .unwrap()
                    .len()
            })
            .sum();
        if !boundary_probed
            && before.get_i64("active_bytes").unwrap() + batch_bytes as i64 > 64 * 1024 * 1024
        {
            gate(&store, current.id.as_str(), 1, false).await;
            let denied_at = Instant::now();
            let denied = store.commit_agent_conversation(mutation.clone()).await;
            let elapsed = denied_at.elapsed().as_secs_f64() * 1000.0;
            assert!(
                matches!(denied, Err(SandboxStoreError::Unavailable(_))),
                "{denied:?}"
            );
            assert_eq!(head(&store, current.id.as_str()).await, before);
            assert_eq!(
                store.get_agent(&current.id).await.unwrap(),
                Some(current.clone())
            );
            emit(
                serde_json::json!({"stage":"closed_rollover_boundary","elapsed_ms":elapsed,
                "result":format!("{denied:?}"),"unchanged_head":true,"unchanged_agent":true,
                "seq":current.conversation.last_seq,"active_bytes":before.get_i64("active_bytes").unwrap(),
                "automatic_80_percent_protection":false}),
            );
            gate(&store, current.id.as_str(), 2, true).await;
            boundary_probed = true;
        }
        let at = Instant::now();
        let written = store
            .commit_agent_conversation(mutation.clone())
            .await
            .unwrap();
        let elapsed = at.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(written, expected_rows);
        add_hash(&mut expected, &written);
        current = mutation.agent.clone();
        added += n;
        let after = head(&store, current.id.as_str()).await;
        emit(serde_json::json!({"stage":"write","events":n,"added":added,
            "seq":current.conversation.last_seq,"elapsed_ms":elapsed,
            "lifetime_bytes":after.get_i64("lifetime_bytes").unwrap(),
            "active_bytes":after.get_i64("active_bytes").unwrap(),
            "active_count":after.get_i64("active_count").unwrap(),
            "segment":after.get_i64("active_ordinal").unwrap(),
            "wall_seconds":start.elapsed().as_secs_f64()}));
        if added == 64 || (added - n) / 10000 < added / 10000 {
            let at = Instant::now();
            let replayed = store.commit_agent_conversation(mutation).await.unwrap();
            assert_eq!(replayed, written);
            assert_eq!(head(&store, current.id.as_str()).await, after);
            emit(
                serde_json::json!({"stage":"write_idempotent_replay","added":added,
                "elapsed_ms":at.elapsed().as_secs_f64()*1000.0,"no_duplicate":true}),
            );
        }
    }
    let sealed = head(&store, current.id.as_str()).await;
    if target >= 100000 {
        assert!(sealed.get_i64("lifetime_bytes").unwrap() >= 640 * 1024 * 1024);
        assert!(boundary_probed);
    }
    let reopen = Instant::now();
    store = MongoSandboxStore::open(&client, &name).await.unwrap();
    emit(serde_json::json!({"stage":"reopen","elapsed_ms":reopen.elapsed().as_secs_f64()*1000.0}));
    let replay_at = Instant::now();
    let mut replay_hash = Sha256::new();
    let mut seq = 0;
    while seq < current.conversation.last_seq {
        let at = Instant::now();
        let page = store
            .agent_log_range(&current.id, seq, current.conversation.last_seq, 128)
            .await
            .unwrap();
        let elapsed = at.elapsed().as_secs_f64() * 1000.0;
        assert!(!page.is_empty());
        for row in &page {
            seq += 1;
            assert_eq!(row.seq, seq);
            assert_eq!(row.agent_id, current.id);
        }
        add_hash(&mut replay_hash, &page);
        emit(
            serde_json::json!({"stage":"replay_page","through":seq,"events":page.len(),"elapsed_ms":elapsed}),
        );
    }
    let expected_hash = format!("{:x}", expected.finalize());
    let actual_hash = format!("{:x}", replay_hash.finalize());
    assert_eq!(actual_hash, expected_hash);
    emit(serde_json::json!({"stage":"replay_complete","events":seq,
        "elapsed_seconds":replay_at.elapsed().as_secs_f64(),"sha256":actual_hash,
        "expected_sha256":expected_hash,"gaps":0,"duplicates":0,
        "lifetime_bytes":sealed.get_i64("lifetime_bytes").unwrap()}));
    // Runtime continuation after reopening, then the actual settlement root.
    let recorder = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    let finish = Instant::now();
    recorder
        .record_update(
            &current.id,
            1,
            &mut AgentMessageAssembly::default(),
            chunk("history scale continuation"),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let result = recorder
        .record_update(
            &current.id,
            1,
            &mut AgentMessageAssembly::default(),
            layer3_adapter::ConversationUpdate::TurnEnded(layer3_adapter::PromptResponse::new(
                layer3_adapter::TurnStopReason::EndTurn,
            )),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    assert!(result.turn_closed);
    let settled = store
        .reconcile_command(&command.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(settled.status, AgentResponseStatus::Completed);
    assert!(settled.work.lease.is_none());
    assert_eq!(
        store
            .reconcile_command(&command.receipt.envelope.command_id)
            .await
            .unwrap(),
        settled
    );
    let finished = store.get_agent(&current.id).await.unwrap().unwrap();
    assert_eq!(finished.status, AgentStatus::Ready);
    let tail = store
        .agent_log(&current.id, current.conversation.last_seq, 256)
        .await
        .unwrap();
    assert!(tail.iter().any(|r| matches!(&r.kind, AgentLogKind::AgentMessage {content} if content == "history scale continuation")));
    assert_eq!(
        finished.conversation.last_seq,
        current.conversation.last_seq + tail.len() as u64
    );
    emit(
        serde_json::json!({"stage":"settled","elapsed_ms":finish.elapsed().as_secs_f64()*1000.0,
        "status":"Completed","command_lease":null,"agent_status":"Ready",
        "seq":finished.conversation.last_seq,"tail_events":tail.len(),
        "wall_seconds":start.elapsed().as_secs_f64()}),
    );
    client.database(&name).drop().await.unwrap();
    emit(serde_json::json!({"stage":"cleanup","database_dropped":name}));
}
