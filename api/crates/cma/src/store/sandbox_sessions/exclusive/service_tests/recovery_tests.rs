use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn revoked_authority_cancels_a_blocked_client_publication() {
    let f = Services::new(true).await;
    let mut connection = f
        .left
        .open_authorized(&f.caller, &f.agents[0], f.authority())
        .await
        .unwrap();
    f.stop_agent(0).await;
    let pending = std::future::pending::<()>();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), connection.while_authorized(pending))
            .await
            .unwrap()
            .is_err()
    );
    f.left.close_connection(&mut connection).await;
    f.stores.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn successful_heartbeats_preserve_one_stream_past_its_initial_deadline() {
    let f = Services::new(true).await;
    let mut connection = f
        .left
        .open_authorized(&f.caller, &f.agents[0], f.authority())
        .await
        .unwrap();
    let first = f.receiver.active_claim().await;
    for _ in 0..13 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        tokio::time::timeout(
            Duration::from_millis(100),
            connection.send(DesktopFrame::Binary(vec![1])),
        )
        .await
        .unwrap()
        .unwrap();
    }
    let renewed = f.receiver.active_claim().await;
    assert_eq!(renewed.epoch, first.epoch);
    assert_eq!(renewed.session_id, first.session_id);
    assert!(renewed.issued_at_ms > first.issued_at_ms);
    assert!(renewed.expires_at_ms > first.expires_at_ms);
    assert_eq!(f.receiver.frames(), 13);
    assert!(
        f.left
            .still_authorized(&f.caller, &f.agents[0], &mut connection)
            .await
    );
    assert!(matches!(
        f.right.available(&f.caller, &f.agents[1]).await,
        Err(DesktopError::Busy)
    ));
    f.left.close_connection(&mut connection).await;
    assert!(f.right.available(&f.caller, &f.agents[1]).await.is_ok());
    f.stores.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn uncertain_revoke_closes_local_frames_and_keeps_remote_discovery_busy() {
    let f = Services::new(true).await;
    let mut connection = f
        .left
        .open_authorized(&f.caller, &f.agents[0], f.authority())
        .await
        .unwrap();
    f.receiver.refuse_revoke();
    f.left.close_connection(&mut connection).await;
    assert!(connection
        .send(DesktopFrame::Binary(vec![1]))
        .await
        .is_err());
    assert!(matches!(
        f.right.available(&f.caller, &f.agents[1]).await,
        Err(DesktopError::Busy)
    ));
    assert!(matches!(
        f.right
            .open_authorized(&f.caller, &f.agents[1], f.authority())
            .await,
        Err(DesktopError::Busy)
    ));
    assert!(f
        .stores
        .right
        .has_open_sandbox_session(&f.stores.request.sandbox_id, &layer2_core::now_rfc3339())
        .await
        .unwrap());
    f.stores.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn remote_discovery_observes_prior_viewer_until_confirmed_handoff() {
    let f = Services::new(true).await;
    assert!(f.right.available(&f.caller, &f.agents[1]).await.is_ok());
    let mut first = f
        .left
        .open_authorized(&f.caller, &f.agents[0], f.authority())
        .await
        .unwrap();
    assert!(matches!(
        f.right.available(&f.caller, &f.agents[1]).await,
        Err(DesktopError::Busy)
    ));
    f.left.close_connection(&mut first).await;
    assert!(f.right.available(&f.caller, &f.agents[1]).await.is_ok());
    let mut next = f
        .right
        .open_authorized(&f.caller, &f.agents[1], f.authority())
        .await
        .unwrap();
    // Delayed cleanup still carries the prior token and cannot revoke its replacement.
    f.left.close_connection(&mut first).await;
    next.send(DesktopFrame::Binary(vec![1])).await.unwrap();
    assert!(matches!(
        f.left.available(&f.caller, &f.agents[0]).await,
        Err(DesktopError::Busy)
    ));
    f.right.close_connection(&mut next).await;
    f.stores.db.drop().await.unwrap();
}
