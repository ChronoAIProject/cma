#[path = "support/wire.rs"]
mod wire;

use cma_sdk::bridges::{BridgePlatform, BridgeStableThread};
use serde_json::json;
use wire::WireServer;

#[tokio::test]
async fn additional_platform_choices_preserve_distinct_wire_identity() {
    let server = WireServer::new().await;
    let _ = (&server.base_url, &server.anonymous);
    server.expect(wire::json(
        "GET",
        "/api/v1/bridge-connectors",
        None,
        200,
        json!({"bots":[
            {"id":"slack-bot","platform":"slack","active":true},
            {"id":"whatsapp-bot","platform":"whatsapp","active":true},
            {"id":"x-bot","platform":"x","active":true}
        ],"keys":[],"routes":[]}),
        true,
    ));
    let choices = server.client.bridges().choices(None).await.unwrap();
    for (bot, platform) in choices.bots.iter().zip([
        BridgePlatform::Slack,
        BridgePlatform::Whatsapp,
        BridgePlatform::X,
    ]) {
        assert_eq!(bot.platform, platform);
    }
    assert_eq!(server.request_count(), 1);
    server.assert_drained();
}

#[tokio::test]
async fn normalized_identity_and_thread_evidence_do_not_claim_text_admission_or_delivery() {
    let server = WireServer::new().await;
    let _ = (&server.base_url, &server.anonymous);
    let connectors: Vec<_> = ["slack", "whatsapp", "x"].map(|platform| json!({
        "platform":platform,"evidence":"contract_tested","live_verified":false,
        "inbound_text":false,"stable_sender_chat":true,
        "stable_thread": if platform == "slack" { Some("slack_thread") } else { None },
        "reply_to_bot":false,"bot_mentions":false,"inbound_edits":false,"inbound_deletes":false,
        "verified_actions":false,"attachment_input":"unavailable","attachment_output":false,
        "output_text":false,"output_edit":false,"output_progress":false,"output_limit_utf16":null,"output_format":"unavailable",
        "stages":{"management":false,"ingress":false,"allocation":false,"execution":false,"outbound":false}
    })).into();
    server.expect(wire::json(
        "GET",
        "/api/v1/bridge-connectors/capabilities",
        None,
        200,
        json!({"version":1,"source_revision":"85607255ee0beadca460a350e0aa6d177edd2fd2",
            "connection_validation_available":true,"connectors":connectors}),
        true,
    ));
    let catalog = server.client.bridges().capabilities(None).await.unwrap();
    assert_eq!(
        catalog.connectors[0].stable_thread,
        Some(BridgeStableThread::SlackThread)
    );
    for connector in catalog.connectors {
        assert!(connector.stable_sender_chat);
        assert!(!connector.inbound_text && !connector.output_text && !connector.live_verified);
        assert!(!connector.stages.ingress && !connector.stages.outbound);
    }
    assert_eq!(server.request_count(), 1);
    server.assert_drained();
}
