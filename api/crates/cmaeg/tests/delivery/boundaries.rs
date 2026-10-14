use super::support::App;
use cmaeg::{
    channels::resolver::ResolvedChannel, delivery::intents::DeliveryOutcome,
    turns::outcomes::TurnOutcome,
};
use serde_json::json;
use std::{sync::Arc, time::Duration};

#[tokio::test]
async fn admitted_turn_outlives_ref_expiry_and_uses_channel_key_for_final_delivery() {
    let mut app = App::standard(0).await;
    app.event.expires_at = super::relay_support::now() + 1;
    if let cmaeg_core::seal::ReplyAddress::NyxidRelay {
        reply_token_expires_at,
        ..
    } = &mut app.event.reply
    {
        *reply_token_expires_at = app.event.expires_at;
    }
    Arc::get_mut(&mut app.turn).unwrap().event_ref =
        cmaeg_core::seal::EventRef::seal(&app.event, &app.fixture.state.master_keys).unwrap();
    app.outcome(TurnOutcome::Accepted {
        response_id: "response".into(),
    })
    .await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    app.outcome(TurnOutcome::Completed {
        final_text: Some("late final".into()),
    })
    .await;
    app.drain().await;
    assert_eq!(app.fake.texts(), ["late final"]);
    assert_eq!(
        app.fake.requests.lock().unwrap()[0].authorization,
        "Bearer test-route-credential"
    );
}

#[tokio::test]
async fn partial_send_with_missing_route_key_is_unknown_and_retains_first_message_ref() {
    let mut app = App::standard(0).await;
    let turn = Arc::get_mut(&mut app.turn).unwrap();
    let previous = &turn.channel;
    let mut definition = previous.definition.clone();
    definition.secrets.channel_key = None;
    turn.channel = Arc::new(ResolvedChannel {
        id: previous.id.clone(),
        pointer: previous.pointer.clone(),
        definition,
        agent_key_hash: previous.agent_key_hash,
        consumer_keys: previous.consumer_keys.clone(),
        loaded_at: previous.loaded_at,
        version: previous.version,
    });
    let result = app.reply(json!({"text":"x".repeat(9000)})).await;
    assert_eq!(
        result.delivery,
        DeliveryOutcome::Unknown {
            reason: "partial_delivery".into()
        }
    );
    assert_eq!(
        result
            .message_ref
            .unwrap()
            .open(&app.fixture.state.master_keys)
            .unwrap()
            .payload
            .platform_message_id,
        "platform-1"
    );
    app.outcome(TurnOutcome::Completed {
        final_text: Some("must not duplicate a partial tool reply".into()),
    })
    .await;
    app.drain().await;
    assert_eq!(app.fake.texts(), ["x".repeat(4000)]);
}

#[tokio::test]
async fn consumed_reply_token_is_not_reused_after_terminal_state_cleanup() {
    let mut app = App::standard(0).await;
    app.reply(json!({"text":"first"})).await;
    app.outcome(TurnOutcome::Completed { final_text: None })
        .await;
    tokio::time::sleep(Duration::from_secs(31)).await;
    app.reply(json!({"text":"late"})).await;
    app.drain().await;
    let requests = app.fake.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].authorization, "Bearer test-route-credential");
    assert_ne!(requests[0].authorization, requests[1].authorization);
}

#[tokio::test]
async fn auto_final_off_still_allows_tools_and_does_not_send_committed_output() {
    let mut app = App::standard(0).await;
    let turn = Arc::get_mut(&mut app.turn).unwrap();
    let previous = &turn.channel;
    let mut definition = previous.definition.clone();
    definition.reply.auto_final = false;
    turn.channel = Arc::new(ResolvedChannel {
        id: previous.id.clone(),
        pointer: previous.pointer.clone(),
        definition,
        agent_key_hash: previous.agent_key_hash,
        consumer_keys: previous.consumer_keys.clone(),
        loaded_at: previous.loaded_at,
        version: previous.version,
    });
    app.outcome(TurnOutcome::Accepted {
        response_id: "response".into(),
    })
    .await;
    app.outcome(TurnOutcome::MessageCommitted {
        text: "silent".into(),
    })
    .await;
    app.outcome(TurnOutcome::Completed {
        final_text: Some("silent final".into()),
    })
    .await;
    app.reply(json!({"text":"explicit"})).await;
    app.drain().await;
    assert_eq!(app.fake.texts(), ["explicit"]);
}
