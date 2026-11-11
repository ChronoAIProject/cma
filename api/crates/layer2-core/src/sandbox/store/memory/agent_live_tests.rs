use super::*;
use crate::sandbox::conversation_recorder::live::LIVE_BUFFER_FRAGMENTS;
use crate::sandbox::AgentLiveFragmentKind;
use tokio::sync::broadcast::error::TryRecvError;

#[tokio::test]
async fn subscription_precedes_admission_and_survives_turns_without_persisting_fragments() {
    let (store, ready) = ready_agent(None).await;
    let recorder = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    let mut live = recorder.subscribe_live(&ready.id).unwrap();
    let mut unrelated = recorder
        .subscribe_live(&crate::AgentId("agt_unrelated".into()))
        .unwrap();
    let mutation = begin_turn(&store, ready);
    let working = mutation.agent.clone();
    store.commit_agent_conversation(mutation).await.unwrap();
    let mut assembly = AgentMessageAssembly::default();
    for text in ["Hello ", "world"] {
        let outcome = recorder
            .record_update(
                &working.id,
                1,
                &mut assembly,
                message_chunk("reply", text),
                NOW.into(),
            )
            .await
            .unwrap();
        assert!(outcome.appended.is_empty());
    }
    recorder
        .record_update(
            &working.id,
            1,
            &mut assembly,
            ConversationUpdate::Said(SessionUpdate::AgentThoughtChunk(ContentChunk::new(
                ContentBlock::from("private thought"),
            ))),
            NOW.into(),
        )
        .await
        .unwrap();
    let first = live.try_recv().unwrap();
    let second = live.try_recv().unwrap();
    let thought = live.try_recv().unwrap();
    assert_eq!(first.agent_id, working.id);
    assert_eq!(first.runtime_generation, 1);
    assert_eq!(first.after_seq, working.conversation.last_seq);
    assert_eq!(Some(first.turn_sequence), working.conversation.active_turn);
    assert_eq!(first.message_id, second.message_id);
    assert_eq!(first.kind, AgentLiveFragmentKind::Message);
    assert_eq!(first.delta, "Hello ");
    assert_eq!(second.delta, "world");
    assert_eq!(thought.kind, AgentLiveFragmentKind::Thought);
    assert_ne!(first.message_id, thought.message_id);
    assert!(!format!("{thought:?}").contains("private thought"));
    assert_eq!(unrelated.try_recv(), Err(TryRecvError::Empty));
    assert_eq!(
        store.agent_log(&working.id, 0, 200).await.unwrap().len() as u64,
        working.conversation.last_seq
    );
    let ended = recorder
        .record_update(&working.id, 1, &mut assembly, turn_ended(), NOW.into())
        .await
        .unwrap();
    assert!(ended.appended.iter().any(|entry| matches!(&entry.kind,
        AgentLogKind::AgentMessage { content } if content == "Hello world")));
    let ready = store.get_agent(&working.id).await.unwrap().unwrap();
    let mutation = begin_turn(&store, ready);
    let next_turn = mutation.agent.conversation.active_turn.unwrap();
    store.commit_agent_conversation(mutation).await.unwrap();
    recorder
        .record_update(
            &working.id,
            1,
            &mut assembly,
            message_chunk("reply", "next turn"),
            NOW.into(),
        )
        .await
        .unwrap();
    let next = live.try_recv().unwrap();
    assert_eq!(next.turn_sequence, next_turn);
    assert_ne!(next.turn_sequence, first.turn_sequence);
    assert_ne!(next.message_id, first.message_id);
}

#[tokio::test]
async fn unseen_message_identity_survives_the_first_evidence_commit() {
    let (store, ready) = ready_agent(Some("Inspect the proof.")).await;
    let mutation = begin_first_message(&store, ready);
    let working = mutation.agent.clone();
    store.commit_agent_conversation(mutation).await.unwrap();
    let recorder = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    let mut live = recorder.subscribe_live(&working.id).unwrap();
    let mut assembly = AgentMessageAssembly::default();
    for text in ["first ", "second"] {
        recorder
            .record_update(
                &working.id,
                1,
                &mut assembly,
                ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(ContentChunk::new(
                    ContentBlock::from(text),
                ))),
                NOW.into(),
            )
            .await
            .unwrap();
    }
    let first = live.try_recv().unwrap();
    let second = live.try_recv().unwrap();
    assert_eq!(first.message_id, second.message_id);
    assert_eq!(first.delta, "first ");
    assert_eq!(second.delta, "second");
    let committed_seq = store
        .get_agent(&working.id)
        .await
        .unwrap()
        .unwrap()
        .conversation
        .last_seq;
    assert!(committed_seq > working.conversation.last_seq);
    assert_eq!(first.after_seq, committed_seq);
    assert_eq!(second.after_seq, committed_seq);
    assert_eq!(live.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
async fn stale_and_rejected_updates_never_emit_live_fragments() {
    let (store, working, recorder) = working_agent().await;
    let mut live = recorder.subscribe_live(&working.id).unwrap();
    let mut assembly = AgentMessageAssembly::default();
    assert_eq!(
        recorder
            .record_update(
                &working.id,
                2,
                &mut assembly,
                message_chunk("stale", "stale"),
                NOW.into()
            )
            .await,
        Err(AgentConversationError::StaleGeneration)
    );
    assert_eq!(live.try_recv(), Err(TryRecvError::Empty));
    recorder
        .record_update(
            &working.id,
            1,
            &mut assembly,
            message_chunk("accepted", "accepted"),
            NOW.into(),
        )
        .await
        .unwrap();
    live.try_recv().unwrap();
    let before = assembly.clone();
    store
        .lock()
        .agent_logs
        .get_mut(&working.id)
        .unwrap()
        .push(AgentLogEntry {
            agent_id: working.id.clone(),
            seq: working.conversation.last_seq + 1,
            appended_at: NOW.into(),
            kind: AgentLogKind::ConversationGap { frames: 1 },
        });
    assert!(matches!(
        recorder
            .record_update(
                &working.id,
                1,
                &mut assembly,
                message_chunk("rejected", "rejected"),
                NOW.into()
            )
            .await,
        Err(AgentConversationError::Corrupt(_))
    ));
    assert_eq!(assembly, before);
    assert_eq!(live.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
async fn multiple_subscribers_receive_identical_utf8_bounded_fragments() {
    let (_, working, recorder) = working_agent().await;
    let mut first = recorder.subscribe_live(&working.id).unwrap();
    let mut second = recorder.subscribe_live(&working.id).unwrap();
    let delta = "\u{20ac}".repeat(7_000);
    recorder
        .record_update(
            &working.id,
            1,
            &mut AgentMessageAssembly::default(),
            message_chunk("bounded", &delta),
            NOW.into(),
        )
        .await
        .unwrap();
    let mut assembled = String::new();
    let mut identity = None;
    while let Ok(fragment) = first.try_recv() {
        assert!(fragment.delta.len() <= 16 * 1024);
        assert_eq!(second.recv().await.unwrap(), fragment);
        if let Some(identity) = &identity {
            assert_eq!(identity, &fragment.message_id);
        } else {
            identity = Some(fragment.message_id.clone());
        }
        assembled.push_str(&fragment.delta);
    }
    assert_eq!(assembled, delta);
    assert_eq!(second.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
async fn idle_thoughts_emit_nothing_and_per_agent_lag_is_explicit() {
    let (store, ready) = ready_agent(None).await;
    let recorder = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    let mut live = recorder.subscribe_live(&ready.id).unwrap();
    let mut unrelated = recorder
        .subscribe_live(&crate::AgentId("agt_unrelated".into()))
        .unwrap();
    let mut assembly = AgentMessageAssembly::default();
    recorder
        .record_update(
            &ready.id,
            1,
            &mut assembly,
            ConversationUpdate::Said(SessionUpdate::AgentThoughtChunk(ContentChunk::new(
                ContentBlock::from("idle"),
            ))),
            NOW.into(),
        )
        .await
        .unwrap();
    assert_eq!(live.try_recv(), Err(TryRecvError::Empty));
    store
        .commit_agent_conversation(begin_turn(&store, ready.clone()))
        .await
        .unwrap();
    for _ in 0..(LIVE_BUFFER_FRAGMENTS + 8) {
        recorder
            .record_update(
                &ready.id,
                1,
                &mut assembly,
                message_chunk("reply", "fragment"),
                NOW.into(),
            )
            .await
            .unwrap();
    }
    assert_eq!(live.try_recv(), Err(TryRecvError::Lagged(8)));
    assert!(live.try_recv().is_ok());
    assert_eq!(unrelated.try_recv(), Err(TryRecvError::Empty));
}
