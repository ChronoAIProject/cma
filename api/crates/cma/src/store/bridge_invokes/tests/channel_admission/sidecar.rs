use super::*;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn held_commands_count_toward_capacity_and_duplicate_receipts_require_exact_versioned_sidecar(
) {
    let c = ChannelFixture::new().await;
    let request = c.request().await;
    let admitted = c
        .commands
        .reserve_channel_command(request.clone())
        .await
        .unwrap();
    let mut ordinary = request.reservation().clone();
    ordinary.envelope.actor = caller().actor;
    ordinary.envelope.source.kind = AgentSourceKind::UserInterface;
    ordinary.envelope.command_id = layer2_core::ids::AgentCommandId::generate();
    ordinary.envelope.authorization.authority_id = "actual-fixture-login".into();
    ordinary.envelope.command = AgentCommand::message("next UI command");
    ordinary.envelope.request_fingerprint = ordinary.envelope.command.fingerprint().unwrap();
    ordinary.key_sha256 = command_key_digest("bounded-ui-next").unwrap();
    ordinary.response_id = Some(layer2_core::ids::AgentResponseId::generate());
    ordinary.queue_capacity = 1;
    assert_eq!(
        c.f.resources.reserve_command(ordinary).await,
        Err(CommandLedgerError::QueueFull)
    );
    let id = admitted.receipt.envelope.command_id.as_str();
    let expected = record_bson(&request.sidecar().unwrap()).unwrap();
    for missing in [false, true] {
        let mutation = if missing {
            doc! {"$unset":{"channel_admission":""}}
        } else {
            doc! {"$set":{"channel_admission.response_id":"different-response"}}
        };
        c.f.resources
            .agent_commands
            .update_one(doc! {"_id":id}, mutation)
            .await
            .unwrap();
        assert!(matches!(
            c.commands.reserve_channel_command(c.request().await).await,
            Err(ChannelCommandAdmissionError::SourceMismatch)
        ));
        // The unchanged ordinary decoder remains readable even when the Channel
        // sidecar is unavailable; only the narrow adapter refuses its receipt.
        assert_eq!(
            c.f.resources
                .get_command(&admitted.receipt.envelope.command_id)
                .await
                .unwrap(),
            Some(admitted.clone())
        );
        c.f.resources
            .agent_commands
            .update_one(
                doc! {"_id":id},
                doc! {"$set":{"channel_admission":expected.clone()}},
            )
            .await
            .unwrap();
    }
    assert_eq!(
        c.commands
            .reserve_channel_command(c.request().await)
            .await
            .unwrap(),
        admitted
    );
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
