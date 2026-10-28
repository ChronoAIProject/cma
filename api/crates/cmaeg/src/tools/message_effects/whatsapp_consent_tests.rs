use super::*;
#[tokio::test]
async fn native_whatsapp_changes_consent_preserves_existing_received_reaction_authority() {
    let (app, graph) = fixture().await;
    let reference = reference(&app).await;
    let message = reference.open(&app.state.keys).unwrap().payload;
    let mut channel = (*app
        .state
        .resolver
        .resolve_current(app.event.channel_id.as_str())
        .await
        .unwrap())
    .clone();
    let cmaeg_core::channel::Source::Whatsapp { message_events, .. } =
        &mut channel.definition.sources[0]
    else {
        panic!()
    };
    *message_events = true;
    assert!(whatsapp_authority::check(&message, &channel, app.caller()).is_ok());
    assert!(graph.calls.lock().unwrap().is_empty());
}
