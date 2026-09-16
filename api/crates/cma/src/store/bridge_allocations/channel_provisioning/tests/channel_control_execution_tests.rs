use super::*;
use crate::store::sandbox_mongo::record_bson;
use layer2_core::bridge::{channel_control::*, invoke::*};
use layer2_core::sandbox::{store::*, AgentLogKind, AgentOperationDelivery};
use mongodb::bson::Document;

use super::control_admission::fixture::ControlFixture;
#[path = "channel_control_execution_tests/accepted.rs"]
mod accepted;
#[path = "channel_control_execution_tests/accepted_race.rs"]
mod accepted_race;
#[path = "channel_control_execution_tests/boundaries.rs"]
mod boundaries;
#[path = "channel_control_execution_tests/compatibility.rs"]
pub(crate) mod compatibility;
#[path = "channel_control_execution_tests/denial.rs"]
mod denial;
#[path = "channel_control_execution_tests/denial_faults.rs"]
mod denial_faults;
#[path = "channel_control_execution_tests/denial_races.rs"]
mod denial_races;
#[path = "channel_control_execution_tests/local_runtime.rs"]
mod local_runtime;
#[path = "channel_control_execution_tests/local_runtime_fixture.rs"]
mod local_runtime_fixture;
#[path = "channel_control_execution_tests/production.rs"]
mod production;
#[path = "channel_control_execution_tests/races.rs"]
mod races;
#[path = "channel_control_execution_tests/reclaim.rs"]
mod reclaim;
#[path = "channel_control_execution_tests/temporal.rs"]
mod temporal;
#[path = "channel_control_execution_tests/worker.rs"]
mod worker;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_execution_common_worker_preserves_source_partial_output_and_dispatches_exact_control_once(
) {
    for steer in [false, true] {
        let c = ControlFixture::new().await;
        c.f.emit_assistant_text("before-control", "retained partial source output")
            .await;
        c.f.emit_assistant_text("before-boundary", "continuing source work")
            .await;
        c.f.await_assistant_text("retained partial source output")
            .await;
        let held = worker::admit(&c, steer, "owned-control").await;
        let historical = held.sidecar.intent.clone();
        // Real source reconciliation advances its revision after admission.
        c.f.emit_assistant_text(
            "after-control-admission",
            "source progress after control admission",
        )
        .await;
        c.f.emit_assistant_text("after-boundary", "source work remains active")
            .await;
        c.f.await_assistant_text("source progress after control admission")
            .await;
        c.f.resources
            .reconcile_command(&c.f.command.receipt.envelope.command_id)
            .await
            .unwrap();
        let progressed =
            c.f.resources
                .get_command(&c.f.command.receipt.envelope.command_id)
                .await
                .unwrap()
                .unwrap();
        assert!(progressed.revision > historical.observed_source_command_revision);
        let (execution, writes) = worker::execution(&c, None, false);
        worker::run(&c, &held, execution.clone()).await;
        let command =
            c.f.resources
                .get_command(&held.sidecar.command_id)
                .await
                .unwrap()
                .unwrap();
        assert!(
            command.work.dispatch.is_some(),
            "control must use the common dispatch transaction"
        );
        assert_eq!(
            command.work.dispatch.as_ref().unwrap().turn_sequence,
            historical.reference.binding().turn_sequence
        );
        assert!(command.receipt.response_id.is_none());
        worker::run(&c, &held, execution).await;
        {
            let inputs = writes.lock().unwrap();
            assert_eq!(inputs.len(), 1);
            match (&inputs[0], steer) {
                (layer3_adapter::ConversationInput::Cancel, false) => {}
                (layer3_adapter::ConversationInput::Prompt(blocks), true) => {
                    assert!(!blocks.is_empty())
                }
                _ => panic!("wrong effect for admitted Channel control"),
            }
        }
        let agent =
            c.f.resources
                .get_agent(&command.receipt.envelope.agent_id)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(agent.conversation.active_turn, command.target_turn);
        let logs = c.f.resources.agent_log(&agent.id, 0, 100).await.unwrap();
        assert_eq!(
            logs.iter()
                .filter(|entry| matches!(entry.kind, AgentLogKind::TurnStarted { .. }))
                .count(),
            1
        );
        assert!(logs.iter().any(|entry| matches!(&entry.kind, AgentLogKind::AgentMessage { content } if content == "retained partial source output")));
        assert!(!logs
            .iter()
            .any(|entry| matches!(entry.kind, AgentLogKind::ApprovalResolved { .. })));
        let row =
            c.f.client
                .database(&c.f.database)
                .collection::<Document>("bridge_channel_control_intents")
                .find_one(doc! {"_id":&historical.id})
                .await
                .unwrap()
                .unwrap();
        let after: ChannelControlIntent = crate::store::sandbox_mongo::read_record(&row).unwrap();
        assert!(
            after == historical,
            "execution never refreshes the permanent captured revisions"
        );
        assert_eq!(c.f.items().await.len(), 1, "no automatic outgoing message");
        c.f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_execution_ambiguous_writer_preserves_recovery_and_never_replays() {
    let c = ControlFixture::new().await;
    let held = worker::admit(&c, false, "ambiguous-control").await;
    let (execution, writes) = worker::execution(&c, None, true);
    worker::run(&c, &held, execution.clone()).await;
    worker::run(&c, &held, execution).await;
    let command =
        c.f.resources
            .get_command(&held.sidecar.command_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        command.work.observation.as_ref().unwrap().delivery,
        AgentOperationDelivery::RecoveryRequired
    );
    assert_eq!(writes.lock().unwrap().len(), 1);
    let (reopened, new_writes) = worker::execution(&c, None, false);
    worker::run(&c, &held, reopened).await;
    assert!(new_writes.lock().unwrap().is_empty());
    assert_eq!(c.f.items().await.len(), 1);
    c.f.close().await;
}
