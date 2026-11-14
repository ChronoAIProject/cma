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
    assert_eq!(claims["jti"], body["correlation_id"]);
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

#[tokio::test]
async fn generated_callback_passes_production_verification_and_rejects_unbound_correlation() {
    use axum::{routing::get, Json, Router};
    use cmaeg::{
        channels::resolver::ResolvedChannel,
        ingress::relay::{
            jwks::JwksCache,
            verify::{verify, CallbackHeaders},
        },
    };
    use cmaeg_core::seal::ChannelPointer;
    use std::time::{Duration, Instant};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/jwks", get(|| async { Json(Signer::shared().jwks()) })),
        )
        .with_graceful_shutdown(async {
            let _ = stopped.await;
        })
        .await
        .unwrap();
    });
    let definition =
        super::bootstrap::channel(&origin, &super::bootstrap::random_secret().unwrap()).unwrap();
    let channel = ResolvedChannel {
        id: ChannelId::new("cmaeg1.ch.synthetic-channel").unwrap(),
        pointer: ChannelPointer {
            v: 1,
            record_id: definition.record_id.clone(),
            record_url: format!("{origin}/records"),
        },
        definition,
        agent_key_hash: [0; 32],
        consumer_keys: vec![],
        loaded_at: Instant::now(),
        version: 1,
    };
    let identity = EventIdentity {
        owner: channel.definition.owner.subject.clone(),
        channel_id: channel.id.clone(),
        record_id: channel.definition.record_id.clone(),
        event_id: EventId::new(uuid::Uuid::new_v4().to_string()).unwrap(),
    };
    let signed = Event::new(identity.clone()).signed();
    let keys = JwksCache::new(Duration::from_secs(60)).unwrap();
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let headers = |token| CallbackHeaders {
        callback_token: token,
        user_token: None,
        message_id: identity.event_id.as_str(),
        platform: "telegram",
        timestamp: &signed.timestamp,
    };
    verify(
        &channel,
        0,
        &keys,
        headers(&signed.callback),
        &signed.body,
        now,
    )
    .await
    .expect("fixture must pass the production callback and reply verifier");
    let encoded = signed.callback.split('.').nth(1).unwrap();
    let mut claims: Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(encoded).unwrap()).unwrap();
    claims["jti"] = uuid::Uuid::new_v4().to_string().into();
    let wrong = Signer::shared().token(&claims);
    let error = verify(&channel, 0, &keys, headers(&wrong), &signed.body, now)
        .await
        .err()
        .unwrap();
    assert_eq!(error.0, "payload_binding_mismatch");
    stop.send(()).unwrap();
    server.await.unwrap();
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
