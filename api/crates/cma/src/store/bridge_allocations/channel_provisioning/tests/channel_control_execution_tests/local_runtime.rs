use super::local_runtime_fixture::*;
use super::*;
use futures_util::FutureExt;
use std::sync::atomic::{AtomicI64, Ordering};

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_local_nonowner_preserves_hold_and_cannot_steal_through_later_message() {
    for steer in [false, true] {
        let c = ControlFixture::new().await;
        let held = worker::admit(&c, steer, "local-nonowner-control").await;
        let owner = LocalRuntime::new(&c, &held, "owner", false);
        let nonowner = LocalRuntime::new(&c, &held, "missing", false);
        assert!(!Arc::ptr_eq(&owner.registry, &nonowner.registry));
        let owner_execution = execution(&c, owner.clone());
        let nonowner_execution = execution(&c, nonowner.clone());
        let owner_worker =
            local_runtime_fixture::worker(&c, owner_execution.clone(), "actual-owner-worker");
        let nonowner_worker =
            local_runtime_fixture::worker(&c, nonowner_execution.clone(), "actual-nonowner-worker");
        let before = state(&c, &held).await;
        nonowner_worker
            .run_one(&held.sidecar.command_id)
            .await
            .unwrap();
        assert_eq!(
            state(&c, &held).await,
            before,
            "nonowner must preserve held control and lane"
        );
        owner_execution.release_held(&held.command).await;
        let later = later_message(&c).await;
        let released = command(&c, &held).await;
        assert_eq!(released.status, AgentResponseStatus::Queued);
        assert_eq!(
            nonowner_execution.check_queued(&later).await,
            CommandReadiness::Ready
        );
        let before = state(&c, &held).await;
        nonowner_worker
            .run_one(&held.sidecar.command_id)
            .await
            .unwrap();
        nonowner_worker
            .run_one(&later.receipt.envelope.command_id)
            .await
            .unwrap();
        assert_eq!(
            state(&c, &held).await,
            before,
            "ordinary candidate must not steal the older control lease"
        );
        assert_eq!(
            c.f.resources
                .get_command(&later.receipt.envelope.command_id)
                .await
                .unwrap(),
            Some(later)
        );
        assert!(nonowner.writes.lock().unwrap().is_empty());
        owner_worker
            .run_one(&held.sidecar.command_id)
            .await
            .unwrap();
        assert!(command(&c, &held).await.work.dispatch.is_some());
        assert_eq!(owner.writes.lock().unwrap().len(), 1);
        assert!(matches!(
            (&owner.writes.lock().unwrap()[0], steer),
            (layer3_adapter::ConversationInput::Cancel, false)
                | (layer3_adapter::ConversationInput::Prompt(_), true)
        ));
        nonowner_worker
            .run_one(&held.sidecar.command_id)
            .await
            .unwrap();
        owner_worker
            .run_one(&held.sidecar.command_id)
            .await
            .unwrap();
        assert_eq!(owner.writes.lock().unwrap().len(), 1);
        assert!(nonowner.writes.lock().unwrap().is_empty());
        assert_eq!(c.f.items().await.len(), 1);
        c.f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_local_attachment_mismatch_or_capability_absence_remains_retryable() {
    for steer in [false, true] {
        for mode in [
            "agent",
            "generation",
            "conversation",
            "unpublished",
            "capability",
            "conversation-capability",
        ] {
            let c = ControlFixture::new().await;
            let held = worker::admit(&c, steer, "local-mismatch-control").await;
            let unavailable = LocalRuntime::new(&c, &held, mode, false);
            let execution = execution(&c, unavailable.clone());
            let worker =
                local_runtime_fixture::worker(&c, execution.clone(), "local-mismatch-worker");
            let before = state(&c, &held).await;
            worker.run_one(&held.sidecar.command_id).await.unwrap();
            assert_eq!(state(&c, &held).await, before, "{mode}");
            let owner = LocalRuntime::new(&c, &held, "owner", false);
            local_runtime_fixture::execution(&c, owner)
                .release_held(&held.command)
                .await;
            let before = state(&c, &held).await;
            let released = command(&c, &held).await;
            assert_eq!(
                execution.check_queued(&released).await,
                CommandReadiness::Retry,
                "{mode}"
            );
            worker.run_one(&held.sidecar.command_id).await.unwrap();
            assert_eq!(state(&c, &held).await, before, "{mode}");
            assert!(unavailable.writes.lock().unwrap().is_empty());
            assert_eq!(c.f.items().await.len(), 1);
            c.f.close().await;
        }
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_local_absence_does_not_hide_definitive_held_revocation_or_expiry() {
    for expired in [false, true] {
        let c = ControlFixture::new().await;
        let mut held = worker::admit(&c, false, "absent-runtime-definitive-denial").await;
        let nonowner = LocalRuntime::new(&c, &held, "missing", false);
        if expired {
            expire(&c, &mut held).await;
        } else {
            denial::consent(&c, 1, SourceControlAction::RevokeSenderControls).await;
        }
        let before = state(&c, &held).await;
        local_runtime_fixture::worker(&c, execution(&c, nonowner.clone()), "absent-denial-worker")
            .run_one(&held.sidecar.command_id)
            .await
            .unwrap();
        denial::assert_refused(
            &c,
            &held,
            if expired {
                CommandFailureCode::QueueTimeout
            } else {
                CommandFailureCode::AuthorityRevoked
            },
        )
        .await;
        let after = state(&c, &held).await;
        assert_eq!(after["agent"], before["agent"]);
        assert_eq!(after["logs"], before["logs"]);
        assert_eq!(nonowner.resolves.load(Ordering::SeqCst), 0);
        assert!(nonowner.writes.lock().unwrap().is_empty());
        assert_eq!(c.f.items().await.len(), 1);
        c.f.close().await;
    }
}

struct LoseAfterReadiness {
    execution: Arc<crate::commands::ResourceCommandExecution>,
    runtime: Arc<LocalRuntime>,
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_local_resolution_deadline_preserves_hold_without_false_refusal() {
    let c = ControlFixture::new().await;
    let held = worker::admit(&c, false, "local-timeout-control").await;
    let owner = LocalRuntime::new(&c, &held, "owner", false);
    let hook: worker::Hook = Box::new(|| {
        async {
            tokio::time::sleep(Duration::from_secs(16)).await;
            panic!("the original readiness deadline must cancel the local lookup");
        }
        .boxed()
    });
    *owner.hook.lock().unwrap() = Some((0, hook));
    let execution = execution(&c, owner.clone());
    let before = state(&c, &held).await;
    assert_eq!(
        execution
            .channel_controls
            .as_ref()
            .unwrap()
            .release_held(&held.command, &execution.operations)
            .await,
        Err(ChannelControlError::Unavailable),
    );
    assert_eq!(state(&c, &held).await, before);
    assert!(owner.writes.lock().unwrap().is_empty());
    assert_eq!(c.f.items().await.len(), 1);
    c.f.close().await;
}

#[async_trait::async_trait]
impl AgentCommandExecution for LoseAfterReadiness {
    async fn release_held(&self, command: &AdmittedAgentCommand) {
        self.execution.release_held(command).await;
    }
    async fn check_queued(&self, command: &AdmittedAgentCommand) -> CommandReadiness {
        let result = self.execution.check_queued(command).await;
        assert_eq!(result, CommandReadiness::Ready);
        self.runtime.detach();
        result
    }
    async fn prepare_queued(&self, _: &AdmittedAgentCommand) {
        panic!("control must not revive");
    }
    async fn execute(&self, command: &AdmittedAgentCommand) -> CommandAttempt {
        self.execution.execute(command).await
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_local_owner_loss_after_readiness_never_binds_or_sends() {
    let c = ControlFixture::new().await;
    let held = worker::admit(&c, false, "local-detached-control").await;
    let owner = LocalRuntime::new(&c, &held, "owner", false);
    let before = state(&c, &held).await;
    let execution = Arc::new(LoseAfterReadiness {
        execution: execution(&c, owner.clone()),
        runtime: owner.clone(),
    });
    local_runtime_fixture::worker(&c, execution, "detached-owner-worker")
        .run_one(&held.sidecar.command_id)
        .await
        .unwrap();
    let after = command(&c, &held).await;
    assert_eq!(after.status, AgentResponseStatus::Queued);
    assert!(after.work.lease.is_none());
    assert!(after.work.dispatch.is_none());
    assert!(after.work.failure.is_none());
    assert!(owner.writes.lock().unwrap().is_empty());
    let snapshot = state(&c, &held).await;
    assert_eq!(snapshot["agent"], before["agent"]);
    assert_eq!(snapshot["logs"], before["logs"]);
    assert_eq!(c.f.items().await.len(), 1);
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_local_unknown_binding_commit_recovers_once_and_nonowner_only_reconciles() {
    let mut c = ControlFixture::new().await;
    let held = worker::admit(&c, false, "local-unknown-binding").await;
    let application = temporal::scope_execution_commits(&mut c).await;
    let owner = LocalRuntime::new(&c, &held, "owner", false);
    let nonowner = LocalRuntime::new(&c, &held, "missing", false);
    let entered = Arc::new(AtomicI64::new(-1));
    let count = entered.clone();
    let admin = c.f.client.database("admin");
    let hook_admin = admin.clone();
    let hook: worker::Hook = Box::new(move || {
        async move {
        let value = hook_admin.run_command(doc! {
            "configureFailPoint":"failCommand", "mode":{"times":5_i32},
            "data":{"failCommands":["commitTransaction"], "appName":application,
                "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"owned binding acknowledgement lost"}},
        }).await.unwrap();
        count.store(temporal::failpoint_count(&value), Ordering::SeqCst);
    }.boxed()
    });
    // Held-release and exact claim readiness each resolve once. The third
    // resolve occurs after claim, immediately before conversation binding.
    *owner.hook.lock().unwrap() = Some((2, hook));
    let owner_worker =
        local_runtime_fixture::worker(&c, execution(&c, owner.clone()), "unknown-binding-owner");
    let result = owner_worker.run_one(&held.sidecar.command_id).await;
    let stopped = admin
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    result.unwrap();
    assert_eq!(
        temporal::failpoint_count(&stopped) - entered.load(Ordering::SeqCst),
        5
    );
    let committed = command(&c, &held).await;
    assert!(committed.work.dispatch.is_some());
    assert_eq!(owner.writes.lock().unwrap().len(), 1);
    let other = local_runtime_fixture::worker(
        &c,
        execution(&c, nonowner.clone()),
        "unknown-binding-nonowner",
    );
    other.run_one(&held.sidecar.command_id).await.unwrap();
    owner_worker
        .run_one(&held.sidecar.command_id)
        .await
        .unwrap();
    assert_eq!(
        command(&c, &held).await.work.dispatch,
        committed.work.dispatch
    );
    assert_eq!(owner.writes.lock().unwrap().len(), 1);
    assert_eq!(
        nonowner.resolves.load(Ordering::SeqCst),
        0,
        "a binding reconciles before local readiness"
    );
    assert!(nonowner.writes.lock().unwrap().is_empty());
    assert_eq!(c.f.items().await.len(), 1);
    c.f.close().await;
}
