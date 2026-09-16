use super::*;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn retained_command_after_reset_refuses_cached_and_fresh_revoked_or_suspended_authority() {
    for revoke in [false, true] {
        let f = LifecycleFixture::new().await;
        let queued = f
            .queued_channel_command("reset-command-authority-denial")
            .await;
        reset(&f, &queued, &f.f.original.record).await;
        let authority = queued
            .channel
            .authorize_execution(&queued.invokes, &queued.command)
            .await
            .unwrap();
        let fence = authority.fence();
        let grant = fence.proof().grant();
        let bridge = fence.proof().bridge();
        if revoke {
            queued
                .invokes
                .revoke(
                    &caller(),
                    None,
                    RevokeBridgeInvokeGrant {
                        grant_id: grant.id.clone(),
                        expected_revision: grant.revision,
                        expected_bridge_revision: bridge.revision,
                        idempotency_key: "reset-command-revoke".into(),
                    },
                )
                .await
                .unwrap();
        } else {
            f.f.original
                .svc
                .mutate(
                    &caller(),
                    None,
                    change(
                        &bridge.id,
                        bridge.revision,
                        "reset-command-suspend",
                        BridgeChange::Disable {
                            accepted_work: AcceptedWorkPolicy::SuspendOriginal,
                        },
                    ),
                )
                .await
                .unwrap();
        }
        assert!(queued
            .channel
            .authorize_execution(&queued.invokes, &queued.command)
            .await
            .is_err());
        let mut session = f.f.original.client.start_session().await.unwrap();
        crate::store::sandbox_mongo::start_transaction(&mut session)
            .await
            .unwrap();
        assert!(queued
            .store
            .fence_channel_execution(&fence, &queued.command, &mut session)
            .await
            .is_err());
        session.abort_transaction().await.unwrap();
        assert_eq!(
            f.f.original
                .resources
                .get_command(&queued.command.receipt.envelope.command_id)
                .await
                .unwrap(),
            Some(queued.command)
        );
        f.close().await;
    }
}
