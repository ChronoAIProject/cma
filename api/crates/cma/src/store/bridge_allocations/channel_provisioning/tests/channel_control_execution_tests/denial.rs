use super::*;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_held_denial_preserves_shared_reference_for_every_original_control() {
    let c = ControlFixture::new().await;
    c.f.resources
        .reconcile_command(&c.f.command.receipt.envelope.command_id)
        .await
        .unwrap();
    let first = c
        .store
        .reserve(c.request("shared-reference-cancel", 701, false).await)
        .await
        .unwrap();
    let second = c
        .store
        .reserve(c.request("shared-reference-steer", 702, true).await)
        .await
        .unwrap();
    assert!(first.sidecar.intent.reference == second.sidecar.intent.reference);
    let before =
        c.f.resources
            .get_agent(&first.command.receipt.envelope.agent_id)
            .await
            .unwrap()
            .unwrap();
    let logs = c.f.resources.agent_log(&before.id, 0, 100).await.unwrap();
    let source =
        c.f.resources
            .get_command(&c.f.command.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
    consent(&c, 1, SourceControlAction::RevokeSenderControls).await;
    let (execution, effects) = worker::execution(&c, None, false);
    worker::run(&c, &first, execution.clone()).await;
    assert_refused(&c, &first, CommandFailureCode::AuthorityRevoked).await;
    worker::run(&c, &second, execution).await;
    assert_eq!(stored(&c, &second).await.command.status, AgentResponseStatus::Failed,
        "the first refusal must leave the shared reference readable for the second original held control");
    assert_refused(&c, &second, CommandFailureCode::AuthorityRevoked).await;
    assert!(effects.lock().unwrap().is_empty());
    assert_eq!(
        c.f.resources.get_agent(&before.id).await.unwrap().unwrap(),
        before
    );
    assert_eq!(
        c.f.resources.agent_log(&before.id, 0, 100).await.unwrap(),
        logs
    );
    assert_eq!(
        c.f.resources
            .get_command(&source.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap(),
        source
    );
    assert_eq!(c.f.items().await.len(), 1);
    c.f.close().await;
}

pub(super) async fn consent(c: &ControlFixture, revision: u64, action: SourceControlAction) {
    let bridge =
        c.f.bridges
            .bridge(&c.f.inbox.pin.bridge_id)
            .await
            .unwrap()
            .unwrap();
    BridgeSourceControlService::new(c.f.invokes.clone(), c.invokes.clone())
        .submit(
            &caller(),
            None,
            &c.f.grant.id,
            &format!("denial-consent-{revision}"),
            SourceControlRequest {
                expected_bridge_revision: bridge.revision,
                expected_grant_revision: 1,
                expected_policy_revision: revision,
                action,
            },
        )
        .await
        .unwrap();
}

pub(super) async fn stored(
    c: &ControlFixture,
    held: &AdmittedChannelControl,
) -> AdmittedChannelControl {
    let row =
        c.f.client
            .database(&c.f.database)
            .collection::<Document>("agent_commands")
            .find_one(doc! {"_id":held.sidecar.command_id.as_str()})
            .await
            .unwrap()
            .unwrap();
    let result = AdmittedChannelControl {
        command: crate::store::agent_commands::read_command(&row).unwrap(),
        sidecar: mongodb::bson::deserialize_from_bson(
            row.get("channel_control_admission").unwrap().clone(),
        )
        .unwrap(),
    };
    result.validate().unwrap();
    result
}

pub(super) fn refusal_store(c: &ControlFixture) -> Arc<dyn ChannelControlExecutionStore> {
    Arc::new(crate::store::bridge_invokes::channel_control_execution::MongoChannelControlExecutionStore::new(c.invokes.clone()))
}

pub(super) async fn assert_refused(
    c: &ControlFixture,
    held: &AdmittedChannelControl,
    code: CommandFailureCode,
) {
    let current = stored(c, held).await;
    current.validate().unwrap();
    assert_eq!(current.command.status, AgentResponseStatus::Failed);
    assert_eq!(current.sidecar.execution_hold, ChannelControlHold::Refused);
    assert_eq!(current.command.work.failure.as_ref().unwrap().code, code);
    assert_eq!(current.command.receipt, held.command.receipt);
    assert_eq!(current.command.target_turn, held.command.target_turn);
    assert_eq!(current.command.key_sha256, held.command.key_sha256);
    assert!(current.sidecar.intent == held.sidecar.intent);
    assert_eq!(current.sidecar.intent_sha256, held.sidecar.intent_sha256);
    assert!(current.command.work.lease.is_none());
    assert!(current.command.work.dispatch.is_none());
    assert!(current.command.work.observation.is_none());
    assert_eq!(current.command.revision, held.command.revision + 1);
    let (restarted, effects) = worker::execution(c, None, false);
    worker::run(c, held, restarted).await;
    assert!(effects.lock().unwrap().is_empty());
    assert!(stored(c, held).await == current);
    assert_eq!(c.f.items().await.len(), 1);
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_held_denial_records_revoked_consent_without_changing_receipt_or_effects() {
    for reenabled in [false, true] {
        let c = ControlFixture::new().await;
        let held = worker::admit(&c, reenabled, "held-consent-denial").await;
        let logs =
            c.f.resources
                .agent_log(&held.command.receipt.envelope.agent_id, 0, 100)
                .await
                .unwrap();
        consent(&c, 1, SourceControlAction::RevokeSenderControls).await;
        if reenabled {
            consent(&c, 2, SourceControlAction::AllowSenderCancelAndSteer).await;
        }
        let (execution, effects) = worker::execution(&c, None, false);
        worker::run(&c, &held, execution).await;
        assert!(effects.lock().unwrap().is_empty());
        assert_refused(&c, &held, CommandFailureCode::AuthorityRevoked).await;
        assert_eq!(
            c.f.resources
                .agent_log(&held.command.receipt.envelope.agent_id, 0, 100)
                .await
                .unwrap(),
            logs
        );
        c.f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_held_denial_records_completed_source_or_later_turn_without_runtime_write()
{
    for later_turn in [false, true] {
        let c = ControlFixture::new().await;
        let held = worker::admit(&c, true, "held-source-denial").await;
        c.f.finish().await;
        c.f.resources
            .reconcile_command(&c.f.command.receipt.envelope.command_id)
            .await
            .unwrap();
        if later_turn {
            layer2_core::sandbox::AgentOperationService::new(
                c.f.resources.clone(),
                c.f.registry.clone(),
            )
            .execute_command(
                &caller().caller,
                &held.command.receipt.envelope.agent_id,
                AgentCommand::message("later owner turn remains untouched by refused control"),
            )
            .await
            .unwrap();
        }
        let logs =
            c.f.resources
                .agent_log(&held.command.receipt.envelope.agent_id, 0, 100)
                .await
                .unwrap();
        let agent =
            c.f.resources
                .get_agent(&held.command.receipt.envelope.agent_id)
                .await
                .unwrap()
                .unwrap();
        let (execution, effects) = worker::execution(&c, None, false);
        worker::run(&c, &held, execution).await;
        assert!(effects.lock().unwrap().is_empty());
        assert_refused(&c, &held, CommandFailureCode::TargetTurnChanged).await;
        assert_eq!(
            c.f.resources.agent_log(&agent.id, 0, 100).await.unwrap(),
            logs
        );
        assert_eq!(
            c.f.resources.get_agent(&agent.id).await.unwrap().unwrap(),
            agent
        );
        c.f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_held_denial_requires_complete_retained_evidence_and_same_incarnation() {
    for damage in ["intent", "reference", "history", "canonical", "incarnation"] {
        let c = ControlFixture::new().await;
        let held = worker::admit(&c, false, "unproved-held-denial").await;
        consent(&c, 1, SourceControlAction::RevokeSenderControls).await;
        let database = c.f.client.database(&c.f.database);
        match damage {
            "intent" => {
                database
                    .collection::<Document>("bridge_channel_control_intents")
                    .delete_one(doc! {"_id":&held.sidecar.intent.id})
                    .await
                    .unwrap();
            }
            "reference" => {
                database
                    .collection::<Document>("bridge_channel_control_references")
                    .delete_one(doc! {"_id":&held.sidecar.intent.reference.reference})
                    .await
                    .unwrap();
            }
            "history" => {
                database
                    .collection::<Document>("bridge_source_control_audits")
                    .delete_one(doc! {"grant_id":c.f.grant.id.as_str(),"revision":1_i64})
                    .await
                    .unwrap();
            }
            "canonical" => {
                database
                    .collection::<Document>("bridge_callback_dispositions")
                    .delete_one(doc! {"kind":"provider","record.version":2_i32})
                    .await
                    .unwrap();
            }
            "incarnation" => {
                database
                    .collection::<Document>("agents")
                    .update_one(
                        doc! {"_id":held.command.receipt.envelope.agent_id.as_str()},
                        doc! {"$set":{"record.created_at":"2001-01-01T00:00:00Z"}},
                    )
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let (execution, effects) = worker::execution(&c, None, false);
        worker::run(&c, &held, execution).await;
        assert!(effects.lock().unwrap().is_empty());
        assert!(
            stored(&c, &held).await == held,
            "uncertain {damage} cannot become a denial"
        );
        assert_eq!(c.f.items().await.len(), 1);
        c.f.close().await;
    }
}
