use super::execution_tests::execution;
use super::*;
use std::{
    sync::atomic::Ordering,
    time::{Duration, SystemTime},
};

async fn admit_ui(
    c: &ChannelFixture,
    key: &str,
) -> Result<CommandStatusView, CommandAdmissionError> {
    generic::command_service(&c.f)
        .admit(
            &caller(),
            "fixture-login",
            CommandSubmissionSource::UserInterface,
            CommandSubmission {
                agent_id: c.f.target.agent_id.clone(),
                command: AgentCommand::message(key),
                idempotency_key: key.into(),
                expected_agent_revision: None,
                requested_model: None,
            },
        )
        .await
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn revoked_channel_hold_is_refused_without_dispatch_and_releases_actual_queue_capacity() {
    let c = ChannelFixture::new().await;
    let held = c
        .commands
        .reserve_channel_command(c.request().await)
        .await
        .unwrap();
    let (execution, sends) = execution(&c, false);
    let before =
        c.f.resources
            .agent_log(&c.f.target.agent_id, 0, 100)
            .await
            .unwrap();
    // This is a refusal port, not operator-supplied authority. An active grant
    // cannot be terminally rejected as revoked even by an internal caller.
    assert!(execution
        .channel
        .as_ref()
        .unwrap()
        .refuse_execution_hold(&held, ChannelHoldRefusalReason::GrantRevoked)
        .await
        .is_err());
    for index in 0..MAX_COMMAND_QUEUE_CAPACITY - 1 {
        admit_ui(&c, &format!("capacity-{index}")).await.unwrap();
    }
    assert!(matches!(
        admit_ui(&c, "after-refusal").await,
        Err(CommandAdmissionError::Ledger(CommandLedgerError::QueueFull))
    ));
    c.f.service
        .revoke(&caller(), None, c.f.revoke(&c.grant, "held-revoke"))
        .await
        .unwrap();
    AgentCommandWorker::new(
        c.f.resources.clone(),
        execution,
        "held-refusal-worker".into(),
    )
    .unwrap()
    .run_one(&held.receipt.envelope.command_id)
    .await
    .unwrap();
    let current =
        c.f.resources
            .get_command(&held.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(current.status, AgentResponseStatus::Failed);
    assert_eq!(
        current.work.failure.as_ref().unwrap().code,
        CommandFailureCode::AuthorityRevoked
    );
    assert!(current.work.dispatch.is_none() && current.work.lease.is_none());
    assert_eq!(current.receipt, held.receipt);
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    assert_eq!(
        c.f.resources
            .agent_log(&c.f.target.agent_id, 0, 100)
            .await
            .unwrap(),
        before
    );
    assert_eq!(
        admit_ui(&c, "after-refusal").await.unwrap().status,
        AgentResponseStatus::Queued
    );
    c.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn expired_channel_hold_is_terminal_but_active_unexpired_source_cannot_fake_timeout() {
    let c = ChannelFixture::new().await;
    let mut held = c
        .commands
        .reserve_channel_command(c.request().await)
        .await
        .unwrap();
    let (execution, sends) = execution(&c, false);
    assert!(execution
        .channel
        .as_ref()
        .unwrap()
        .refuse_execution_hold(&held, ChannelHoldRefusalReason::QueueTimeout)
        .await
        .is_err());
    let stamp = SystemTime::now();
    held.receipt.admitted_at = layer2_core::rfc3339_at(stamp - Duration::from_secs(600));
    held.queue_deadline = layer2_core::rfc3339_at(stamp - Duration::from_secs(1));
    let mut fields = crate::store::agent_commands::command_document(&held).unwrap();
    fields.remove("_id");
    c.f.resources
        .agent_commands
        .update_one(
            doc! {"_id":held.receipt.envelope.command_id.as_str()},
            doc! {"$set":fields},
        )
        .await
        .unwrap();
    AgentCommandWorker::new(
        c.f.resources.clone(),
        execution,
        "expired-held-worker".into(),
    )
    .unwrap()
    .run_one(&held.receipt.envelope.command_id)
    .await
    .unwrap();
    let current =
        c.f.resources
            .get_command(&held.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(current.status, AgentResponseStatus::Failed);
    assert_eq!(
        current.work.failure.as_ref().unwrap().code,
        CommandFailureCode::QueueTimeout
    );
    assert!(current.work.dispatch.is_none() && current.work.lease.is_none());
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    c.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn suspended_channel_hold_stays_resumable_and_stale_refusal_cannot_replace_released_command()
{
    let mut c = ChannelFixture::new().await;
    let held = c
        .commands
        .reserve_channel_command(c.request().await)
        .await
        .unwrap();
    let (execution, sends) = execution(&c, false);
    c.f.definitions
        .mutate(
            &caller(),
            None,
            BridgeMutation {
                id: c.f.bridge.id.clone(),
                expected_revision: c.f.bridge.revision,
                idempotency_key: "hold-suspend".into(),
                change: BridgeChange::Disable {
                    accepted_work: AcceptedWorkPolicy::SuspendOriginal,
                },
            },
        )
        .await
        .unwrap();
    execution.release_held(&held).await;
    assert_eq!(
        c.f.resources
            .get_command(&held.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(held.clone())
    );
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    c.f.bridge = c.f.bridges.bridge(&c.f.bridge.id).await.unwrap().unwrap();
    c.f.enable("hold-resume").await;
    execution.release_held(&held).await;
    let released =
        c.f.resources
            .get_command(&held.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(released.status, AgentResponseStatus::Queued);
    c.f.service
        .revoke(&caller(), None, {
            let mut revoke = c.f.revoke(&c.grant, "released-revoke");
            revoke.expected_bridge_revision =
                c.f.bridges
                    .bridge(&c.f.bridge.id)
                    .await
                    .unwrap()
                    .unwrap()
                    .revision;
            revoke
        })
        .await
        .unwrap();
    assert!(execution
        .channel
        .as_ref()
        .unwrap()
        .refuse_execution_hold(&held, ChannelHoldRefusalReason::GrantRevoked)
        .await
        .is_err());
    assert_eq!(
        c.f.resources
            .get_command(&held.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(released)
    );
    c.close().await;
}
