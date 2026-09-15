use super::*;
use layer2_core::bridge::outbox::*;

struct UnknownReply;
#[async_trait::async_trait]
impl RelayDeliveryPort for UnknownReply {
    async fn deliver(&self, _: RelayDeliveryRequest) -> DeliveryOutcome {
        DeliveryOutcome::Unknown(DeliveryFailure::ResponseLost)
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_child_cleanup_drain_preserves_accepted_dispatch_controls_and_early_replies() {
    let mut f = CleanupFixture::new(true).await;
    let queued = f.f.queued("drain-original-channel-message").await;
    f.reset().await;
    let original = f.request_cleanup().await.unwrap();
    assert!(original
        .holds
        .contains(&ChannelReuseCleanupHold::PendingCommand));
    f.close_admission().await.unwrap();
    let resources = &f.f.f.original.resources;
    assert_eq!(
        resources
            .get_command(&queued.command.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(queued.command.clone())
    );
    let now = SystemTime::now();
    let claimed = resources
        .claim_next_command(CommandClaimRequest {
            agent_id: queued.command.receipt.envelope.agent_id.clone(),
            holder: "drain-retained-dispatch".into(),
            now: layer2_core::rfc3339_at(now),
            expires_at: layer2_core::rfc3339_at(now + Duration::from_secs(30)),
        })
        .await
        .unwrap()
        .unwrap();
    let authority = queued
        .channel
        .authorize_execution(&queued.invokes, &claimed)
        .await
        .unwrap();
    f.operations()
        .execute_channel_claim(&authority, &claimed)
        .await
        .unwrap();
    let replies = f.f.reply_mid_turn(&queued).await;
    let pending = f.observe().await.unwrap();
    assert!(pending.holds.contains(&ChannelReuseCleanupHold::AgentBusy));
    assert!(pending
        .holds
        .contains(&ChannelReuseCleanupHold::PendingReply));
    let target = &queued.command.receipt.envelope.agent_id;
    let next = f
        .ordinary_request(
            target,
            "drain-new-message",
            AgentCommand::message("New work"),
        )
        .await;
    assert_eq!(
        resources.reserve_command(next).await.unwrap_err(),
        CommandLedgerError::NotAdmissible
    );
    assert!(f
        .operations()
        .send_message(&caller().caller, target, "New legacy queue item".into())
        .await
        .is_err());
    f.operations()
        .steer(&caller().caller, target, "Finish the accepted work".into())
        .await
        .unwrap();
    for (key, command) in [
        (
            "drain-steer",
            AgentCommand::Steer {
                input: AgentInput::text("Finish this turn"),
            },
        ),
        ("drain-interrupt", AgentCommand::Interrupt),
        (
            "drain-approval",
            AgentCommand::AnswerApproval {
                approval_id: "accepted-approval".into(),
                decision: AgentApprovalDecision::Deny,
            },
        ),
    ] {
        let request = f.ordinary_request(target, key, command).await;
        assert!(matches!(
            resources.reserve_command(request).await.unwrap(),
            CommandReservationResult::Created(_)
        ));
    }
    assert!(replies
        .deliver_one("drain-unknown-delivery", &UnknownReply)
        .await
        .unwrap());
    let uncertain = f.observe().await.unwrap();
    assert!(uncertain
        .holds
        .contains(&ChannelReuseCleanupHold::UncertainReply));
    assert!(uncertain
        .holds
        .contains(&ChannelReuseCleanupHold::PendingCommand));
    assert!(pending
        .holds
        .contains(&ChannelReuseCleanupHold::PendingReply));
    assert!(f.request_cleanup().await.unwrap() == original);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_child_cleanup_drain_refuses_closure_while_accepted_legacy_queue_awaits_revive() {
    let mut f = CleanupFixture::new(true).await;
    let target = f.original.placement.target().agent_id.clone();
    f.reset().await;
    let original = f.request_cleanup().await.unwrap();
    let paused = f.f.pause().await;
    assert!(matches!(
        f.operations()
            .send_message(&caller().caller, &target, "Accepted queue item".into())
            .await
            .unwrap(),
        AgentMessageOutcome::Queued { .. }
    ));
    let before =
        f.f.f
            .original
            .resources
            .get_agent(&target)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(before.conversation.queued_messages.len(), 1);
    assert!(f.close_admission().await.is_err());
    assert!(f.observe().await.is_err());
    let after =
        f.f.f
            .original
            .resources
            .get_agent(&target)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(after, before);
    assert_eq!(f.f.sandbox().await.id, paused.id);
    assert!(!f
        .row("agents", target.as_str())
        .await
        .contains_key(CLEANUP_ADMISSION_FIELD));
    assert_eq!(
        f.db()
            .collection::<Document>("channel_reuse_cleanup_admissions")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    let retained: ChannelReuseCleanupState = crate::store::sandbox_mongo::read_record(
        &f.row("channel_reuse_cleanup_requests", &original.id).await,
    )
    .unwrap();
    assert!(retained == original);
    f.close().await;
}
