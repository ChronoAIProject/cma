use super::{delivery_tests::*, test_support::*, *};
use crate::{
    channels::resolver::ResolvedChannel,
    delivery::{durable::receipt_for, intents::DeliveryOutcome},
};
use cmaeg_core::{channel::DefaultReply, seal::ReplyAddress};
use serde_json::json;
use std::sync::Arc;

async fn setup() -> Setup {
    Setup::with_request(supported(), fixture::request(|value| {
        fixture::targets(value);
        value["routing"]["consumers"][0]["reply"] = json!({"destination":"default"});
        value["routing"]["default_reply"] = json!({"type":"telegram_chat","source_id":"telegram","chat_id":"-200","thread_id":"7"});
    }, fixture::message("hello"))).await
}
fn redirect(setup: &Setup) {
    let mut current = setup.current.channel.lock().unwrap();
    let mut definition = current.definition.clone();
    let DefaultReply::TelegramChat { chat_id, .. } = definition
        .routing
        .as_mut()
        .unwrap()
        .default_reply
        .as_mut()
        .unwrap()
    else {
        unreachable!()
    };
    *chat_id = "-300".into();
    // Also prove that binding equality protects against an inconsistent
    // resolver snapshot that changes a destination without changing version.
    *current = Arc::new(ResolvedChannel {
        id: current.id.clone(),
        pointer: current.pointer.clone(),
        definition,
        agent_key_hash: current.agent_key_hash,
        consumer_keys: current.consumer_keys.clone(),
        loaded_at: current.loaded_at,
        version: current.version,
    });
}

#[tokio::test]
async fn default_delivery_keeps_original_identity_and_reuses_receipt_after_restart() {
    let setup = setup().await;
    let mut lease = accepted(&setup).await;
    let body = setup.journal().body(&lease).await.unwrap();
    let encoded = String::from_utf8(super::storage::bytes(&body).unwrap()).unwrap();
    assert!(!encoded.contains("fixture-channel-key"));
    assert!(!encoded.contains("77:fixture"));
    let (delivery, mut rx) = lane(&setup);
    let frame = recovery_frame("resp_1", 0, true);
    let (result, ()) = tokio::join!(
        delivery.deliver_frame(
            &setup.plan.identity,
            &setup.target().id,
            &mut lease,
            &frame,
            effect(),
        ),
        async {
            let reply = command(&mut rx).await;
            assert_eq!(reply.event, event(&body));
            assert!(matches!(&reply.default_destination,
            Some((DefaultDestination::Telegram { chat_id, thread_id, .. }, operation))
                if chat_id == "-200" && thread_id.as_deref() == Some("7") && operation.len() == 64));
            let _permit = reply.gate.attempt().await.unwrap();
            assert!(reply.gate.commit());
            reply
                .receipt
                .send(receipt_for(DeliveryOutcome::Confirmed {
                    platform_message_id: Some("sent-default".into()),
                }))
                .unwrap();
        }
    );
    assert!(matches!(result.unwrap(), FrameDelivery::Settled { .. }));
    setup.journal().release(&mut lease).await.unwrap();
    redirect(&setup);
    let mut resumed = accepted(&setup).await;
    assert!(matches!(
        delivery
            .deliver_frame(
                &setup.plan.identity,
                &setup.target().id,
                &mut resumed,
                &frame,
                effect(),
            )
            .await
            .unwrap(),
        FrameDelivery::Settled { .. }
    ));
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn destination_edit_refuses_queued_send_and_never_redirects_original_reply() {
    let setup = setup().await;
    let mut lease = accepted(&setup).await;
    let (delivery, mut rx) = lane(&setup);
    let frame = recovery_frame("resp_1", 0, false);
    let (result, ()) = tokio::join!(
        delivery.deliver_frame(
            &setup.plan.identity,
            &setup.target().id,
            &mut lease,
            &frame,
            effect(),
        ),
        async {
            let reply = command(&mut rx).await;
            redirect(&setup);
            assert!(reply.gate.attempt().await.is_err());
            reply
                .receipt
                .send(receipt_for(DeliveryOutcome::Rejected {
                    reason: "default_destination_revoked".into(),
                }))
                .unwrap();
        }
    );
    assert!(matches!(result.unwrap(), FrameDelivery::Settled { .. }));
    setup.journal().release(&mut lease).await.unwrap();
    let mut resumed = accepted(&setup).await;
    assert!(matches!(
        delivery
            .deliver_frame(
                &setup.plan.identity,
                &setup.target().id,
                &mut resumed,
                &recovery_frame("resp_1", 1, true),
                effect(),
            )
            .await,
        Err(ExecutionError::AuthorityChanged)
    ));
    assert_eq!(cursor(&resumed.state), Some(0));
    assert!(rx.try_recv().is_err());
}

#[test]
fn admission_refuses_sync_and_missing_default_before_any_execution() {
    let request = fixture::request(fixture::targets, fixture::message("hello"));
    let mut plan = fixture::routed(&request);
    assert_eq!(validate_routed_delivery(&request.channel, &plan), Ok(()));
    plan.targets[0].event.reply = ReplyAddress::Sync {
        waiter: "local-waiter".into(),
    };
    assert_eq!(
        validate_routed_delivery(&request.channel, &plan),
        Err(ExecutionError::Unsupported)
    );
    let mut request = fixture::request(fixture::targets, fixture::message("hello"));
    Arc::get_mut(&mut request.channel)
        .unwrap()
        .definition
        .routing
        .as_mut()
        .unwrap()
        .consumers[0]
        .reply
        .destination = cmaeg_core::channel::ReplyChoice::Default;
    plan.targets[0].event.reply = ReplyAddress::None;
    assert_eq!(
        validate_routed_delivery(&request.channel, &plan),
        Err(ExecutionError::Unsupported)
    );
}
