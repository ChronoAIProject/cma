use super::*;

#[tokio::test]
async fn actor_continuation_capability_is_exact_retained_context_and_revocable() {
    let f = fixture().await;
    let (link, browser) = ready(&f).await;
    assert_eq!(approve(&f, &link, &browser).await.status(), StatusCode::OK);
    let pending = f.auth.pending_actor_wakeups(1).await.unwrap().remove(0);
    let reference = format!("cma1.cont.{}.{}", link, pending.notification_id);
    let (resolved, origin) = f
        .auth
        .authorized_actor_continuation(&reference)
        .await
        .unwrap();
    assert_eq!(resolved, actor());
    assert_eq!(origin.source_event.event_id, "event-a");
    for wrong in [
        format!("cma1.cont.{link}"),
        format!("{reference}.extra"),
        format!("cma1.cont.{link}.{}", "0".repeat(64)),
        format!("cma1.cont.{}.{}", "0".repeat(32), pending.notification_id),
    ] {
        assert!(f.auth.authorized_actor_continuation(&wrong).await.is_err());
    }
    let claim = f
        .auth
        .claim_actor_wakeup(&link, &pending.notification_id, "worker", 60)
        .await
        .unwrap()
        .unwrap();
    f.auth
        .authorized_actor_continuation(&reference)
        .await
        .unwrap();
    f.auth.ack_actor_wakeup(&claim, "receipt").await.unwrap();
    f.auth
        .authorized_actor_continuation(&reference)
        .await
        .unwrap();
    f.auth.revoke_actor_link(&actor(), &link).await.unwrap();
    assert!(f
        .auth
        .authorized_actor_continuation(&reference)
        .await
        .is_err());
}

#[tokio::test]
async fn actor_continuation_refuses_cancelled_expired_and_replaced_login() {
    for cause in ["cancelled", "expired", "replacement"] {
        let f = fixture().await;
        let (link, browser) = ready(&f).await;
        assert_eq!(approve(&f, &link, &browser).await.status(), StatusCode::OK);
        let pending = f.auth.pending_actor_wakeups(1).await.unwrap().remove(0);
        let reference = format!("cma1.cont.{}.{}", link, pending.notification_id);
        match cause {
            "cancelled" => {
                let claim = f
                    .auth
                    .claim_actor_wakeup(&link, &pending.notification_id, "worker", 60)
                    .await
                    .unwrap()
                    .unwrap();
                f.auth.cancel_actor_wakeup(&claim).await.unwrap();
            }
            "expired" => {
                let mut row = f.store.actor_link_by_id(&link).await.unwrap().unwrap();
                let revision = row.revision;
                row.origin.as_mut().unwrap().reply_expires_at = 1;
                row.revision += 1;
                f.store
                    .replace_actor_link(Some(revision), row)
                    .await
                    .unwrap();
            }
            _ => {
                f.seed().await;
            }
        }
        assert!(
            f.auth
                .authorized_actor_continuation(&reference)
                .await
                .is_err(),
            "{cause}"
        );
    }
}
