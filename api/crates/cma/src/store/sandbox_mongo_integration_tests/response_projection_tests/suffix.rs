use super::*;

#[tokio::test]
#[ignore = "requires the disposable Mongo replica-set runner"]
async fn verified_suffix_reuses_only_the_original_prefix_and_preserves_post_terminal_usage() {
    let mut f = Fixture::new(1_000, 990, None, Some("retained"), 64).await;
    let original = f
        .store
        .agent_response_log_window(&f.agent, Some(990))
        .await
        .unwrap()
        .unwrap();
    let usage = AgentLogEntry {
        agent_id: f.agent.id.clone(),
        seq: 1001,
        appended_at: NOW.into(),
        kind: AgentLogKind::Usage {
            input_tokens: 7,
            output_tokens: 11,
            context_used: 18,
            context_window: None,
        },
    };
    f.store
        .agent_log
        .insert_one(log_document(&usage).unwrap())
        .await
        .unwrap();
    f.agent.conversation.last_seq = 1001;
    f.agent.revision += 1;
    f.store
        .agents
        .replace_one(
            doc! {"_id":f.agent.id.as_str()},
            crate::store::agents::agent_document(&f.agent).unwrap(),
        )
        .await
        .unwrap();
    f.store = f
        .store
        .prepare_bounded_admission(std::slice::from_ref(&f.agent.id))
        .await
        .unwrap();
    f.reset();
    let extended = f
        .store
        .agent_response_log_suffix(&f.agent, &original)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        &extended.entries[..original.entries.len()],
        &original.entries
    );
    assert_eq!(extended.entries.last(), Some(&usage));
    assert_eq!(extended.through, 1001);
    assert_eq!(
        extended.entries,
        f.store
            .agent_response_log_window(&f.agent, Some(990))
            .await
            .unwrap()
            .unwrap()
            .entries
    );
    f.assert_read_only();
    // A different original boundary cannot borrow the saved prefix, even when
    // all coordinates still match the current head.
    let mut forged = original.clone();
    forged.entries.last_mut().unwrap().appended_at = "2026-01-01T00:00:00Z".into();
    assert!(matches!(
        f.store.agent_response_log_suffix(&f.agent, &forged).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires the disposable Mongo replica-set runner"]
async fn suffix_budget_or_missing_proof_requests_complete_verification_without_partial_success() {
    let mut f = Fixture::new(1_000, 990, None, Some("retained"), 64).await;
    let previous = f
        .store
        .agent_response_log_window(&f.agent, Some(990))
        .await
        .unwrap()
        .unwrap();
    f.agent.conversation.last_seq += 1;
    f.agent.revision += 1;
    f.store
        .agents
        .replace_one(
            doc! {"_id":f.agent.id.as_str()},
            crate::store::agents::agent_document(&f.agent).unwrap(),
        )
        .await
        .unwrap();
    // Missing source at an asserted head is corruption, never a shorter window.
    assert!(f
        .store
        .agent_response_log_suffix(&f.agent, &previous)
        .await
        .unwrap()
        .is_none());
    assert!(f
        .store
        .agent_log_prefix(&f.agent.id, f.agent.conversation.last_seq)
        .await
        .is_err());
    f.cleanup().await;
}
