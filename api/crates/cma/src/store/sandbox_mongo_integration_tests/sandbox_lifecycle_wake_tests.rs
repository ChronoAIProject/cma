use super::*;
use crate::store::admission_wake_test_support::CommitGate;
use futures_util::FutureExt;
use std::time::Duration;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retirement_reauthorization_wakes_only_the_owners_parked_retirement() {
    use layer2_core::sandbox::RetirementCredentialRecovery;

    let (client, name) = database("retirement_wake").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut parked = sandbox("sbx_retirement_wake_parked");
    parked.status = SandboxStatus::Paused;
    parked.begin_retirement(NOW.into()).unwrap();
    parked.work.ready_at = "2026-10-06T00:00:00Z".into();
    parked.work.retirement_credential_recovery = Some(RetirementCredentialRecovery {
        newer_than_ms: 40,
        expected_scope: [1; 32],
        previous_token_generation: Some(5),
        authorization_updated: false,
        admitted: None,
    });
    let mut other = parked.clone();
    other.id = SandboxId("sbx_retirement_wake_other".into());
    other.owner = SubjectId::new("other");
    let mut ordinary = parked.clone();
    ordinary.id = SandboxId("sbx_retirement_wake_ordinary".into());
    ordinary.work.retirement_credential_recovery = None;
    for sandbox in [&parked, &other, &ordinary] {
        store.create_sandbox(sandbox.clone()).await.unwrap();
    }
    store
        .wake_retiring_sandboxes(&parked.owner, MINUTE)
        .await
        .unwrap();
    let mut expected = parked.clone();
    expected.revision += 1;
    expected.work.ready_at = MINUTE.into();
    expected
        .work
        .retirement_credential_recovery
        .as_mut()
        .unwrap()
        .authorization_updated = true;
    assert_eq!(store.get_sandbox(&parked.id).await.unwrap(), Some(expected));
    assert_eq!(store.get_sandbox(&other.id).await.unwrap(), Some(other));
    assert_eq!(
        store.get_sandbox(&ordinary.id).await.unwrap(),
        Some(ordinary)
    );
    assert!(store.sandbox_admitted.notified().now_or_never().is_some());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn lifecycle_admission_wake_follows_committed_pause_resume_and_delete_only() {
    for action in ["pause", "revive", "delete"] {
        let gate = Arc::new(CommitGate::default());
        let client = gate.client().await;
        let database = format!("cma_lifecycle_wake_{}", uuid::Uuid::new_v4().simple());
        let store = MongoSandboxStore::open(&client, &database).await.unwrap();
        let wake = store.sandbox_admitted.clone();
        let mut original = sandbox(&format!("sbx_wake_{action}"));
        if action == "revive" {
            original.status = SandboxStatus::Paused;
            original.runtime = None;
            original.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
            original.lifecycle.checkpoints.latest = Some(checkpoint(7));
        }
        store.create_sandbox(original.clone()).await.unwrap();
        assert!(wake.notified().now_or_never().is_none());
        let mut invalid = original.clone();
        invalid.begin_retirement(MINUTE.into()).unwrap();
        assert!(store
            .commit(SandboxTransaction::SaveSandboxAndAppend {
                command_revival: None,
                released_runtime: None,
                expected_revision: 999,
                sandbox: Box::new(invalid),
                event_at: MINUTE.into(),
                event: SandboxEventKind::StatusChanged {
                    from: original.status,
                    to: SandboxStatus::Retiring,
                },
            })
            .await
            .is_err());
        assert!(wake.notified().now_or_never().is_none());
        let writer = store.clone();
        let input = original.clone();
        gate.arm();
        let task = tokio::spawn(async move {
            if action == "delete" {
                let mut next = input.clone();
                next.begin_retirement(MINUTE.into()).unwrap();
                writer
                    .commit(SandboxTransaction::SaveSandboxAndAppend {
                        command_revival: None,
                        released_runtime: None,
                        expected_revision: input.revision,
                        sandbox: Box::new(next),
                        event_at: MINUTE.into(),
                        event: SandboxEventKind::StatusChanged {
                            from: input.status,
                            to: SandboxStatus::Retiring,
                        },
                    })
                    .await
                    .unwrap();
            } else {
                let command = if action == "pause" {
                    SandboxLifecycleCommand::pause(input.id, input.owner, MINUTE.into(), None)
                } else {
                    SandboxLifecycleCommand::revive(
                        input.id,
                        input.owner,
                        MINUTE.into(),
                        SandboxReviveCause::ExplicitRequest,
                    )
                };
                assert!(matches!(
                    writer.schedule_sandbox_lifecycle(command).await.unwrap(),
                    SandboxLifecycleCommandResult::Accepted(_)
                ));
            }
        });
        tokio::time::timeout(Duration::from_secs(5), gate.entered.notified())
            .await
            .unwrap();
        let premature = wake.notified().now_or_never().is_some();
        let before = store.get_sandbox(&original.id).await.unwrap().unwrap();
        gate.release();
        task.await.unwrap();
        assert!(!premature, "{action} cannot wake before durable commit");
        assert_eq!(before, original);
        assert!(wake.notified().now_or_never().is_some(), "{action}");
        assert!(wake.notified().now_or_never().is_none());
        let next = store.get_sandbox(&original.id).await.unwrap().unwrap();
        assert_ne!(next.status, original.status);
        assert!(next.work.is_due(MINUTE));
        assert!(store
            .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
                original.id,
                SubjectId::new("wrong-owner"),
                MINUTE.into(),
                None,
            ))
            .await
            .is_err());
        assert!(wake.notified().now_or_never().is_none());
        client.database(&database).drop().await.unwrap();
        client.shutdown().await;
    }
}
