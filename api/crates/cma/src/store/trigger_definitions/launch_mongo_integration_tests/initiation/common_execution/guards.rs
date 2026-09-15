use super::*;

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn trigger_common_and_legacy_dispatch_race_invokes_one_writer_and_holds_later_input() {
    let accepted = Accepted::new().await;
    let second = reuse::reserve_human(&accepted.f, &accepted.agent, "second-message").await;
    assert_eq!(second.receipt.agent_order, 2);
    let runtime = Runtime::new(false, false);
    let service = AgentOperationService::new(accepted.f.resources.clone(), runtime.clone());
    let claimed = accepted.claim().await;
    assert_eq!(claimed.receipt, accepted.command.receipt);
    let (common, legacy) = tokio::join!(
        service.execute_trigger_claim(&claimed),
        service.dispatch_queued_first_message(
            &accepted.agent.id,
            AgentWorkAuthority::new("startup".into(), 1).unwrap()
        ),
    );
    common.unwrap();
    assert!(matches!(
        legacy.unwrap(),
        FirstMessageDispatchOutcome::CommonCommand(_)
    ));
    assert_eq!(runtime.sends.load(Ordering::SeqCst), 1);
    assert!(accepted
        .f
        .resources
        .claim_next_command(CommandClaimRequest {
            agent_id: accepted.agent.id.clone(),
            holder: "other-worker".into(),
            now: layer2_core::now_rfc3339(),
            expires_at: expiry()
        })
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        accepted
            .f
            .resources
            .get_command(&second.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentResponseStatus::Queued
    );
    accepted.f.close().await;
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn trigger_common_stale_execution_proof_refuses_cleanup_before_runtime_writer() {
    for (collection, field) in [
        ("agents", "channel_reuse_cleanup_admission"),
        ("sandboxes", "channel_reuse_cleanup_execution"),
    ] {
        let accepted = Accepted::new().await;
        let runtime = Runtime::new(false, false);
        let service = AgentOperationService::new(accepted.f.resources.clone(), runtime.clone());
        let claimed = accepted.claim().await;
        let authority = service.authorize_trigger_claim(&claimed).await.unwrap();
        let id = if collection == "agents" {
            accepted.agent.id.as_str()
        } else {
            accepted.agent.sandbox_id.as_str()
        };
        accepted
            .f
            .seed
            .client
            .database(&accepted.f.seed.database)
            .collection::<Document>(collection)
            .update_one(doc! {"_id":id}, doc! {"$set":{field:{"closed":true}}})
            .await
            .unwrap();
        assert!(authority.current_proof().await.is_err());
        assert!(service.execute_trigger_claim(&claimed).await.is_err());
        assert_eq!(runtime.sends.load(Ordering::SeqCst), 0);
        assert_eq!(
            accepted
                .f
                .resources
                .get_command(&claimed.receipt.envelope.command_id)
                .await
                .unwrap()
                .unwrap()
                .receipt,
            accepted.command.receipt
        );
        accepted.f.close().await;
    }
}
