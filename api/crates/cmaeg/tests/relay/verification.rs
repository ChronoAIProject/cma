use super::{signer::Signer, support::*};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[tokio::test]
async fn rejects_each_callback_claim_binding_and_time_boundary() {
    let fixture = Fixture::new(|_| {}, Default::default()).await;
    let cases: Vec<(&str, Value, &str)> = vec![
        ("iss", json!("https://other.example"), "token_issuer"),
        ("aud", json!("channel-relay/reply"), "token_audience"),
        ("token_type", json!("access"), "token_type"),
        ("jti", json!(""), "token_id"),
        ("jti", json!("other"), "payload_binding_mismatch"),
        ("api_key_id", json!("other"), "callback_key_mismatch"),
        ("message_id", json!("other"), "message_mismatch"),
        ("platform", json!("other"), "platform_mismatch"),
        ("exp", json!(now() + 400), "token_time"),
        ("exp", json!(now() - 61), "token_time"),
        ("iat", json!(-1), "token_time"),
        ("iat", json!(now() + 90), "token_time"),
        ("nbf", json!(now() + 90), "token_time"),
        ("nbf", json!(-1), "token_time"),
    ];
    for (field, value, reason) in cases {
        let mut envelope = Envelope::new();
        envelope.callback[field] = value;
        problem(fixture.send(&envelope).await, 401, reason).await;
    }
    assert!(fixture.recorder.requests.lock().unwrap().is_empty());
}
#[tokio::test]
async fn exact_body_signature_algorithm_and_header_constraints() {
    let fixture = Fixture::new(|_| {}, Default::default()).await;
    let envelope = Envelope::new();
    let (mut body, token) = envelope.signed();
    body.push(b' ');
    problem(
        fixture.request(body, &token).send().await.unwrap(),
        401,
        "body_digest_mismatch",
    )
    .await;
    let (body, _) = envelope.signed();
    let mut claims = envelope.callback;
    claims["body_sha256"] = format!("{:x}", Sha256::digest(&body)).into();
    for (header, reason) in [
        (
            json!({"alg":"HS256","typ":"JWT","kid":"test-rsa"}),
            "token_algorithm",
        ),
        (
            json!({"alg":"none","typ":"JWT","kid":"test-rsa"}),
            "token_algorithm",
        ),
        (
            json!({"alg":"RS256","typ":"JWT","kid":"test-rsa","jku":"https://attacker.example/keys"}),
            "token_header",
        ),
        (
            json!({"alg":"RS256","typ":"JWT","kid":"test-rsa","x5u":"https://attacker.example/keys"}),
            "token_header",
        ),
        (
            json!({"alg":"RS256","typ":"at+jwt","kid":"test-rsa"}),
            "token_header",
        ),
        (
            json!({"alg":"RS256","typ":"JWT","kid":"unknown"}),
            "signing_key_unknown",
        ),
    ] {
        let token = Signer::shared().with_header(&claims, header);
        problem(
            fixture.request(body.clone(), &token).send().await.unwrap(),
            401,
            reason,
        )
        .await;
    }
    let token = Signer::shared().token(&claims);
    let mut parts: Vec<_> = token.split('.').map(str::to_owned).collect();
    let replacement = if parts[2].starts_with('A') { "B" } else { "A" };
    parts[2].replace_range(0..1, replacement);
    problem(
        fixture
            .request(body, &parts.join("."))
            .send()
            .await
            .unwrap(),
        401,
        "token_signature",
    )
    .await;
}
#[tokio::test]
async fn rejects_each_payload_and_reply_binding() {
    let fixture = Fixture::new(|_| {}, Default::default()).await;
    for (pointer, value, reason) in [
        ("/unexpected", json!(true), "payload_invalid"),
        ("/agent/unexpected", json!(true), "payload_invalid"),
        ("/message_id", json!("other"), "payload_binding_mismatch"),
        (
            "/correlation_id",
            json!("other"),
            "payload_binding_mismatch",
        ),
        (
            "/agent/api_key_id",
            json!("other"),
            "payload_binding_mismatch",
        ),
        ("/platform", json!("other"), "payload_binding_mismatch"),
        ("/conversation/id", json!("other"), "route_denied"),
        ("/sender/platform_id", json!(""), "payload_bounds"),
        ("/timestamp", json!("yesterday"), "payload_bounds"),
        ("/reply_token", Value::Null, "reply_token_missing"),
        ("/reply_token", json!("not-a-token"), "token_format"),
    ] {
        let mut envelope = Envelope::new();
        let (parent, key) = pointer.rsplit_once('/').unwrap();
        envelope.payload.pointer_mut(parent).unwrap()[key] = value;
        problem(fixture.send(&envelope).await, 401, reason).await;
    }
    for (field, value, reason) in [
        ("api_key_id", json!("other"), "reply_binding_mismatch"),
        ("conversation_id", json!("other"), "reply_binding_mismatch"),
        (
            "inbound_message_id",
            json!("other"),
            "reply_binding_mismatch",
        ),
        ("platform", json!("other"), "reply_binding_mismatch"),
        ("aud", json!("channel-relay/callback"), "token_audience"),
        ("token_type", json!("relay_callback"), "token_type"),
        ("iss", json!("https://other.example"), "token_issuer"),
        ("exp", json!(now() + 1801), "token_time"),
    ] {
        let mut envelope = Envelope::new();
        envelope.reply[field] = value;
        problem(fixture.send(&envelope).await, 401, reason).await;
    }
    let mut expired = Envelope::new();
    expired.reply["iat"] = (now() - 100).into();
    expired.reply["exp"] = (now() - 1).into();
    problem(fixture.send(&expired).await, 401, "reply_expired").await;
    assert!(fixture.recorder.requests.lock().unwrap().is_empty());
}
#[tokio::test]
async fn optional_relay_user_only_corroborates_and_cannot_add_authority() {
    let fixture = Fixture::new(|_| {}, Default::default()).await;
    let mut relay = claims(ISSUER, "access", 300);
    relay.as_object_mut().unwrap().extend(json!({"sub":"user-1","relay":true,"relay_api_key_id":"key-1","relay_allowed_service_ids":[],"relay_allowed_node_ids":[],"relay_allow_all_services":false,"relay_allow_all_nodes":false}).as_object().unwrap().clone());
    let (body, token) = Envelope::new().signed();
    assert_eq!(
        fixture
            .request(body.clone(), &token)
            .header("x-nyxid-user-token", Signer::shared().token(&relay))
            .send()
            .await
            .unwrap()
            .status(),
        202
    );
    for (field, value) in [
        ("relay_api_key_id", json!("other")),
        ("sub", json!("other")),
        ("relay", json!(false)),
        ("delegated", json!(true)),
        ("sa", json!(true)),
        ("assistant_forward", json!(true)),
        ("cnf", json!({})),
        ("act", json!({})),
        ("relay_allowed_node_ids", json!([""])),
    ] {
        let mut modified = relay.clone();
        modified[field] = value;
        problem(
            fixture
                .request(body.clone(), &token)
                .header("x-nyxid-user-token", Signer::shared().token(&modified))
                .send()
                .await
                .unwrap(),
            401,
            "relay_user_mismatch",
        )
        .await;
    }
    relay
        .as_object_mut()
        .unwrap()
        .remove("relay_allowed_node_ids");
    problem(
        fixture
            .request(body, &token)
            .header("x-nyxid-user-token", Signer::shared().token(&relay))
            .send()
            .await
            .unwrap(),
        401,
        "token_claims",
    )
    .await;
}
