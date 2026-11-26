use super::*;
use layer2_core::sandbox::store::{SandboxTransaction, SandboxTransactionStore};

#[tokio::test]
async fn service_access_revives_with_new_authority_and_preserves_existing_conversation() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    let agent = prepare_agent(&harness, &active, now, 1).await;
    let pause_at = now + Duration::from_secs(1);
    harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            active.id.clone(),
            active.owner.clone(),
            rfc3339_at(pause_at),
            None,
        ))
        .await
        .unwrap();
    for _ in 0..4 {
        harness.run(&active.id, pause_at).await;
    }
    let paused = harness.stored(&active.id).await;
    assert_eq!(paused.status, SandboxStatus::Paused);
    let mut granted = paused.clone();
    let grant = granted
        .grant_service_access(
            &active.owner,
            &[ServiceId::new("api-github")],
            rfc3339_at(pause_at),
        )
        .unwrap()
        .unwrap();
    harness
        .store
        .commit(SandboxTransaction::SaveSandboxAndAppend {
            command_revival: None,
            released_runtime: None,
            expected_revision: paused.revision,
            sandbox: Box::new(granted.clone()),
            event_at: rfc3339_at(pause_at),
            event: SandboxEventKind::ServiceAccessGranted { grant },
        })
        .await
        .unwrap();
    let revive_at = pause_at + Duration::from_secs(1);
    harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
            active.id.clone(),
            active.owner.clone(),
            rfc3339_at(revive_at),
            SandboxReviveCause::ExplicitRequest,
        ))
        .await
        .unwrap();
    for _ in 0..12 {
        if harness.stored(&active.id).await.status == SandboxStatus::Active {
            break;
        }
        restarted_worker(&harness, revive_at)
            .await
            .run_one(&active.id, revive_at)
            .await
            .unwrap();
    }
    let revived = harness.stored(&active.id).await;
    assert_eq!(revived.status, SandboxStatus::Active);
    assert_eq!(revived.profile, active.profile);
    assert_eq!(revived.credential.reaches, active.credential.reaches);
    assert_eq!(
        revived.credential.key_generation,
        granted.credential.key_generation + 1
    );
    assert!(!revived.needs_service_access_credential());
    let current = revived.credential.current.as_ref().unwrap();
    assert!(current.generation > granted.credential.key_generation);
    assert_ne!(
        current.provider_key_id,
        paused.credential.current.as_ref().unwrap().provider_key_id
    );
    assert!(revived
        .credential
        .revoke_debt
        .iter()
        .any(|debt| debt.provider_key_id
            == paused.credential.current.as_ref().unwrap().provider_key_id));
    let restored = harness.store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(restored.status, AgentStatus::Ready);
    assert_eq!(restored.worktree_id, agent.worktree_id);
    assert_eq!(
        restored.conversation.conversation_id,
        agent.conversation.conversation_id
    );
    assert!(restored.conversation.first_message.is_none());
    let events = harness
        .store
        .sandbox_events(&active.id, 0, 200)
        .await
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.kind, SandboxEventKind::ServiceAccessGranted { .. }))
            .count(),
        1
    );
    assert_eq!(events.iter().filter(|event| matches!(event.kind, SandboxEventKind::CredentialChanged { key_generation } if key_generation == current.generation)).count(), 1);
}
