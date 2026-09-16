use super::*;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_execution_replaces_expired_claim_without_authorizing_the_stale_worker() {
    let c = ControlFixture::new().await;
    let held = worker::admit(&c, false, "expired-owner-control").await;
    let service = worker::service(&c);
    service
        .release_held(&held.command, &worker::operations(&c))
        .await
        .unwrap();
    let stamp = std::time::SystemTime::now();
    let claimed =
        c.f.resources
            .claim_next_command(CommandClaimRequest {
                agent_id: held.command.receipt.envelope.agent_id.clone(),
                holder: "departed-control-worker".into(),
                now: layer2_core::rfc3339_at(stamp),
                expires_at: layer2_core::rfc3339_at(stamp + Duration::from_secs(2)),
            })
            .await
            .unwrap()
            .unwrap();
    let stale = claimed.work.lease.as_ref().unwrap().claim.clone();
    assert!(service
        .ready_for_claim(&claimed, &worker::operations(&c))
        .await
        .is_err());
    let original_authority = service.authorize(&claimed).await.unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(original_authority.refreshed().await.is_err());
    assert!(service.authorize(&claimed).await.is_err());
    service
        .ready_for_claim(&claimed, &worker::operations(&c))
        .await
        .unwrap();
    let (execution, inputs) = worker::execution(&c, None, false);
    assert!(!matches!(
        execution.execute(&claimed).await,
        CommandAttempt::Finished
    ));
    assert!(inputs.lock().unwrap().is_empty());
    worker::run(&c, &held, execution.clone()).await;
    let completed =
        c.f.resources
            .get_command(&held.sidecar.command_id)
            .await
            .unwrap()
            .unwrap();
    let dispatch = completed.work.dispatch.as_ref().unwrap();
    assert!(dispatch.claim.fence > stale.fence);
    assert_ne!(dispatch.claim.holder, stale.holder);
    assert_eq!(dispatch.turn_sequence, held.command.target_turn.unwrap());
    assert_eq!(completed.receipt, held.command.receipt);
    assert_eq!(inputs.lock().unwrap().len(), 1);
    assert!(service.authorize(&claimed).await.is_err());
    assert!(!matches!(
        execution.execute(&claimed).await,
        CommandAttempt::Finished
    ));
    worker::run(&c, &held, execution).await;
    assert_eq!(inputs.lock().unwrap().len(), 1);
    c.f.close().await;
}
