use super::{
    ledger::{hash, Ledger},
    protocol::{frames, Event},
    signer::Signer,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use cmaeg::ingress::routing_store::EventIdentity;
use cmaeg_core::ids::{ChannelId, EventId, RecordId};
use serde_json::{json, Value};

#[test]
fn relay_replays_preserve_signed_body_and_claim_bindings() {
    let mut event = Event::new(EventIdentity {
        owner: uuid::Uuid::new_v4().to_string(),
        channel_id: ChannelId::new("cmaeg1.ch.synthetic-channel").unwrap(),
        record_id: RecordId::new(uuid::Uuid::new_v4().to_string()).unwrap(),
        event_id: EventId::new(uuid::Uuid::new_v4().to_string()).unwrap(),
    });
    let first = event.signed();
    let second = event.signed();
    assert_eq!(first.body, second.body);
    assert_eq!(first.callback, second.callback);
    assert_eq!(first.reply, second.reply);
    let parts: Vec<_> = first.callback.split('.').collect();
    let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
    let body: Value = serde_json::from_slice(&first.body).unwrap();
    assert_eq!(claims["message_id"], body["message_id"]);
    assert_eq!(claims["body_sha256"], hash(&first.body));
    assert_eq!(claims["aud"], "channel-relay/callback");
    assert_eq!(
        claims["exp"].as_i64().unwrap() - claims["iat"].as_i64().unwrap(),
        300
    );
    let key = &Signer::shared().jwks()["keys"][0];
    let n = URL_SAFE_NO_PAD.decode(key["n"].as_str().unwrap()).unwrap();
    let e = URL_SAFE_NO_PAD.decode(key["e"].as_str().unwrap()).unwrap();
    ring::signature::RsaPublicKeyComponents { n: &n, e: &e }
        .verify(
            &ring::signature::RSA_PKCS1_2048_8192_SHA256,
            format!("{}.{}", parts[0], parts[1]).as_bytes(),
            &URL_SAFE_NO_PAD.decode(parts[2]).unwrap(),
        )
        .unwrap();
    assert_eq!(body["reply_token"], first.reply);
    assert_eq!(body["timestamp"], first.timestamp);
}

#[test]
fn provider_replay_retains_exact_metadata_cursor_order_and_terminal_text() {
    let mut ledger = Ledger::default();
    let body = json!({"conversation":"synthetic","metadata":{"join":"synthetic"}});
    let row = ledger
        .admit("key", "event", body.to_string().as_bytes(), &body)
        .unwrap();
    let initial = frames(&row, None, true);
    assert!(initial.contains("response.created"));
    assert!(!initial.contains("response.completed"));
    let replay = frames(&row, Some(0), false);
    assert!(!replay.contains("response.created"));
    assert!(replay.contains("id: 1\n"));
    assert!(replay.contains("id: 2\n"));
    assert!(replay.contains("fixture terminal text"));
    assert!(replay.contains("response.completed"));
    assert!(frames(&row, Some(2), false).is_empty());
    assert_eq!(
        super::protocol::snapshot(&row, "completed")["metadata"],
        body["metadata"]
    );
}
