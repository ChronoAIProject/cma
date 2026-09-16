use super::*;
use futures_util::FutureExt;

async fn consent(c: &ControlFixture, revision: u64, action: SourceControlAction, key: &str) {
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
            key,
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

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_execution_reenabled_consent_never_revives_an_admitted_control() {
    let c = ControlFixture::new().await;
    let held = worker::admit(&c, true, "old-consent-control").await;
    consent(
        &c,
        1,
        SourceControlAction::RevokeSenderControls,
        "revoke-control-consent",
    )
    .await;
    consent(
        &c,
        2,
        SourceControlAction::AllowSenderCancelAndSteer,
        "new-control-consent",
    )
    .await;
    assert!(worker::service(&c).authorize(&held.command).await.is_err());
    let (execution, writes) = worker::execution(&c, None, false);
    worker::run(&c, &held, execution).await;
    assert!(writes.lock().unwrap().is_empty());
    assert!(c
        .f
        .resources
        .get_command(&held.sidecar.command_id)
        .await
        .unwrap()
        .unwrap()
        .work
        .dispatch
        .is_none());
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_execution_fresh_fence_rejects_stale_claim_source_runtime_and_conversation()
{
    for change in ["claim", "turn", "runtime", "conversation"] {
        let c = ControlFixture::new().await;
        let held = worker::admit(&c, false, "fenced-control").await;
        let claimed = worker::claim(&c, &held).await;
        let authority = worker::service(&c).authorize(&claimed).await.unwrap();
        let database = c.f.client.database(&c.f.database);
        if change == "claim" {
            c.f.resources
                .release_command_claim(
                    claimed
                        .work
                        .lease
                        .as_ref()
                        .map(|lease| &lease.claim)
                        .unwrap(),
                    layer2_core::now_rfc3339(),
                )
                .await
                .unwrap();
        } else {
            let update = match change {
                "turn" => doc! {"$inc":{"record.conversation.active_turn":1_i64}},
                "runtime" => doc! {"$inc":{"record.conversation.runtime_generation":1_i64}},
                "conversation" => {
                    doc! {"$set":{"record.conversation.conversation_id":"another-conversation"}}
                }
                _ => unreachable!(),
            };
            database
                .collection::<Document>("agents")
                .update_one(
                    doc! {"_id":claimed.receipt.envelope.agent_id.as_str()},
                    update,
                )
                .await
                .unwrap();
        }
        assert!(authority.refreshed().await.is_err(), "{change}");
        let (execution, writes) = worker::execution(&c, None, false);
        assert!(!matches!(
            execution.execute(&claimed).await,
            CommandAttempt::Finished
        ));
        assert!(writes.lock().unwrap().is_empty());
        c.f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_execution_ordered_writer_rechecks_revocation_after_durable_binding() {
    let c = ControlFixture::new().await;
    let held = worker::admit(&c, false, "writer-revoked-control").await;
    let bridge =
        c.f.bridges
            .bridge(&c.f.inbox.pin.bridge_id)
            .await
            .unwrap()
            .unwrap();
    let invokes = c.f.invokes.clone();
    let grant_id = c.f.grant.id.clone();
    let hook: worker::Hook = Box::new(move || {
        async move {
            invokes
                .revoke(
                    &caller(),
                    None,
                    RevokeBridgeInvokeGrant {
                        grant_id,
                        expected_bridge_revision: bridge.revision,
                        expected_revision: 1,
                        idempotency_key: "writer-control-revoke".into(),
                    },
                )
                .await
                .unwrap();
        }
        .boxed()
    });
    let (execution, writes) = worker::execution(&c, Some(hook), false);
    worker::run(&c, &held, execution).await;
    let command =
        c.f.resources
            .get_command(&held.sidecar.command_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        command.work.observation.as_ref().unwrap().delivery,
        AgentOperationDelivery::PreWriteFailed
    );
    assert!(writes.lock().unwrap().is_empty());
    assert_eq!(c.f.items().await.len(), 1);
    c.f.close().await;
}
