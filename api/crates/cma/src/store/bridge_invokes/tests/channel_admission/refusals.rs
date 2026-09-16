use super::*;
use crate::bridge::custody::BridgeCredentialStore;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn exact_persisted_content_and_capabilities_are_required_and_arbitrary_text_cannot_use_inbox_proof(
) {
    let c = ChannelFixture::new().await;
    let request = c.request().await;
    for effect in [
        BridgeInvokeEffect::AdmitMessage {
            inbox_id: "different-inbox".into(),
        },
        BridgeInvokeEffect::ExecuteCommand {
            command_id: request.reservation().envelope.command_id.clone(),
        },
    ] {
        let other =
            c.f.service
                .authorize(c.f.request(&c.grant, effect))
                .await
                .unwrap();
        assert!(matches!(
            ChannelCommandReservation::prepare(other, c.inbox.clone(), c.allocation.clone()),
            Err(ChannelCommandAdmissionError::SourceMismatch)
        ));
    }
    let mut forged = c.inbox.clone();
    forged.envelope.text = Some("Replace the signed input".into());
    let forged =
        ChannelCommandReservation::prepare(request.proof().clone(), forged, c.allocation.clone())
            .unwrap();
    assert!(matches!(
        c.commands.reserve_channel_command(forged).await,
        Err(ChannelCommandAdmissionError::SourceMismatch)
    ));
    for change in 0..3 {
        let mut altered = c.inbox.clone();
        match change {
            0 => altered.capabilities = None,
            1 => altered.envelope.sender_id = "fabricated-owner".into(),
            _ => altered.envelope.content_type = "image".into(),
        }
        assert!(ChannelCommandReservation::prepare(
            request.proof().clone(),
            altered,
            c.allocation.clone()
        )
        .is_err());
    }
    let mut allocation = c.allocation.clone();
    allocation.state = layer2_core::bridge::allocation::AllocationState::Retired;
    assert!(ChannelCommandReservation::prepare(
        request.proof().clone(),
        c.inbox.clone(),
        allocation
    )
    .is_err());
    assert_eq!(
        c.f.resources
            .agent_commands
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    let admitted = c.commands.reserve_channel_command(request).await.unwrap();
    c.f.resources
        .agent_commands
        .update_one(
            doc! {"_id":admitted.receipt.envelope.command_id.as_str()},
            doc! {"$set":{"record.receipt.envelope.actor.sender_id":"corrupt-actor"}},
        )
        .await
        .unwrap();
    assert!(matches!(
        c.commands.reserve_channel_command(c.request().await).await,
        Err(ChannelCommandAdmissionError::Ledger(
            CommandLedgerError::Corrupt
        ))
    ));
    assert_eq!(
        c.f.resources
            .agent_commands
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    c.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn revoked_grant_refuses_both_new_commands_and_duplicate_receipts_without_minting_a_turn() {
    for replay in [false, true] {
        let c = ChannelFixture::new().await;
        if replay {
            c.commands
                .reserve_channel_command(c.request().await)
                .await
                .unwrap();
        }
        let proof_before_revocation = c.request().await;
        c.f.service
            .revoke(&caller(), None, c.f.revoke(&c.grant, "revoke"))
            .await
            .unwrap();
        assert!(matches!(
            c.commands
                .reserve_channel_command(proof_before_revocation)
                .await,
            Err(ChannelCommandAdmissionError::Invoke(
                BridgeInvokeError::Revoked
            ))
        ));
        assert_eq!(
            c.f.resources
                .agent_commands
                .count_documents(doc! {})
                .await
                .unwrap(),
            u64::from(replay)
        );
        c.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn duplicate_replay_fences_suspend_custody_allocation_and_target_creation_changes() {
    for change in 0..4 {
        let c = ChannelFixture::new().await;
        c.commands
            .reserve_channel_command(c.request().await)
            .await
            .unwrap();
        let original = c.request().await;
        match change {
            0 => {
                c.f.definitions
                    .mutate(
                        &caller(),
                        None,
                        BridgeMutation {
                            id: c.f.bridge.id.clone(),
                            expected_revision: c.f.bridge.revision,
                            idempotency_key: "suspend".into(),
                            change: BridgeChange::Disable {
                                accepted_work: AcceptedWorkPolicy::SuspendOriginal,
                            },
                        },
                    )
                    .await
                    .unwrap();
            }
            1 => {
                let management =
                    c.f.definitions
                        .management_lease(&caller(), None, &c.f.bridge.id, c.f.bridge.revision)
                        .await
                        .unwrap();
                c.f.bridges
                    .credentials
                    .revoke(
                        &management,
                        &c.f.bridge.current().unwrap().config.connector.custody,
                        1,
                        &"e".repeat(64),
                    )
                    .await
                    .unwrap();
            }
            2 => {
                c.f.client
                    .database(&c.f.database)
                    .collection::<Document>("bridge_allocations")
                    .update_one(
                        doc! {"_id": &c.allocation.id},
                        doc! {"$set":{"record.state":"retired"}},
                    )
                    .await
                    .unwrap();
            }
            _ => {
                c.f.resources.agents.update_one(doc! {"_id": c.f.target.agent_id.as_str()},
                    doc! {"$set":{"record.creation_fingerprint":"e".repeat(64),"creation_fingerprint":"e".repeat(64)}}).await.unwrap();
            }
        }
        assert!(c.commands.reserve_channel_command(original).await.is_err());
        assert_eq!(
            c.f.resources
                .agent_commands
                .count_documents(doc! {})
                .await
                .unwrap(),
            1
        );
        c.close().await;
    }
}
