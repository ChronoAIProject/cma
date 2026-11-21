use super::*;
use layer3_adapter::wire::{AgentResourcesPlan, AgentResourcesReceipt};

fn batch(fence: u64, plan: &AgentResourcesPlan) -> RequestFrame {
    RequestFrame::PrepareAgentResources(FencedRequest::new(fence, plan.clone()))
}

async fn plan(supervisor: &Supervisor) -> AgentResourcesPlan {
    let workspace = prepare_workspace(supervisor).await;
    AgentResourcesPlan {
        worktree: WorktreeRequest::new(
            "worktree:project:agent-1:add:v1",
            "project",
            "agent-1",
            workspace.resolved_commit,
        ),
        home: home_plan("agent-1", b"prepared profile"),
    }
}

fn receipt(response: ResponseFrame, fence: u64) -> AgentResourcesReceipt {
    match response {
        ResponseFrame::PrepareAgentResources(receipt) => {
            assert_eq!(receipt.authority_fence, fence);
            receipt.receipt
        }
        other => panic!("resource preparation failed: {other:?}"),
    }
}

#[tokio::test]
async fn agent_resources_replay_preserves_both_journals_and_dirty_files_across_restart() {
    let config = RuntimeConfig::for_test(root("resource-replay"));
    let supervisor = provision(&config).await;
    let plan = plan(&supervisor).await;
    let original_worktree = supervisor
        .dispatch(add_worktree_request(plan.worktree.clone()))
        .await;
    let original_home = supervisor
        .dispatch(materialize_home_request(plan.home.clone()))
        .await;
    let first = receipt(supervisor.dispatch(batch(2, &plan)).await, 2);
    assert_eq!(
        original_worktree,
        ResponseFrame::AddWorktree(FencedReceipt::new(1, first.worktree.clone()))
    );
    assert_eq!(
        original_home,
        ResponseFrame::MaterializeAgentHome(FencedReceipt::new(1, first.home.clone()))
    );
    let dirty = std::path::Path::new(&first.worktree.path).join("owner-work.txt");
    std::fs::write(&dirty, b"retained work").unwrap();
    let profile = config.agent_home("agent-1").join("config.toml");
    std::fs::write(&profile, b"owner-edited profile").unwrap();
    drop(supervisor);
    let supervisor = recreate(&config).await;
    assert_eq!(
        receipt(supervisor.dispatch(batch(3, &plan)).await, 3),
        first
    );
    assert_eq!(std::fs::read(dirty).unwrap(), b"retained work");
    assert_eq!(std::fs::read(profile).unwrap(), b"owner-edited profile");
    for operation in [
        Operation::add_worktree(&plan.worktree),
        Operation::create_agent_home(&plan.home),
    ] {
        assert!(mutation_journal::admitted(&config, &operation).unwrap());
    }
    cleanup(&config);
}

#[tokio::test]
async fn agent_resources_second_effect_failure_keeps_first_receipt_and_recovers_exactly() {
    let config = RuntimeConfig::for_test(root("resource-partial"));
    let supervisor = provision(&config).await;
    let plan = plan(&supervisor).await;
    let blocked = config.agent_home("agent-1");
    std::fs::create_dir_all(blocked.parent().unwrap()).unwrap();
    std::fs::write(&blocked, b"unrelated file").unwrap();
    let failed = failure(supervisor.dispatch(batch(2, &plan)).await);
    assert_eq!(
        failed.operation,
        layer3_adapter::wire::WireOperation::PrepareAgentResources
    );
    let Begin::Completed(first) = mutation_journal::begin::<WorktreeReceipt>(
        &config,
        &Operation::add_worktree(&plan.worktree),
    )
    .unwrap() else {
        panic!("first operation must retain its own completed receipt");
    };
    assert_eq!(std::fs::read(&blocked).unwrap(), b"unrelated file");
    let dirty = std::path::Path::new(&first.path).join("keep.txt");
    std::fs::write(&dirty, b"partial-effect work").unwrap();
    std::fs::remove_file(blocked).unwrap();
    drop(supervisor);
    let supervisor = recreate(&config).await;
    let completed = receipt(supervisor.dispatch(batch(3, &plan)).await, 3);
    assert_eq!(completed.worktree, first);
    assert_eq!(std::fs::read(dirty).unwrap(), b"partial-effect work");
    cleanup(&config);
}

#[tokio::test]
async fn agent_resources_refuses_stale_or_retired_either_resource_before_effects() {
    for home in [false, true] {
        for retired in [false, true] {
            let config = RuntimeConfig::for_test(root("resource-fences"));
            let supervisor = provision(&config).await;
            let plan = plan(&supervisor).await;
            let operation = match (home, retired) {
                (false, false) => Operation::add_worktree(&plan.worktree),
                (true, false) => Operation::create_agent_home(&plan.home),
                (false, true) => Operation::remove_worktree(&WorktreeRef::new(
                    "worktree:project:agent-1:remove:v1",
                    "project",
                    "agent-1",
                )),
                (true, true) => Operation::remove_agent_home(&AgentHomeRemoveRequest::new(
                    "agent:agent-1:home:remove:v1",
                    AgentSlot::new("agent-1"),
                )),
            };
            crate::mutation_authority::admit(&config, 9, &operation).unwrap();
            assert!(matches!(
                supervisor.dispatch(batch(2, &plan)).await,
                ResponseFrame::Failure(_)
            ));
            assert!(!config.home.join("project/.worktrees/agent-1").exists());
            assert!(!config.agent_home("agent-1").exists());
            cleanup(&config);
        }
    }
}

#[tokio::test]
async fn agent_resources_requires_credentials_and_refuses_restore_or_checkpoint_fences() {
    let config = RuntimeConfig::for_test(root("resource-guards"));
    let supervisor = provision(&config).await;
    let plan = plan(&supervisor).await;
    for phase in [
        Phase::Provisioned,
        Phase::Installed,
        Phase::RestorePending,
        Phase::RestoreCredentialed,
    ] {
        *supervisor.phase.write().await = phase;
        let error = failure(supervisor.dispatch(batch(2, &plan)).await);
        assert!(matches!(
            error.code.as_str(),
            "credential_not_installed" | "restore_not_completed"
        ));
        assert!(!config.home.join("project/.worktrees/agent-1").exists());
    }
    *supervisor.phase.write().await = Phase::Credentialed;
    let hold = supervisor
        .dispatch(RequestFrame::CheckpointForTeardown(
            layer3_adapter::CheckpointHoldRequest {
                checkpoint: layer3_adapter::CheckpointRequest {
                    epoch: 1,
                    exclusions: vec![],
                },
                writer_resume_token: "a".repeat(64),
            },
        ))
        .await;
    assert!(
        matches!(hold, ResponseFrame::CheckpointForTeardown(_)),
        "{hold:?}"
    );
    let error = failure(supervisor.dispatch(batch(2, &plan)).await);
    assert_eq!(error.code, "checkpoint_writers_held");
    assert!(!config.home.join("project/.worktrees/agent-1").exists());
    cleanup(&config);
}

#[tokio::test]
async fn capacity_checks_both_preparation_intents_before_either_effect() {
    for (count, accepted) in [(1020, false), (1019, true)] {
        let config = RuntimeConfig::for_test(root("resource-capacity"));
        let supervisor = provision(&config).await;
        let plan = plan(&supervisor).await;
        crate::mutation_journal::capacity_tests::fill(&config, count);
        let response = supervisor.dispatch(batch(2, &plan)).await;
        if accepted {
            let result = receipt(response, 2);
            assert!(std::path::Path::new(&result.worktree.path).is_dir());
            assert!(config.agent_home("agent-1").is_dir());
        } else {
            let error = failure(response);
            assert_eq!(error.code, "mutation_ledger_capacity");
            assert_eq!(error.detail, crate::mutation_capacity::DETAIL);
            assert!(!error.retryable);
            assert!(!crate::worktree::expected_path(&config.home, "project", "agent-1").exists());
            assert!(!config.agent_home("agent-1").exists());
            assert!(crate::mutation_authority::records(&config)
                .unwrap()
                .is_empty());
        }
        cleanup(&config);
    }
}
