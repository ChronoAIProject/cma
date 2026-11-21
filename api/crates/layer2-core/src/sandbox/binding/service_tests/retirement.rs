use super::*;

#[tokio::test]
async fn absent_conversation_delete_commits_a_retiring_tombstone_while_admission_is_stopped() {
    use crate::admission::AdmissionControlStore;
    let f = fixture();
    let binding = f.ready().await;
    f.store
        .change(
            0,
            &SubjectId::new("operator"),
            "contain incident",
            true,
            100,
        )
        .await
        .unwrap();
    let id = "conversation_absent";
    f.service
        .retire_conversation(&f.caller, &binding.binding_id, id)
        .await
        .unwrap();
    let tombstone = f
        .store
        .binding_conversation(&binding.binding_id, binding.generation, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(tombstone.state, BindingConversationState::Retiring);
    assert_eq!(tombstone.revision, 1);
    assert!(f
        .store
        .get_agent(&tombstone.agent_id)
        .await
        .unwrap()
        .is_none());
    f.service
        .retire_conversation(&f.caller, &binding.binding_id, id)
        .await
        .unwrap();
    assert_eq!(
        f.store
            .binding_conversation(&binding.binding_id, binding.generation, id)
            .await
            .unwrap(),
        Some(tombstone.clone())
    );
    assert_eq!(
        f.store
            .pending_binding_conversations(None, 100)
            .await
            .unwrap(),
        [tombstone]
    );
    f.store
        .change(1, &SubjectId::new("operator"), "resolved", false, 101)
        .await
        .unwrap();
    assert!(matches!(
        f.service
            .ensure_conversation(&f.caller, &binding.binding_id, id)
            .await,
        Err(BindingError::Conflict)
    ));
}

#[tokio::test]
async fn binding_retirement_fences_paused_parent_before_waiting_for_conversations() {
    let f = fixture();
    let binding = f.ready().await;
    let (conversation, _) = f
        .service
        .ensure_conversation(&f.caller, "binding-one", "conversation_one")
        .await
        .unwrap();
    f.service
        .reconcile_conversation(&binding, &conversation)
        .await
        .unwrap();
    let mut sandbox = f
        .store
        .get_sandbox(&binding.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let revision = sandbox.revision;
    sandbox.status = SandboxStatus::Paused;
    sandbox.lifecycle.checkpoints.latest = Some(
        crate::sandbox::lifecycle_test_support::checkpoint(1, &crate::now_rfc3339()),
    );
    sandbox.revision += 1;
    f.store.save_sandbox(revision, sandbox).await.unwrap();
    f.service.retire(&f.caller, "binding-one").await.unwrap();
    let retiring = f.binding().await;
    f.service.reconcile_binding(&retiring).await.unwrap();
    let parent = f
        .store
        .get_sandbox(&binding.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parent.status, SandboxStatus::Retiring);
    assert_eq!(parent.work.step, SandboxWorkStep::StopAgents);
    assert!(parent.revive_eligibility().is_err());
    assert_eq!(
        f.binding().await.work,
        BindingWork::RetireCurrentConversations
    );
    assert_eq!(
        f.store
            .binding_conversation("binding-one", 1, "conversation_one")
            .await
            .unwrap()
            .unwrap()
            .state,
        BindingConversationState::Retiring
    );
    assert!(f
        .custody
        .keys
        .lock()
        .unwrap()
        .contains_key(&binding.sandbox_id));
}

#[tokio::test]
async fn binding_retirement_rejects_stale_revision_and_foreign_credential_ownership() {
    for wrong_source in [false, true] {
        let f = fixture();
        let binding = f.ready().await;
        f.service.retire(&f.caller, "binding-one").await.unwrap();
        let retiring = f.binding().await;
        let sandbox = f
            .store
            .get_sandbox(&binding.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        if wrong_source {
            let mut foreign = retiring.clone();
            foreign.tool_endpoint = Some("https://another.example/tools".into());
            assert!(f
                .service
                .resources
                .sandboxes
                .retire_for_binding(&foreign, &binding.sandbox_id)
                .await
                .is_err());
        } else {
            let mut newer = retiring.clone();
            newer.advance_revision(crate::now_rfc3339()).unwrap();
            f.store
                .save_binding(Some(retiring.revision), newer)
                .await
                .unwrap();
            assert!(f.service.reconcile_binding(&retiring).await.is_err());
        }
        assert_eq!(
            f.store
                .get_sandbox(&binding.sandbox_id)
                .await
                .unwrap()
                .unwrap(),
            sandbox
        );
    }
}
