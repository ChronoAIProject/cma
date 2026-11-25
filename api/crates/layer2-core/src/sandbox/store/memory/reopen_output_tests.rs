use super::*;
use crate::sandbox::store::{
    AgentConversationStore, AgentMessageCheckpoint, AgentReopenGuard, AgentStore,
};
use crate::sandbox::{
    AgentConversationError, AgentMessageAssembly, AgentUpdateOutcome, AgentUpdateRecorder,
    DurableAgentUpdateRecorder,
};
use layer3_adapter::{ConversationUpdate, RuntimeOutputReceipt, RuntimeOutputSource};

async fn fixture() -> (ValidationFixture, AgentReopenGuard) {
    let mut parent = claimed_revive_phase(SandboxRevivePhase::ConversationReopen);
    parent.work.lease.as_mut().unwrap().expires_at =
        crate::rfc3339_at(std::time::SystemTime::now() + std::time::Duration::from_secs(3600));
    let agent = test_agent("agt_reopen_output", AgentStatus::Paused);
    let guard = AgentReopenGuard {
        owner_restart: None,
        sandbox: SandboxMutationGuard::capture(&parent),
        agent_fence: agent.work.next_fence,
        native_conversation_id: agent.conversation.conversation_id.clone().unwrap(),
        runtime_generation: 5,
    };
    (seed_fixture(parent, vec![agent]).await, guard)
}

fn output(
    guard: &AgentReopenGuard,
    cursor: u64,
    update: Option<ConversationUpdate>,
) -> ConversationUpdate {
    let (acks, _) = tokio::sync::mpsc::channel(1);
    ConversationUpdate::RuntimeOutput {
        receipt: RuntimeOutputReceipt::new(
            RuntimeOutputSource {
                stream_id: "d99003d1-0c3c-46cd-aa47-9bbdbed50b12".into(),
                cursor,
                native_conversation_id: guard.native_conversation_id.clone(),
                payload_sha256: format!("{cursor:064x}"),
            },
            acks,
        ),
        update: update.map(|update| Box::new(update).into()),
    }
}

#[tokio::test]
async fn reopen_receipts_survive_failed_handshake_retry_and_final_publication() {
    let (fixture, guard) = fixture().await;
    let original = &fixture.agents[0];
    let recorder = DurableAgentUpdateRecorder::new(Arc::new(fixture.store.clone()));
    let mut assembly = AgentMessageAssembly::default();
    let historical = ConversationUpdate::Said(layer3_adapter::SessionUpdate::AgentMessageChunk(
        layer3_adapter::ContentChunk::new(layer3_adapter::ContentBlock::from(
            "already saved history",
        )),
    ))
    .without_prompt_evidence();
    recorder
        .record_reopen_update(
            &original.id,
            &guard,
            &mut assembly,
            output(&guard, 1, Some(historical)),
            NOW.into(),
        )
        .await
        .unwrap();
    let persisted = fixture
        .store
        .get_agent(&original.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        persisted, *original,
        "output must not publish readiness or generation"
    );
    let receipt = fixture
        .store
        .agent_message_checkpoint(&original.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.runtime_generation, 5);
    assert_eq!(receipt.output.as_ref().unwrap().cursor, 1);
    assert!(fixture
        .store
        .agent_log(&original.id, 0, 200)
        .await
        .unwrap()
        .is_empty());
    assert!(matches!(
        recorder
            .record_update(
                &original.id,
                4,
                &mut AgentMessageAssembly::default(),
                output(&guard, 2, None),
                NOW.into()
            )
            .await,
        Err(AgentConversationError::StaleGeneration)
    ));

    // A failed handshake loses all local state. A replacement recorder reuses
    // the durable receipt and never appends duplicate historical messages.
    let replacement = DurableAgentUpdateRecorder::new(Arc::new(fixture.store.clone()));
    replacement
        .record_reopen_update(
            &original.id,
            &guard,
            &mut AgentMessageAssembly::default(),
            output(&guard, 1, None),
            NOW.into(),
        )
        .await
        .unwrap();
    assert_eq!(
        fixture
            .store
            .agent_message_checkpoint(&original.id)
            .await
            .unwrap(),
        Some(receipt)
    );
    replacement
        .record_reopen_update(
            &original.id,
            &guard,
            &mut AgentMessageAssembly::default(),
            output(&guard, 2, None),
            NOW.into(),
        )
        .await
        .unwrap();
    fixture
        .store
        .commit_sandbox_lifecycle(reopen_mutation(&fixture.current, &fixture.agents))
        .await
        .unwrap();
    let ready = fixture
        .store
        .get_agent(&original.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ready.status, AgentStatus::Ready);
    assert_eq!(ready.conversation.runtime_generation, Some(5));
    let checkpoint = fixture
        .store
        .agent_message_checkpoint(&original.id)
        .await
        .unwrap()
        .unwrap();
    assert!(checkpoint.matches(&ready));
    assert_eq!(checkpoint.output.as_ref().unwrap().cursor, 2);
    replacement
        .record_update(
            &original.id,
            5,
            &mut AgentMessageAssembly::default(),
            output(&guard, 3, None),
            NOW.into(),
        )
        .await
        .unwrap();
    assert_eq!(
        fixture
            .store
            .agent_message_checkpoint(&original.id)
            .await
            .unwrap()
            .unwrap()
            .output
            .unwrap()
            .cursor,
        3
    );
    assert_eq!(
        fixture
            .store
            .agent_log(&original.id, 0, 200)
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn stale_reopen_lease_identity_and_epoch_cannot_record_or_reacknowledge() {
    let (fixture, guard) = fixture().await;
    let agent = &fixture.agents[0];
    let recorder = DurableAgentUpdateRecorder::new(Arc::new(fixture.store.clone()));
    recorder
        .record_reopen_update(
            &agent.id,
            &guard,
            &mut AgentMessageAssembly::default(),
            output(&guard, 1, None),
            NOW.into(),
        )
        .await
        .unwrap();
    let durable = fixture
        .store
        .agent_message_checkpoint(&agent.id)
        .await
        .unwrap();
    for change in 0..4 {
        let mut stale = guard.clone();
        match change {
            0 => stale.sandbox.lease.as_mut().unwrap().fence += 1,
            1 => stale.native_conversation_id = "other-original".into(),
            2 => stale.runtime_generation += 1,
            _ => stale.agent_fence += 1,
        }
        assert!(recorder
            .record_reopen_update(
                &agent.id,
                &stale,
                &mut AgentMessageAssembly::default(),
                output(&guard, 1, None),
                NOW.into()
            )
            .await
            .is_err());
        assert_eq!(
            fixture
                .store
                .agent_message_checkpoint(&agent.id)
                .await
                .unwrap(),
            durable
        );
    }
    // Takeover after the first ACK: even a duplicate cursor requires the current
    // parent fence, so a replaced worker cannot claim ongoing authority.
    fixture
        .store
        .lock()
        .sandboxes
        .get_mut(&fixture.current.id)
        .unwrap()
        .work
        .lease
        .as_mut()
        .unwrap()
        .fence += 1;
    assert!(recorder
        .record_reopen_update(
            &agent.id,
            &guard,
            &mut AgentMessageAssembly::default(),
            output(&guard, 1, None),
            NOW.into()
        )
        .await
        .is_err());
    assert_eq!(
        fixture
            .store
            .agent_message_checkpoint(&agent.id)
            .await
            .unwrap(),
        durable
    );
}

#[tokio::test]
async fn reopen_metadata_commits_in_order_without_publishing_the_generation() {
    let (fixture, guard) = fixture().await;
    let original = &fixture.agents[0];
    let recorder = DurableAgentUpdateRecorder::new(Arc::new(fixture.store.clone()));
    let metadata = ConversationUpdate::Metadata(layer3_adapter::ConversationMetadata {
        description: Some("Retained conversation title".into()),
        ..Default::default()
    });
    let mut assembly = AgentMessageAssembly::default();
    recorder
        .record_reopen_update(
            &original.id,
            &guard,
            &mut assembly,
            output(&guard, 1, Some(metadata.clone())),
            NOW.into(),
        )
        .await
        .unwrap();
    let current = fixture
        .store
        .get_agent(&original.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.status, AgentStatus::Paused);
    assert_eq!(current.conversation.runtime_generation, Some(4));
    assert!(current.conversation.last_seq > original.conversation.last_seq);
    let logs = fixture.store.agent_log(&original.id, 0, 200).await.unwrap();
    recorder
        .record_reopen_update(
            &original.id,
            &guard,
            &mut assembly,
            output(&guard, 1, Some(metadata)),
            NOW.into(),
        )
        .await
        .unwrap();
    assert_eq!(
        fixture.store.agent_log(&original.id, 0, 200).await.unwrap(),
        logs
    );
    fixture
        .store
        .commit_sandbox_lifecycle(reopen_mutation(
            &fixture.current,
            std::slice::from_ref(&current),
        ))
        .await
        .unwrap();
    let ready = fixture
        .store
        .get_agent(&original.id)
        .await
        .unwrap()
        .unwrap();
    assert!(fixture
        .store
        .agent_message_checkpoint(&original.id)
        .await
        .unwrap()
        .unwrap()
        .matches(&ready));
}

#[tokio::test]
async fn source_less_reopen_metadata_drops_the_prior_generation_receipt() {
    let (fixture, guard) = fixture().await;
    let original = &fixture.agents[0];
    let old_receipt = layer3_adapter::RuntimeOutputSource {
        stream_id: "d99003d1-0c3c-46cd-aa47-9bbdbed50b12".into(),
        cursor: 9,
        native_conversation_id: guard.native_conversation_id.clone(),
        payload_sha256: "a".repeat(64),
    };
    fixture.store.lock().agent_message_checkpoints.insert(
        original.id.clone(),
        AgentMessageCheckpoint {
            version: 1,
            update_id: uuid::Uuid::new_v4().to_string(),
            update_fingerprint: "b".repeat(64),
            native_conversation_id: guard.native_conversation_id.clone(),
            runtime_generation: 4,
            active_turn: None,
            after_seq: original.conversation.last_seq,
            assembly: AgentMessageAssembly::default(),
            outcome: AgentUpdateOutcome {
                appended: vec![],
                turn_closed: false,
            },
            output: Some(old_receipt),
        },
    );
    let recorder = DurableAgentUpdateRecorder::new(Arc::new(fixture.store.clone()));
    recorder
        .record_reopen_update(
            &original.id,
            &guard,
            &mut AgentMessageAssembly::default(),
            ConversationUpdate::SignInRequired {
                url: "https://example.invalid/sign-in".into(),
                code: None,
                message: None,
            },
            NOW.into(),
        )
        .await
        .unwrap();
    let reopened = fixture
        .store
        .get_agent(&original.id)
        .await
        .unwrap()
        .unwrap();
    fixture
        .store
        .commit_sandbox_lifecycle(reopen_mutation(&fixture.current, &[reopened]))
        .await
        .unwrap();
    let ready = fixture
        .store
        .get_agent(&original.id)
        .await
        .unwrap()
        .unwrap();
    let checkpoint = fixture
        .store
        .agent_message_checkpoint(&original.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint.runtime_generation, 5);
    assert_eq!(checkpoint.output, None);
    let replacement = DurableAgentUpdateRecorder::new(Arc::new(fixture.store.clone()));
    replacement
        .record_update(
            &original.id,
            5,
            &mut AgentMessageAssembly::default(),
            output(&guard, 1, None),
            NOW.into(),
        )
        .await
        .unwrap();
    assert_eq!(
        fixture
            .store
            .agent_message_checkpoint(&ready.id)
            .await
            .unwrap()
            .unwrap()
            .output
            .unwrap()
            .cursor,
        1
    );
}
