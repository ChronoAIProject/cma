use super::*;
use layer2_core::sandbox::store::{
    AgentStore, SandboxLifecycleCommand, SandboxLifecycleCommandResult,
    SandboxLifecycleCommandStore,
};
use layer2_core::sandbox::{SandboxLifecycleOperation, SandboxPausePhase, SandboxReviveCause};

#[tokio::test]
async fn cancel_pause_reuses_live_runtime_and_releases_exact_hold_after_restart() {
    for captured in [false, true] {
        let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
        let created = harness.create().await;
        let now = start_time(&created);
        let active = harness.drive_active(&created.id, now).await;
        let agent = lifecycle_recovery_tests::prepare_agent(&harness, &active, now, 1).await;
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
        if captured {
            harness.run(&active.id, pause_at).await;
        }
        let pausing = harness.stored(&active.id).await;
        let token = pausing
            .lifecycle
            .operation
            .as_ref()
            .unwrap()
            .authority()
            .writer_resume_token
            .clone()
            .unwrap();
        let outcome = harness
            .store
            .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
                active.id.clone(),
                active.owner.clone(),
                rfc3339_at(pause_at),
                SandboxReviveCause::TriggerLaunch,
            ))
            .await
            .unwrap();
        let SandboxLifecycleCommandResult::Accepted(outcome) = outcome else {
            panic!("cancellation must be admitted");
        };
        assert_eq!(outcome.sandbox.status, SandboxStatus::Pausing);
        assert!(matches!(
            outcome.sandbox.lifecycle.operation,
            Some(SandboxLifecycleOperation::Pause {
                phase: SandboxPausePhase::ResumeWriters,
                ..
            })
        ));
        assert_eq!(
            outcome
                .sandbox
                .lifecycle
                .operation
                .as_ref()
                .unwrap()
                .authority()
                .writer_resume_token
                .as_deref(),
            Some(token.as_str())
        );
        harness.backend.fail_next(
            WireOperation::ResumeCheckpointWriters,
            "resume_unavailable",
            "try later",
        );
        harness.run(&active.id, pause_at).await;
        assert_eq!(
            harness.stored(&active.id).await.status,
            SandboxStatus::Pausing
        );
        let retry = pause_at + Duration::from_secs(5);
        lifecycle_recovery_tests::restarted_worker(&harness, retry)
            .await
            .run_one(&active.id, retry)
            .await
            .unwrap();
        let resumed = harness.stored(&active.id).await;
        assert_eq!(resumed.status, SandboxStatus::Active);
        assert_eq!(resumed.runtime, active.runtime);
        assert_eq!(
            harness.store.get_agent(&agent.id).await.unwrap(),
            Some(agent)
        );
        assert!(resumed.lifecycle.operation.is_none());
        assert_eq!(
            resumed.work.step,
            SandboxWorkStep::WaitForCredentialRotation
        );
        assert!(
            !harness
                .backend
                .snapshot(&RuntimeHandle(
                    active.runtime.as_ref().unwrap().runtime_id.clone()
                ))
                .await
                .unwrap()
                .writers_held
        );
        assert!(!harness
            .backend
            .requests()
            .await
            .iter()
            .any(|(_, request)| matches!(request, RequestFrame::Restore(_))));
    }
}
