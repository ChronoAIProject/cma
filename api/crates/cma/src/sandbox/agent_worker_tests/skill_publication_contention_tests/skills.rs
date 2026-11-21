use super::*;

#[tokio::test]
async fn skill_result_retries_only_publication_after_success() {
    completed_result(false).await;
}

#[tokio::test]
async fn skill_result_retries_only_publication_after_rejection() {
    completed_result(true).await;
}

async fn completed_result(rejected: bool) {
    for change in [
        Change::None,
        Change::Title,
        Change::PreferenceTitle,
        Change::Log,
        Change::WorktreeRevision,
    ] {
        let (mut h, skills, claimed) = skill_fixture().await;
        if rejected {
            h.backend.fail_next(
                WireOperation::InstallAgentSkill,
                "request_rejected",
                "skill refused",
            );
        }
        let store = inject(&mut h, skills.clone(), Fault::Abort(change));
        let result = h
            .worker
            .execute_claimed_for_test(claimed.clone(), h.now)
            .await;
        assert_eq!(calls(&h, WireOperation::InstallAgentSkill).await, 1);
        result.unwrap();
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
        assert_eq!(
            saved.work.live_skills[0].error.as_deref(),
            if rejected {
                Some("runtime_skill_install_rejected")
            } else {
                None
            }
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

#[tokio::test]
async fn skill_result_refuses_changed_intent_claim_or_parent_after_native_effect() {
    for &change in INCOMPATIBLE {
        let (mut h, skills, claimed) = skill_fixture().await;
        let store = inject(&mut h, skills, Fault::Abort(change));
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
        let installation = &saved.work.live_skills[0];
        if matches!(change, Change::Stop | Change::Delete) {
            // Stop owns cancellation even when the native install succeeded.
            // A late publication must preserve its durable refusal reason.
            assert_eq!(installation.status, AgentSkillInstallStatus::Failed);
            assert_eq!(
                installation.error.as_deref(),
                Some("agent_stopped_before_skill_confirmation")
            );
            if matches!(change, Change::Stop) {
                assert_eq!(saved.requested_stop_reason, Some(AgentStopReason::Owner));
            } else {
                assert_eq!(saved.stop_reason, Some(AgentStopReason::Owner));
                assert_eq!(saved.work.step, AgentWorkStep::EnsureProcessClosed);
            }
            assert_ne!(saved.work.step, AgentWorkStep::InstallSkill);
        } else {
            assert_eq!(
                installation.status,
                AgentSkillInstallStatus::Pending,
                "{change:?}"
            );
            assert!(installation.error.is_none(), "{change:?}");
        }
        let logs = h.store.agent_log(&claimed.id, 0, 100).await.unwrap();
        assert!(
            !logs.iter().any(|entry| matches!(
                entry.kind,
                AgentLogKind::SkillInstallationChanged {
                    status: AgentSkillInstallStatus::Installed,
                    ..
                }
            )),
            "{change:?}: refused publication must not confirm installation"
        );
    }
}

#[tokio::test]
async fn skill_result_abort_budget_never_repeats_install() {
    let (mut h, skills, claimed) = skill_fixture().await;
    let store = inject(&mut h, skills, Fault::AlwaysAbort);
    let result = h
        .worker
        .execute_claimed_for_test(claimed.clone(), h.now)
        .await;
    assert!(matches!(result, Err(SandboxStoreError::Conflict)));
    assert_eq!(calls(&h, WireOperation::InstallAgentSkill).await, 1);
    assert_eq!(attempts(&store), 8);
    let saved = h.stored(&claimed.id).await;
    assert_eq!(saved.work.lease, claimed.work.lease);
    assert_eq!(saved.work.step, AgentWorkStep::InstallSkill);
    assert_eq!(
        saved.work.live_skills[0].status,
        AgentSkillInstallStatus::Pending
    );
}

#[tokio::test]
async fn skill_result_unknown_commit_is_not_replayed() {
    for fault in [
        Fault::Unavailable,
        Fault::CommittedUnavailable,
        Fault::ReadbackUnavailable,
    ] {
        let (mut h, skills, claimed) = skill_fixture().await;
        let store = inject(&mut h, skills, fault);
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

#[tokio::test]
async fn skill_result_store_receipt_recovery_does_not_repeat_publication_or_install() {
    let (mut h, skills, claimed) = skill_fixture().await;
    let store = inject(&mut h, skills, Fault::Reconciled);
    h.worker
        .execute_claimed_for_test(claimed.clone(), h.now)
        .await
        .unwrap();
    assert_eq!(attempts(&store), 1);
    assert_eq!(calls(&h, WireOperation::InstallAgentSkill).await, 1);
    assert_eq!(
        h.stored(&claimed.id).await.work.live_skills[0].status,
        AgentSkillInstallStatus::Installed
    );
    assert!(!h
        .worker
        .run_one(&claimed.id, h.now + Duration::from_secs(31))
        .await
        .unwrap());
    assert_eq!(calls(&h, WireOperation::InstallAgentSkill).await, 1);
}
