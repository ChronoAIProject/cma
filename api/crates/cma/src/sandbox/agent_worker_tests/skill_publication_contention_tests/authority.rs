use super::*;
use layer2_core::agent_ownership::{
    test_support::MemoryOwnershipStore, Member, OwnershipCoordinator, PROTOCOL_VERSION,
};

#[tokio::test(start_paused = true)]
async fn expired_skill_claim_is_not_published_or_reinstalled() {
    let (mut h, skills, claimed) = skill_fixture().await;
    let store = inject(&mut h, skills, Fault::Abort(Change::Expire));
    let result = h
        .worker
        .execute_claimed_for_test(claimed.clone(), h.now)
        .await;
    assert_eq!(result, Err(SandboxStoreError::Conflict));
    assert_eq!(attempts(&store), 1);
    assert_eq!(calls(&h, WireOperation::InstallAgentSkill).await, 1);
    let saved = h.stored(&claimed.id).await;
    assert_eq!(saved.work.lease, claimed.work.lease);
    assert_eq!(
        saved.work.live_skills[0].status,
        AgentSkillInstallStatus::Pending
    );
}

#[tokio::test]
async fn skill_completion_keeps_original_owner_epoch() {
    for change in [Change::None, Change::OwnerReplacement] {
        let (mut h, skills, claimed) = skill_fixture().await;
        let ownership = Arc::new(MemoryOwnershipStore::default());
        let coordinator = OwnershipCoordinator::register(
            ownership.clone(),
            Member {
                instance_id: "skill-publication-owner".into(),
                protocol: PROTOCOL_VERSION,
                relay_address: "http://owner.invalid".into(),
                expires_at_ms: 0,
                draining: false,
                owned_agents: 0,
                max_owned_agents: 8,
            },
        )
        .await
        .unwrap();
        let authority = coordinator.acquire(&claimed.id).await.unwrap().unwrap();
        let store = inject_with_ownership(
            &mut h,
            skills,
            Fault::Abort(change),
            Some((coordinator, ownership)),
        );
        let result = authority
            .run(h.worker.execute_claimed_for_test(claimed.clone(), h.now))
            .await;
        let replaced = matches!(change, Change::OwnerReplacement);
        if replaced {
            assert!(
                result.is_err() || result == Ok(Err(SandboxStoreError::Conflict)),
                "{result:?}"
            );
        } else {
            assert_eq!(result, Ok(Ok(())));
        }
        assert_eq!(attempts(&store), if replaced { 1 } else { 2 });
        assert_eq!(calls(&h, WireOperation::InstallAgentSkill).await, 1);
        assert_eq!(
            h.stored(&claimed.id).await.work.live_skills[0].status,
            if replaced {
                AgentSkillInstallStatus::Pending
            } else {
                AgentSkillInstallStatus::Installed
            }
        );
    }
}

#[tokio::test]
async fn skill_result_uses_parents_observed_before_install() {
    for change in [
        Change::ParentRuntime,
        Change::ParentPause,
        Change::Workspace,
        Change::Worktree,
    ] {
        let (mut h, skills, claimed) = skill_fixture().await;
        // Change the durable parent after the original runtime was adopted and
        // material was pinned, before the native response and first publication.
        let store = inject(&mut h, skills, Fault::AfterPin(change));
        let result = h
            .worker
            .execute_claimed_for_test(claimed.clone(), h.now)
            .await;
        assert_eq!(
            calls(&h, WireOperation::InstallAgentSkill).await,
            1,
            "{change:?}"
        );
        assert_eq!(result, Err(SandboxStoreError::Conflict), "{change:?}");
        assert_eq!(attempts(&store), 0, "{change:?}");
        assert_eq!(
            h.stored(&claimed.id).await.work.live_skills[0].status,
            AgentSkillInstallStatus::Pending
        );
    }
}
