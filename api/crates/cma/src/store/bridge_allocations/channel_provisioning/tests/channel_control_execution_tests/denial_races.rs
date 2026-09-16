use super::*;
use tokio::time::Instant;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_held_denial_cannot_overwrite_released_or_claimed_work() {
    for claim in [false, true] {
        let c = ControlFixture::new().await;
        let held = worker::admit(&c, false, "stale-held-denial").await;
        if claim {
            worker::claim(&c, &held).await;
        } else {
            worker::service(&c)
                .release_held(&held.command)
                .await
                .unwrap();
        }
        denial::consent(&c, 1, SourceControlAction::RevokeSenderControls).await;
        let before = denial::stored(&c, &held).await;
        assert!(denial::refusal_store(&c)
            .refuse_control_hold(&held.command, Instant::now() + Duration::from_secs(15))
            .await
            .is_err());
        assert!(denial::stored(&c, &held).await == before);
        assert!(before.command.work.dispatch.is_none());
        c.f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_held_denial_fences_previously_authorized_release_and_concurrent_workers() {
    let c = ControlFixture::new().await;
    let held = worker::admit(&c, true, "race-held-denial").await;
    let stale = worker::service(&c).authorize(&held.command).await.unwrap();
    denial::consent(&c, 1, SourceControlAction::RevokeSenderControls).await;
    let store = denial::refusal_store(&c);
    let (release, refuse_one, refuse_two) = tokio::join!(
        store.release_control_hold(stale.fence(), &held.command),
        store.refuse_control_hold(&held.command, Instant::now() + Duration::from_secs(15)),
        store.refuse_control_hold(&held.command, Instant::now() + Duration::from_secs(15)),
    );
    assert!(release.is_err());
    assert!(refuse_one.is_ok() || refuse_two.is_ok());
    denial::assert_refused(&c, &held, CommandFailureCode::AuthorityRevoked).await;
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_held_denial_proves_local_grant_revocation_without_external_authority() {
    let c = ControlFixture::new().await;
    let held = worker::admit(&c, false, "grant-held-denial").await;
    let bridge =
        c.f.bridges
            .bridge(&c.f.inbox.pin.bridge_id)
            .await
            .unwrap()
            .unwrap();
    c.f.invokes
        .revoke(
            &caller(),
            None,
            RevokeBridgeInvokeGrant {
                grant_id: c.f.grant.id.clone(),
                expected_bridge_revision: bridge.revision,
                expected_revision: 1,
                idempotency_key: "held-denial-grant-revocation".into(),
            },
        )
        .await
        .unwrap();
    let (execution, effects) = worker::execution(&c, None, false);
    worker::run(&c, &held, execution).await;
    assert!(effects.lock().unwrap().is_empty());
    denial::assert_refused(&c, &held, CommandFailureCode::AuthorityRevoked).await;
    c.f.close().await;
}
