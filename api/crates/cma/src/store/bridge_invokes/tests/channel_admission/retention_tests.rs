use std::sync::atomic::Ordering;

use super::execution_tests::execution;
use super::*;
use crate::store::bridge_invokes::channel_source;

async fn command(c: &ChannelFixture) -> AdmittedAgentCommand {
    c.commands
        .reserve_channel_command(c.request().await)
        .await
        .unwrap()
}

async fn sidecar(c: &ChannelFixture, command: &AdmittedAgentCommand) -> ChannelCommandSidecar {
    let document =
        c.f.resources
            .agent_commands
            .find_one(doc! {"_id": command.receipt.envelope.command_id.as_str()})
            .await
            .unwrap()
            .unwrap();
    mongodb::bson::deserialize_from_bson(document.get("channel_admission").unwrap().clone())
        .unwrap()
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn expired_channel_source_survives_inbox_ttl_and_executes_original_accepted_command_once() {
    let c = ChannelFixture::new().await;
    let command = command(&c).await;
    let sidecar = sidecar(&c, &command).await;
    // Advance the persisted historical retention timestamp in this disposable
    // fixture. The signed input, accepted identity, grant and command stay exact.
    let mut aged = c.inbox.clone();
    let retention_seconds = aged.expires_at - aged.accepted_at;
    let expired_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        - 1;
    aged.expires_at = expired_at;
    aged.accepted_at = expired_at - retention_seconds;
    let archive = channel_source::archive(&aged, &sidecar).unwrap();
    c.f.resources
        .agent_commands
        .update_one(
            doc! {"_id": command.receipt.envelope.command_id.as_str()},
            doc! {"$set": {"channel_source": archive}},
        )
        .await
        .unwrap();
    let inboxes =
        c.f.client
            .database(&c.f.database)
            .collection::<Document>("bridge_callback_inbox");
    inboxes
        .update_one(
            doc! {"_id": &aged.id},
            doc! {"$set": {
                "record": record_bson(&aged).unwrap(),
                "expires_at": mongodb::bson::DateTime::from_millis(aged.expires_at * 1000),
            }},
        )
        .await
        .unwrap();
    let proof =
        c.f.service
            .authorize(c.f.request(
                &c.grant,
                BridgeInvokeEffect::AdmitMessage {
                    inbox_id: aged.id.clone(),
                },
            ))
            .await
            .unwrap();
    assert!(matches!(
        ChannelCommandReservation::prepare(proof, aged, c.allocation.clone()),
        Err(ChannelCommandAdmissionError::Unavailable)
    ));
    let service = ChannelAgentCommandService::new(c.commands.clone());
    service
        .authorize_execution(&c.f.service, &command)
        .await
        .unwrap();
    // Explicit deletion models the real TTL monitor without a timing-sensitive
    // wait. Execution must no longer depend on that expiring collection.
    inboxes.delete_one(doc! {"_id": &c.inbox.id}).await.unwrap();
    assert!(inboxes
        .find_one(doc! {"_id": &c.inbox.id})
        .await
        .unwrap()
        .is_none());
    let (execution, sends) = execution(&c, false);
    for worker in ["retention-one", "retention-reopened"] {
        AgentCommandWorker::new(c.f.resources.clone(), execution.clone(), worker.into())
            .unwrap()
            .run_one(&command.receipt.envelope.command_id)
            .await
            .unwrap();
    }
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    let current =
        c.f.resources
            .get_command(&command.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(current.receipt, command.receipt);
    assert_eq!(current.status, AgentResponseStatus::InProgress);
    let document =
        c.f.resources
            .agent_commands
            .find_one(doc! {"_id": command.receipt.envelope.command_id.as_str()})
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        document
            .get_document("channel_source")
            .unwrap()
            .get_document("inbox")
            .unwrap()
            .get_i64("expires_at")
            .unwrap(),
        expired_at
    );
    assert!(document.get_i64("channel_source_fence").unwrap() > 0);
    c.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_source_archive_rejects_missing_archive_corruption_and_live_inbox_substitution() {
    for case in ["missing", "corrupt", "live-substitute"] {
        let c = ChannelFixture::new().await;
        let command = command(&c).await;
        let inboxes =
            c.f.client
                .database(&c.f.database)
                .collection::<Document>("bridge_callback_inbox");
        if case == "live-substitute" {
            inboxes
                .update_one(
                    doc! {"_id": &c.inbox.id},
                    doc! {"$set": {"record.envelope.text": "substituted text"}},
                )
                .await
                .unwrap();
        } else {
            inboxes.delete_one(doc! {"_id": &c.inbox.id}).await.unwrap();
            let update = if case == "missing" {
                doc! {"$unset": {"channel_source": ""}}
            } else {
                doc! {"$set": {"channel_source.command_correlation.command_id": "cmd_foreign"}}
            };
            c.f.resources
                .agent_commands
                .update_one(
                    doc! {"_id": command.receipt.envelope.command_id.as_str()},
                    update,
                )
                .await
                .unwrap();
        }
        let (execution, sends) = execution(&c, false);
        AgentCommandWorker::new(c.f.resources.clone(), execution, "retention-refusal".into())
            .unwrap()
            .run_one(&command.receipt.envelope.command_id)
            .await
            .unwrap();
        assert_eq!(sends.load(Ordering::SeqCst), 0, "{case}");
        assert_eq!(
            c.f.resources
                .get_command(&command.receipt.envelope.command_id)
                .await
                .unwrap(),
            Some(command),
            "{case}"
        );
        c.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn legacy_channel_command_without_archive_uses_exact_original_inbox_and_can_backfill_on_replay(
) {
    let c = ChannelFixture::new().await;
    let command = command(&c).await;
    c.f.resources
        .agent_commands
        .update_one(
            doc! {"_id": command.receipt.envelope.command_id.as_str()},
            doc! {"$unset": {"channel_source": ""}},
        )
        .await
        .unwrap();
    ChannelAgentCommandService::new(c.commands.clone())
        .authorize_execution(&c.f.service, &command)
        .await
        .unwrap();
    assert_eq!(
        c.commands
            .reserve_channel_command(c.request().await)
            .await
            .unwrap(),
        command
    );
    let document =
        c.f.resources
            .agent_commands
            .find_one(doc! {"_id": command.receipt.envelope.command_id.as_str()})
            .await
            .unwrap()
            .unwrap();
    assert!(document.get_document("channel_source").is_ok());
    let (execution, sends) = execution(&c, false);
    AgentCommandWorker::new(c.f.resources.clone(), execution, "legacy-source".into())
        .unwrap()
        .run_one(&command.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    c.close().await;
}
