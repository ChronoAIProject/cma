use super::*;

#[tokio::test]
async fn suggested_reply_admission_rejects_an_answer_after_another_tab_admits_input() {
    rejects_superseded_answer(false).await;
}

#[tokio::test]
async fn suggested_reply_does_not_reactivate_after_the_newer_admission_is_rejected() {
    rejects_superseded_answer(true).await;
}

async fn rejects_superseded_answer(reject_newer: bool) {
    let (store, service, _, mut current) = setup(Authority::new()).await;
    for kind in [
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: 2,
            origin_key: None,
        },
        AgentLogKind::OwnerMessage {
            content: "Compare two options.".into(),
        },
        AgentLogKind::AgentMessage {
            content: "The comparison is complete.".into(),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 2 },
    ] {
        store
            .append_agent_log(&current.id, NOW.into(), kind)
            .await
            .unwrap();
    }
    current.status = AgentStatus::Ready;
    current.ready_at = Some(NOW.into());
    current.conversation.conversation_id = Some("comparison-conversation".into());
    current.conversation.runtime_generation = Some(1);
    current.conversation.last_seq = 5;
    current.revision += 1;
    store.save_agent(1, current).await.unwrap();
    let mut request = submission("suggestion-original");
    request.command = serde_json::from_value(serde_json::json!({
        "type":"message", "input":{"content":[{"type":"text","text":"Add criteria.\nKeep limits."}]},
        "order":{"type":"after_turn","turn_sequence":2}, "metadata":{"cma.answer_sequence":"4"}
    })).unwrap();
    let owner = context("owner");
    let newer = service
        .admit(
            &owner,
            "selected-credential",
            CommandSubmissionSource::UserInterface,
            submission("other-tab"),
        )
        .await
        .unwrap();
    if reject_newer {
        store
            .reject_unclaimed_command(
                &newer.receipt.command_id,
                1,
                CommandFailureCode::AgentUnavailable,
                crate::now_rfc3339(),
            )
            .await
            .unwrap();
    }
    let result = service
        .admit(
            &owner,
            "selected-credential",
            CommandSubmissionSource::UserInterface,
            request,
        )
        .await;
    assert!(matches!(
        result,
        Err(CommandAdmissionError::Ledger(
            CommandLedgerError::NotAdmissible
        ))
    ));
}
