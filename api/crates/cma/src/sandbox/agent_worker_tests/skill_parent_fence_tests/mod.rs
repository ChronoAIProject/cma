//! Real publication transactions with a synthetic skill runtime.
use super::super::*;
use crate::store::agent_ownership::MongoAgentOwnership;
use crate::store::MongoSandboxStore;
use layer2_core::agent_ownership::{
    Member, OwnershipCoordinator, OwnershipStore, PROTOCOL_VERSION,
};
use layer2_core::sandbox::store::*;
use layer2_core::sandbox::{AgentLogEntry, AgentSkillInstallStatus, Worktree};
use layer2_core::{Authorization, Caller, Capability};
use mongodb::bson::doc;
use mongodb::Client;

mod fixture;
mod store;
use fixture::ProbeFixture;
use store::ProbeStore;

#[derive(Clone, Copy, Debug)]
enum Case {
    KnownAbort,
    UnknownCommitted,
    FailedReadback,
    ParentPause,
    ParentRuntime,
    Stop,
    Delete,
    Owner,
    Valid,
    UnprovedBoundary,
    Refusal,
    Metadata,
    SnapshotSandbox,
    SnapshotWorkspace,
    SnapshotWorktree,
    WorkspaceRemoving,
    WorktreeRemoving,
    MissingSandbox,
    MissingWorkspace,
    MissingWorktree,
    AbortThenPause,
    RollbackLateReceipt,
}

async fn run(case: Case, segmented: bool) {
    let ProbeFixture {
        mut h,
        skills,
        claimed,
        admin,
        app,
        database,
        observation,
    } = fixture::create(case, segmented).await;
    let ownership = if matches!(case, Case::Owner) {
        let store = Arc::new(MongoAgentOwnership::open(&admin, &database).await.unwrap());
        let manager = OwnershipCoordinator::register(
            store.clone(),
            Member {
                instance_id: "skill-fence-owner".into(),
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
        let authority = manager.acquire(&claimed.id).await.unwrap().unwrap();
        h.store = Arc::new(
            (*h.store)
                .clone()
                .enable_agent_ownership("skill-fence-owner".into())
                .await
                .unwrap(),
        );
        Some((store, manager, authority))
    } else {
        None
    };
    let store = Arc::new(ProbeStore {
        inner: h.store.clone(),
        cleanup_worker: h.worker.clone(),
        admin: admin.clone(),
        app,
        case,
        attempts: AtomicUsize::new(0),
        observation: observation.clone(),
        ownership: ownership.as_ref().map(|(s, _, _)| s.clone()),
        proposed: Mutex::new(None),
        before: Mutex::new(None),
    });
    let mut config = AgentWorkerConfig::new("agent-worker", DEPLOYMENT_ID);
    config.lease_duration = Duration::from_secs(30);
    config.open_timeout = Duration::from_secs(2);
    config.ownership = ownership.as_ref().map(|(_, m, _)| m.clone());
    h.worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: store.clone(),
            adapter: h.adapter.clone(),
            skills: skills.clone(),
            registry: h.registry.clone(),
            updates: h.updates.clone(),
            operations: AgentOperationService::new(h.store.clone(), h.registry.clone()),
        },
        config,
    );
    let result = if let Some((_, _, authority)) = &ownership {
        match authority
            .run(h.worker.execute_claimed_for_test(claimed.clone(), h.now))
            .await
        {
            Ok(result) => result,
            Err(layer2_core::agent_ownership::OwnershipError::Stale) => {
                eprintln!("SKILL_FENCE_OWNER_ENVELOPE correctly refused stale execution authority");
                Err(SandboxStoreError::Conflict)
            }
            Err(error) => panic!("unexpected authority error: {error:?}"),
        }
    } else {
        h.worker
            .execute_claimed_for_test(claimed.clone(), h.now)
            .await
    };
    fixture::clear(&admin).await;
    // The injected commit error can precede the applied commit's visibility.
    crate::store::test_visibility::majority_barrier(&admin, &database).await;
    let saved = h.stored(&claimed.id).await;
    let logs = h.store.agent_log(&claimed.id, 0, 100).await.unwrap();
    let requests = h.backend.requests().await;
    let installs = requests
        .iter()
        .filter(|(_, r)| r.operation() == WireOperation::InstallAgentSkill)
        .count();
    let terminals = logs
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                AgentLogKind::SkillInstallationChanged {
                    status: AgentSkillInstallStatus::Installed | AgentSkillInstallStatus::Failed,
                    ..
                }
            )
        })
        .count();
    let attempts = store.attempts.load(Ordering::SeqCst);
    let proposed = store.proposed.lock().unwrap().clone();
    let before = store.before.lock().unwrap().clone();
    let observation = observation.lock().unwrap().clone();
    eprintln!("SKILL_PARENT_PUBLICATION case={case:?} segmented={segmented} succeeded={} proposals={attempts} installs={installs} terminal_logs={terminals} status={:?} commits={} update_failures={} readback_injected={}",
        result.is_ok(), saved.work.live_skills[0].status, observation.commits, observation.update_failures, observation.readback_injected);
    // Cleanup precedes assertions so a reproduced product failure leaves no database.
    admin.database(&database).drop().await.unwrap();
    assert_eq!(installs, 1, "{case:?}: native installation count");
    let installed = requests
        .iter()
        .find_map(|(_, request)| match request {
            layer3_adapter::wire::RequestFrame::InstallAgentSkill(request) => Some(request),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        installed.authority_fence,
        claimed.work.lease.as_ref().unwrap().fence
    );
    assert_eq!(
        installed.request,
        layer3_adapter::AgentSkillInstallRequest::new(
            layer3_adapter::AgentSlot::new(claimed.id.as_str()),
            super::runtime_skill("7.0", 'a', "archive"),
        ),
        "the original operation ID, digest and material must reach the runtime"
    );
    assert_eq!(saved.profile, claimed.profile);
    if matches!(case, Case::Delete) {
        assert!(saved.work.receipts.starts_with(&claimed.work.receipts));
        assert!(saved
            .work
            .receipts
            .iter()
            .any(|receipt| receipt.step == AgentWorkStep::CloseProcess));
    } else {
        assert_eq!(saved.work.receipts, claimed.work.receipts);
    }
    assert_eq!(
        saved.conversation.conversation_id,
        claimed.conversation.conversation_id
    );
    assert_eq!(
        saved.conversation.runtime_generation,
        claimed.conversation.runtime_generation
    );
    assert_eq!(skills.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        saved.work.live_skills[0].material.as_ref().unwrap().version,
        "7.0"
    );
    match case {
        Case::Valid
        | Case::UnprovedBoundary
        | Case::Refusal
        | Case::Metadata
        | Case::RollbackLateReceipt => {
            assert_eq!(result, Ok(()));
            assert_eq!(attempts, 1);
            assert_eq!(terminals, 1);
            assert_eq!(saved, proposed.unwrap().agent);
            assert_eq!(
                saved.work.live_skills[0].status,
                if matches!(case, Case::Refusal) {
                    AgentSkillInstallStatus::Failed
                } else {
                    AgentSkillInstallStatus::Installed
                }
            );
        }
        Case::MissingSandbox | Case::MissingWorkspace | Case::MissingWorktree => {
            assert_eq!(result, Err(SandboxStoreError::NotFound));
            assert_eq!(attempts, 1);
            assert_eq!(terminals, 0);
            assert_eq!(saved, before.unwrap());
        }
        Case::KnownAbort => {
            assert_eq!(result, Ok(()));
            assert_eq!(attempts, 2);
            assert_eq!(observation.update_failures, 5);
            assert_eq!(terminals, 1);
            assert_eq!(
                saved.work.live_skills[0].status,
                AgentSkillInstallStatus::Installed
            );
        }
        Case::UnknownCommitted | Case::FailedReadback => {
            assert_eq!(
                attempts, 1,
                "ambiguous commit must not create a new proposal"
            );
            assert_eq!(observation.commits, 5);
            if matches!(case, Case::FailedReadback) {
                assert!(observation.readback_injected);
                assert!(
                    matches!(result, Err(SandboxStoreError::Unavailable(_))),
                    "{result:?}"
                );
                // An unavailable readback does not establish whether commit happened.
                // Require one complete exact state, never a partial or fresh proposal.
                if saved.work.live_skills[0].status == AgentSkillInstallStatus::Pending {
                    assert_eq!(
                        saved,
                        before.unwrap(),
                        "exact pre-publication state retained"
                    );
                    assert_eq!(terminals, 0);
                } else {
                    assert_eq!(
                        saved,
                        proposed.unwrap().agent,
                        "exact original committed receipt"
                    );
                    assert_eq!(terminals, 1);
                }
            } else {
                assert_eq!(result, Ok(()));
                assert_eq!(
                    saved,
                    proposed.unwrap().agent,
                    "exact original proposal receipt"
                );
                assert_eq!(terminals, 1);
                assert_eq!(
                    saved.work.live_skills[0].status,
                    AgentSkillInstallStatus::Installed
                );
            }
        }
        Case::Stop | Case::Delete => {
            assert_eq!(result, Err(SandboxStoreError::Conflict));
            assert_eq!(attempts, 1);
            assert_eq!(
                saved.work.live_skills[0].status,
                AgentSkillInstallStatus::Failed
            );
            assert_eq!(
                saved.work.live_skills[0].error.as_deref(),
                Some("agent_stopped_before_skill_confirmation")
            );
            assert!(!logs.iter().any(|e| matches!(
                e.kind,
                AgentLogKind::SkillInstallationChanged {
                    status: AgentSkillInstallStatus::Installed,
                    ..
                }
            )));
        }
        Case::Owner
        | Case::ParentPause
        | Case::ParentRuntime
        | Case::SnapshotSandbox
        | Case::SnapshotWorkspace
        | Case::SnapshotWorktree
        | Case::WorkspaceRemoving
        | Case::WorktreeRemoving
        | Case::AbortThenPause => {
            assert_eq!(
                result,
                Err(SandboxStoreError::Conflict),
                "{case:?}: changed authority must fence the completed native result"
            );
            assert_eq!(attempts, 1);
            assert_eq!(terminals, 0);
            assert_eq!(
                saved.work.live_skills[0].status,
                AgentSkillInstallStatus::Pending
            );
        }
    }
}

macro_rules! contract {
    ($name:ident, $case:ident) => {
        #[tokio::test]
        #[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
        async fn $name() {
            for segmented in [false, true] {
                run(Case::$case, segmented).await;
            }
        }
    };
}
contract!(known_abort_one_native_install_one_terminal_log, KnownAbort);
contract!(
    unknown_commit_uses_original_receipt_without_new_proposal,
    UnknownCommitted
);
contract!(
    failed_readback_remains_unavailable_without_new_proposal,
    FailedReadback
);
contract!(
    parent_pause_after_last_read_must_fence_skill_publication,
    ParentPause
);
contract!(
    parent_runtime_after_last_read_must_fence_skill_publication,
    ParentRuntime
);
contract!(stop_after_last_read_is_not_overwritten, Stop);
contract!(delete_after_last_read_is_not_overwritten, Delete);
contract!(owner_replacement_after_last_read_is_not_overwritten, Owner);

contract!(valid_result_publishes_original_material_once, Valid);
contract!(terminal_refusal_publishes_original_material_once, Refusal);
contract!(
    compatible_parent_metadata_does_not_invalidate_result,
    Metadata
);
contract!(
    parent_pause_after_snapshot_requires_a_shared_write,
    SnapshotSandbox
);
contract!(
    workspace_removal_after_snapshot_requires_a_shared_write,
    SnapshotWorkspace
);
contract!(
    worktree_removal_after_snapshot_requires_a_shared_write,
    SnapshotWorktree
);
contract!(
    workspace_removal_after_last_read_refuses_result,
    WorkspaceRemoving
);
contract!(
    worktree_removal_after_last_read_refuses_result,
    WorktreeRemoving
);
contract!(
    missing_sandbox_refuses_result_without_partial_publication,
    MissingSandbox
);
contract!(
    missing_workspace_refuses_result_without_partial_publication,
    MissingWorkspace
);
contract!(
    missing_worktree_refuses_result_without_partial_publication,
    MissingWorktree
);
contract!(
    known_abort_revalidates_changed_parent_without_reinstalling,
    AbortThenPause
);
contract!(
    rollback_is_atomic_and_late_receipt_survives_parent_pause,
    RollbackLateReceipt
);

contract!(
    generic_work_publication_cannot_bypass_parent_proof,
    UnprovedBoundary
);
