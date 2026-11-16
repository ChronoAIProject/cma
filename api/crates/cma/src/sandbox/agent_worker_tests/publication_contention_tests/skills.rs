use super::*;

#[tokio::test]
async fn skill_result_retries_only_publication_after_success_or_rejection() {
    for rejected in [false, true] {
        for change in [Change::None, Change::Title, Change::Log] {
            let (mut h, skills, claimed) = skill_fixture().await;
            if rejected {
                h.backend.fail_next(
                    WireOperation::InstallAgentSkill,
                    "request_rejected",
                    "skill refused",
                );
            }
            let store = inject(
                &mut h,
                skills.clone(),
                Boundary::SkillResult,
                Fault::Abort(change),
            );
            h.worker
                .execute_claimed_for_test(claimed.clone(), h.now)
                .await
                .unwrap();
            let saved = h.stored(&claimed.id).await;
            assert_eq!(attempts(&store), 2, "{change:?} rejected={rejected}");
            assert_eq!(saved.status, AgentStatus::Ready);
            assert_eq!(saved.work.step, AgentWorkStep::None);
            assert!(saved.work.lease.is_none());
            assert_eq!(
                saved.work.live_skills[0].status,
                if rejected {
                    AgentSkillInstallStatus::Failed
                } else {
                    AgentSkillInstallStatus::Installed
                }
            );
            assert_eq!(
                saved.work.live_skills[0].material.as_ref().unwrap().version,
                "7.0"
            );
            assert_eq!(saved.profile, claimed.profile);
            assert_eq!(saved.work.receipts, claimed.work.receipts);
            assert_eq!(
                skills.calls.load(Ordering::SeqCst),
                2,
                "one initial resolution and one live-skill resolution"
            );
            assert_eq!(calls(&h, WireOperation::InstallAgentSkill).await, 1);
            preserves_metadata(&h, &claimed.id, change).await;
            let logs = h.store.agent_log(&claimed.id, 0, 100).await.unwrap();
            assert_eq!(
                logs.iter()
                    .filter(|entry| matches!(
                        entry.kind,
                        AgentLogKind::SkillInstallationChanged {
                            status: AgentSkillInstallStatus::Installed
                                | AgentSkillInstallStatus::Failed,
                            ..
                        }
                    ))
                    .count(),
                1
            );
        }
    }
}

#[tokio::test]
async fn skill_result_refuses_changed_intent_claim_or_parent_after_native_effect() {
    for &change in INCOMPATIBLE {
        let (mut h, skills, claimed) = skill_fixture().await;
        let store = inject(&mut h, skills, Boundary::SkillResult, Fault::Abort(change));
        let result = h
            .worker
            .execute_claimed_for_test(claimed.clone(), h.now)
            .await;
        assert!(
            matches!(result, Err(SandboxStoreError::Conflict)),
            "{change:?}: {result:?}"
        );
        assert_eq!(attempts(&store), 1, "{change:?}");
        assert_eq!(
            calls(&h, WireOperation::InstallAgentSkill).await,
            1,
            "{change:?}"
        );
        let saved = h.stored(&claimed.id).await;
        assert_eq!(
            saved.work.live_skills[0].status,
            AgentSkillInstallStatus::Pending,
            "{change:?}"
        );
    }
}

#[tokio::test]
async fn skill_result_abort_budget_never_repeats_install() {
    let (mut h, skills, claimed) = skill_fixture().await;
    let store = inject(&mut h, skills, Boundary::SkillResult, Fault::AlwaysAbort);
    let result = h
        .worker
        .execute_claimed_for_test(claimed.clone(), h.now)
        .await;
    assert!(matches!(result, Err(SandboxStoreError::Conflict)));
    assert!((2..=8).contains(&attempts(&store)));
    assert_eq!(calls(&h, WireOperation::InstallAgentSkill).await, 1);
    assert_eq!(
        h.stored(&claimed.id).await.work.live_skills[0].status,
        AgentSkillInstallStatus::Pending
    );
}

#[tokio::test]
async fn skill_result_unknown_commit_is_not_replayed() {
    for fault in [Fault::Unavailable, Fault::CommittedUnavailable] {
        let (mut h, skills, claimed) = skill_fixture().await;
        let store = inject(&mut h, skills, Boundary::SkillResult, fault);
        let result = h
            .worker
            .execute_claimed_for_test(claimed.clone(), h.now)
            .await;
        assert!(
            matches!(result, Err(SandboxStoreError::Unavailable(_))),
            "{fault:?}: {result:?}"
        );
        assert_eq!(attempts(&store), 1);
        assert_eq!(calls(&h, WireOperation::InstallAgentSkill).await, 1);
        let saved = h.stored(&claimed.id).await;
        assert_eq!(
            saved.work.live_skills[0].status,
            if matches!(fault, Fault::CommittedUnavailable) {
                AgentSkillInstallStatus::Installed
            } else {
                AgentSkillInstallStatus::Pending
            }
        );
    }
}
