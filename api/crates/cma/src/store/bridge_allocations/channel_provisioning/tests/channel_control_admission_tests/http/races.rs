use super::*;
use crate::bridge::ingress::controls::ChannelControlRuntimeEligibility;

struct RevokeAfterObservation {
    service: BridgeSourceControlService,
}

#[async_trait::async_trait]
impl ChannelControlRuntimeEligibility for RevokeAfterObservation {
    async fn require_available(
        &self,
        proof: &ChannelControlProof,
    ) -> Result<(), ChannelControlError> {
        proof.require_current()?;
        self.service
            .submit(
                &caller(),
                None,
                &proof.source_invoke().grant().id,
                "http-revoke-after-observation",
                SourceControlRequest {
                    expected_bridge_revision: proof.source_invoke().bridge().revision,
                    expected_grant_revision: proof.source_invoke().grant().revision,
                    expected_policy_revision: proof
                        .intent()
                        .reference
                        .binding()
                        .consent
                        .view
                        .revision,
                    action: SourceControlAction::RevokeSenderControls,
                },
            )
            .await
            .unwrap();
        Ok(())
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_http_revocation_after_observation_rolls_back_complete_v2_admission() {
    let c = ControlFixture::with_callback_route().await;
    let before_order = v2::order(&c).await;
    let before_rate = v2::rates(&c).await;
    let runtime = Arc::new(RevokeAfterObservation {
        service: BridgeSourceControlService::new(c.f.invokes.clone(), c.invokes.clone()),
    });
    let value = body(&c, "http-revocation-race", 540, false);
    let (status, result) = post(app(&c, true, Some(runtime)).await, &c, &value).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{result}");
    assert_eq!(rows(&c, "bridge_channel_control_intents").await, 0);
    assert_eq!(rows(&c, "bridge_channel_control_aliases").await, 0);
    assert_eq!(rows(&c, "agent_commands").await, 1);
    assert_eq!(rows(&c, "bridge_callback_inbox").await, 1);
    assert_eq!(rows(&c, "bridge_outbox_items").await, 1);
    assert_eq!(v2::order(&c).await, before_order);
    assert_eq!(v2::rates(&c).await, before_rate);
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_http_provider_disposition_failure_rolls_back_command_and_callback() {
    let c = ControlFixture::with_callback_route().await;
    let before_order = v2::order(&c).await;
    let before_rate = v2::rates(&c).await;
    c.f.client
        .database(&c.f.database)
        .run_command(doc! {"collMod":"bridge_callback_dispositions",
        "validator":{"$or":[{"kind":{"$ne":"provider"}},{"record.version":{"$ne":2_i32}}]},
        "validationLevel":"strict","validationAction":"error"})
        .await
        .unwrap();
    let value = body(&c, "http-provider-rollback", 541, false);
    let (status, result) = post(app(&c, true, Some(Runtime::local(&c))).await, &c, &value).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{result}");
    no_message_effects(&c, 0).await;
    assert_eq!(rows(&c, "bridge_channel_control_aliases").await, 0);
    assert_eq!(v2::order(&c).await, before_order);
    assert_eq!(v2::rates(&c).await, before_rate);
    c.f.close().await;
}
