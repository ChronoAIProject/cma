use super::*;

fn identity_terminal(agent: &crate::sandbox::Agent) -> ConversationUpdate {
    ConversationUpdate::TurnEndedForPrompt {
        prompt_id: layer3_adapter::PromptIdentity::new(
            agent.conversation.conversation_id.as_deref().unwrap(),
            agent.conversation.runtime_generation.unwrap(),
            agent.conversation.active_turn.unwrap(),
        ),
        response: layer3_adapter::PromptResponse::new(layer3_adapter::TurnStopReason::EndTurn),
    }
}

#[tokio::test]
async fn identity_terminal_rescues_a_quiet_receipt_in_both_observer_orders() {
    for terminal_first in [false, true] {
        let (store, working, recorder) = working_agent().await;
        let mut assembly = AgentMessageAssembly::default();
        recorder
            .record_update(&working.id, 1, &mut assembly, retained(1, None), NOW.into())
            .await
            .unwrap();
        let terminal = retained(2, Some(identity_terminal(&working)));
        let quiet_terminal = retained(2, None);
        if terminal_first {
            let outcome = recorder
                .record_update(&working.id, 1, &mut assembly, terminal, NOW.into())
                .await
                .unwrap();
            assert!(outcome.turn_closed);
            recorder
                .record_update(
                    &working.id,
                    1,
                    &mut AgentMessageAssembly::default(),
                    quiet_terminal,
                    NOW.into(),
                )
                .await
                .unwrap();
        } else {
            let outcome = recorder
                .record_update(&working.id, 1, &mut assembly, quiet_terminal, NOW.into())
                .await
                .unwrap();
            assert!(!outcome.turn_closed);
            let outcome = recorder
                .record_update(&working.id, 1, &mut assembly, terminal, NOW.into())
                .await
                .unwrap();
            assert!(outcome.turn_closed);
        }
        let checkpoint = store
            .agent_message_checkpoint(&working.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(checkpoint.output.unwrap().cursor, 2);
        assert_eq!(
            store.get_agent(&working.id).await.unwrap().unwrap().status,
            AgentStatus::Ready
        );
    }
}

#[tokio::test]
async fn delayed_terminal_for_a_after_b_is_admitted_cannot_rewind_or_close_b() {
    let (store, working, recorder) = working_agent().await;
    let mut assembly = AgentMessageAssembly::default();
    recorder
        .record_update(&working.id, 1, &mut assembly, retained(1, None), NOW.into())
        .await
        .unwrap();
    recorder
        .record_update(
            &working.id,
            1,
            &mut assembly,
            retained(2, Some(identity_terminal(&working))),
            NOW.into(),
        )
        .await
        .unwrap();
    let ready = store.get_agent(&working.id).await.unwrap().unwrap();
    store
        .commit_agent_conversation(begin_turn(&store, ready))
        .await
        .unwrap();
    let b = store.get_agent(&working.id).await.unwrap().unwrap();
    assert_ne!(b.conversation.active_turn, working.conversation.active_turn);
    for cursor in [3, 4] {
        recorder
            .record_update(
                &working.id,
                1,
                &mut AgentMessageAssembly::default(),
                retained(cursor, None),
                NOW.into(),
            )
            .await
            .unwrap();
    }
    let before = store
        .agent_message_checkpoint(&working.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.output.as_ref().unwrap().cursor, 4);

    recorder
        .record_update(
            &working.id,
            1,
            &mut AgentMessageAssembly::default(),
            retained(2, Some(identity_terminal(&working))),
            NOW.into(),
        )
        .await
        .unwrap();
    let after = store
        .agent_message_checkpoint(&working.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.output.as_ref().unwrap().cursor, 4);
    assert_eq!(
        store.get_agent(&working.id).await.unwrap().unwrap(),
        b,
        "the delayed A terminal must not finish B"
    );
    let rejected = recorder
        .record_update(
            &working.id,
            1,
            &mut AgentMessageAssembly::default(),
            retained(5, Some(identity_terminal(&working))),
            NOW.into(),
        )
        .await;
    assert!(matches!(
        rejected,
        Err(AgentConversationError::InvalidState(_))
    ));
    assert_eq!(
        store
            .agent_message_checkpoint(&working.id)
            .await
            .unwrap()
            .unwrap()
            .output
            .unwrap()
            .cursor,
        4
    );
}
