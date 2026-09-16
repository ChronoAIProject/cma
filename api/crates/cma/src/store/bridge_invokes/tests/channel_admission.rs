use super::super::channel::MongoChannelCommandStore;
use super::*;
use layer2_core::sandbox::agent_io::*;
use layer2_core::sandbox::store::AgentStore;

mod contention;
mod execution_tests;
mod feature_gates;
pub(in crate::store) mod fixtures;
mod generic;
mod hold_refusal_tests;
mod missing_source_tests;
mod refusals;
mod retention_tests;
mod sidecar;
mod usage_management;
mod usage_tests;
use fixtures::ChannelFixture;
use generic::{ui_command, NoExecution};

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn signed_channel_replay_restart_and_ui_share_one_order_with_actual_actor_and_execution_hold()
{
    let c = ChannelFixture::new().await;
    let target = ResourceTarget::Agent {
        agent_id: c.f.target.agent_id.clone(),
    };
    let before =
        c.f.resources
            .resource_access_snapshot(&target)
            .await
            .unwrap();
    let ui_before = ui_command(&c.f, "ui-before").await;
    let log_before =
        c.f.resources
            .agent_log(&c.f.target.agent_id, 0, 50)
            .await
            .unwrap();
    let a = c.request().await;
    let b = c.request().await;
    let (one, two) = tokio::join!(
        c.commands.reserve_channel_command(a.clone()),
        c.commands.reserve_channel_command(b)
    );
    let record = one.unwrap();
    assert_eq!(two.unwrap(), record);
    let ui_after = ui_command(&c.f, "ui-after").await;
    assert_eq!(
        [
            ui_before.receipt.agent_order,
            record.receipt.agent_order,
            ui_after.receipt.agent_order
        ],
        [1, 2, 3]
    );
    assert_eq!(
        record.receipt.envelope.actor,
        AgentActor::Channel {
            bridge_id: c.f.bridge.id.as_str().into(),
            grant_id: c.grant.id.as_str().into(),
            platform: "telegram".into(),
            chat_id: "-100123".into(),
            sender_id: "12345".into(),
        }
    );
    assert_eq!(record.status, AgentResponseStatus::WaitingAction);
    assert_eq!(record.work, CommandWorkState::default());
    generic::assert_waiting_view(&c.f, &record).await;
    assert_eq!(record.receipt.envelope.source.reference, c.inbox.id);
    assert_eq!(record.receipt.envelope.correlation_id, c.inbox.id);
    assert_eq!(
        c.f.resources.reserve_command(a.reservation().clone()).await,
        Err(CommandLedgerError::Invalid)
    );
    let reopened = Arc::new(
        MongoBridgeInvokeStore::open(c.f.bridges.clone(), c.f.resources.clone(), &c.f.database)
            .await
            .unwrap(),
    );
    let reopened = MongoChannelCommandStore::new(reopened);
    assert_eq!(
        reopened
            .reserve_channel_command(c.request().await)
            .await
            .unwrap(),
        record
    );
    assert_eq!(
        c.f.resources
            .get_response_command(record.receipt.response_id.as_ref().unwrap())
            .await
            .unwrap(),
        Some(record.clone())
    );
    let worker = AgentCommandWorker::new(
        c.f.resources.clone(),
        Arc::new(NoExecution),
        "generic-worker".into(),
    )
    .unwrap();
    worker
        .run_one(&record.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(
        c.f.resources
            .get_command(&record.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(record.clone())
    );
    let inbox =
        c.f.client
            .database(&c.f.database)
            .collection::<Document>("bridge_callback_inbox")
            .find_one(doc! {"_id": &c.inbox.id})
            .await
            .unwrap()
            .unwrap();
    let link = inbox.get_document("command_correlation").unwrap();
    assert_eq!(
        link.get_str("command_id").unwrap(),
        record.receipt.envelope.command_id.as_str()
    );
    assert_eq!(
        link.get_str("response_id").unwrap(),
        record.receipt.response_id.as_ref().unwrap().as_str()
    );
    assert_eq!(link.get_str("allocation_id").unwrap(), c.allocation.id);
    let retained =
        c.f.resources
            .agent_commands
            .find_one(doc! {"_id":record.receipt.envelope.command_id.as_str()})
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        retained.get("channel_admission"),
        Some(&record_bson(&a.sidecar().unwrap()).unwrap())
    );
    assert_eq!(
        c.f.resources
            .resource_access_snapshot(&target)
            .await
            .unwrap(),
        before
    );
    assert_eq!(
        c.f.resources
            .agent_log(&c.f.target.agent_id, 0, 50)
            .await
            .unwrap(),
        log_before
    );
    generic::assert_ordinary_claim(&c.f, ui_before.receipt.command_id).await;
    assert!(generic::ordinary_claim(&c.f).await.is_none());
    // Refusing the original held Channel head releases its position. A later
    // UI command must never overtake it while authority is still pending.
    c.f.service
        .revoke(&caller(), None, c.f.revoke(&c.grant, "fifo-held-revoke"))
        .await
        .unwrap();
    let (execution, sends) = execution_tests::execution(&c, false);
    execution.release_held(&record).await;
    let refused =
        c.f.resources
            .get_command(&record.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(refused.receipt, record.receipt);
    assert_eq!(refused.status, AgentResponseStatus::Failed);
    assert_eq!(
        refused.work.failure.unwrap().code,
        CommandFailureCode::AuthorityRevoked
    );
    assert_eq!(sends.load(std::sync::atomic::Ordering::SeqCst), 0);
    generic::assert_ordinary_claim(&c.f, ui_after.receipt.command_id).await;
    c.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn command_failure_rolls_back_inbox_correlation_and_does_not_consume_agent_order() {
    let c = ChannelFixture::new().await;
    let first = ui_command(&c.f, "ui-before").await;
    c.f.client
        .database(&c.f.database)
        .run_command(doc! {"collMod":"agent_commands",
        "validator":{"forced_failure":true},"validationLevel":"strict","validationAction":"error"})
        .await
        .unwrap();
    assert!(c
        .commands
        .reserve_channel_command(c.request().await)
        .await
        .is_err());
    let inbox =
        c.f.client
            .database(&c.f.database)
            .collection::<Document>("bridge_callback_inbox")
            .find_one(doc! {"_id": &c.inbox.id})
            .await
            .unwrap()
            .unwrap();
    assert!(inbox.get("command_correlation").is_none());
    assert!(inbox.get("command_admission_fence").is_none());
    assert_eq!(
        c.f.resources
            .agent_commands
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    c.f.client
        .database(&c.f.database)
        .run_command(doc! {"collMod":"agent_commands","validator":{}})
        .await
        .unwrap();
    let admitted = c
        .commands
        .reserve_channel_command(c.request().await)
        .await
        .unwrap();
    assert_eq!(admitted.receipt.agent_order, first.receipt.agent_order + 1);
    c.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn held_channel_head_preserves_active_turn_controls_without_releasing_later_ui_messages() {
    let c = ChannelFixture::new().await;
    let held = c
        .commands
        .reserve_channel_command(c.request().await)
        .await
        .unwrap();
    let mut agent =
        c.f.resources
            .get_agent(&c.f.target.agent_id)
            .await
            .unwrap()
            .unwrap();
    let revision = agent.revision;
    agent.status = layer2_core::sandbox::AgentStatus::Working;
    agent.conversation.active_turn = Some(42);
    agent.capabilities = layer2_core::RuntimeCapabilities::new([
        layer2_core::RuntimeCapability::Conversation,
        layer2_core::RuntimeCapability::Interrupt,
    ]);
    agent.revision += 1;
    c.f.resources.save_agent(revision, agent).await.unwrap();
    let later = ui_command(&c.f, "after-held-active-channel").await;
    let control = generic::command_service(&c.f)
        .admit(
            &caller(),
            "fixture-login",
            CommandSubmissionSource::UserInterface,
            CommandSubmission {
                agent_id: c.f.target.agent_id.clone(),
                command: AgentCommand::Interrupt,
                idempotency_key: "interrupt-before-held-message".into(),
                expected_agent_revision: None,
                requested_model: None,
            },
        )
        .await
        .unwrap();
    let selected = generic::ordinary_claim(&c.f).await.unwrap();
    assert_eq!(
        selected.receipt.envelope.command_id,
        control.receipt.command_id
    );
    assert_eq!(selected.target_turn, Some(42));
    c.f.resources
        .reject_command(
            &selected.work.lease.as_ref().unwrap().claim,
            CommandFailureCode::AuthorityRevoked,
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    assert!(generic::ordinary_claim(&c.f).await.is_none());
    assert_eq!(
        c.f.resources
            .get_command(&held.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(held)
    );
    assert_eq!(
        c.f.resources
            .get_command(&later.receipt.command_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentResponseStatus::Queued
    );
    c.close().await;
}
