use super::*;

#[tokio::test]
async fn partition_reset_eligibility_distinguishes_absence_lifecycle_and_cleanup() {
    let f = fixture();
    let binding = f.ready().await;
    let mapping = resource_access::ready_conversation(&f, &binding).await;
    reset::delete(&f, &mapping).await;
    let agent = f.store.get_agent(&mapping.agent_id).await.unwrap().unwrap();
    let sandbox = f
        .store
        .get_sandbox(&binding.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let workspace = f
        .store
        .get_workspace(&binding.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let check = |binding: &ProviderBinding, agent: Option<&Agent>| {
        reset_eligibility(binding, &mapping, agent, Some(&sandbox), Some(&workspace))
    };
    assert!(check(&binding, Some(&agent)).is_ok());
    assert!(matches!(
        check(&binding, None),
        Err(BindingError::ResetRefused("partition_reset_agent_missing"))
    ));
    for status in [
        AgentStatus::Ready,
        AgentStatus::Working,
        AgentStatus::Stopped,
        AgentStatus::Failed,
        AgentStatus::Paused,
    ] {
        let mut value = agent.clone();
        value.status = status;
        assert!(matches!(
            check(&binding, Some(&value)),
            Err(BindingError::ResetRefused(
                "partition_reset_agent_not_deleted"
            ))
        ));
    }
    let mut value = agent.clone();
    value.work.step = crate::sandbox::AgentWorkStep::EnsureProcessClosed;
    assert!(matches!(
        check(&binding, Some(&value)),
        Err(BindingError::ResetRefused(
            "partition_reset_cleanup_pending"
        ))
    ));
    let mut value = binding.clone();
    value.profile.session_affinity = SessionAffinity::SourceUser;
    assert!(matches!(
        check(&value, Some(&agent)),
        Err(BindingError::ResetRefused(
            "partition_reset_affinity_unsupported"
        ))
    ));
    let mut value = binding.clone();
    value.existing_agent = Some(ExistingBindingAgent {
        agent_id: agent.id.clone(),
        worktree_id: agent.worktree_id.clone(),
    });
    assert!(matches!(
        check(&value, Some(&agent)),
        Err(BindingError::ResetRefused("partition_reset_existing_agent"))
    ));
    let mut value = binding.clone();
    value.state = BindingState::Retired;
    assert!(matches!(
        check(&value, Some(&agent)),
        Err(BindingError::ResetRefused(
            "partition_reset_binding_not_ready"
        ))
    ));
    assert!(matches!(
        reset_eligibility(&binding, &mapping, Some(&agent), None, Some(&workspace)),
        Err(BindingError::ResetRefused(
            "partition_reset_parents_unavailable"
        ))
    ));
}
