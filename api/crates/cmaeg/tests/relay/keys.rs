use super::{signer::Signer, support::*};
use cmaeg::IngressConfig;
use serde_json::json;
use std::{sync::atomic::Ordering, time::Duration};

#[tokio::test]
async fn concurrent_cold_callbacks_share_one_jwks_and_one_restore() {
    let fixture = Fixture::new(|_| {}, Default::default()).await;
    fixture.backend.delay_ms.store(30, Ordering::SeqCst);
    let envelope = Envelope::new();
    let responses = futures_util::future::join_all((0..20).map(|_| fixture.send(&envelope))).await;
    for response in responses {
        assert_eq!(response.status(), 202);
    }
    assert_eq!(fixture.backend.gets.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.backend.records.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn jwks_bounds_key_constraints_and_redirects_fail_closed() {
    for (case, reason) in [
        ("oversize", "jwks_bounds"),
        ("too_many", "jwks_bounds"),
        ("empty", "jwks_bounds"),
        ("algorithm", "jwks_invalid"),
        ("type", "jwks_invalid"),
        ("purpose", "jwks_invalid"),
        ("duplicate", "jwks_invalid"),
        ("modulus", "jwks_invalid"),
        ("redirect", "jwks_unavailable"),
    ] {
        let fixture = Fixture::new(|_| {}, Default::default()).await;
        {
            let mut jwks = fixture.backend.jwks.lock().unwrap();
            match case {
                "oversize" => jwks["padding"] = "x".repeat(32768).into(),
                "too_many" => jwks["keys"] = vec![jwks["keys"][0].clone(); 17].into(),
                "empty" => jwks["keys"] = json!([]),
                "algorithm" => jwks["keys"][0]["alg"] = "RS512".into(),
                "type" => jwks["keys"][0]["kty"] = "EC".into(),
                "purpose" => jwks["keys"][0]["use"] = "enc".into(),
                "duplicate" => jwks["keys"] = vec![jwks["keys"][0].clone(); 2].into(),
                "modulus" => jwks["keys"][0]["n"] = "AA".into(),
                "redirect" => fixture.backend.status.store(302, Ordering::SeqCst),
                _ => unreachable!(),
            }
        }
        problem(fixture.send(&Envelope::new()).await, 401, reason).await;
        problem(
            fixture.send(&Envelope::new()).await,
            401,
            "jwks_unavailable",
        )
        .await;
        assert_eq!(fixture.backend.gets.load(Ordering::SeqCst), 1);
        assert!(fixture.recorder.requests.lock().unwrap().is_empty());
    }
    let fixture = Fixture::new(|_| {}, Default::default()).await;
    fixture.backend.jwks.lock().unwrap()["keys"][0]
        .as_object_mut()
        .unwrap()
        .remove("alg");
    assert_eq!(fixture.send(&Envelope::new()).await.status(), 202);
}
#[tokio::test]
async fn expires_cache_and_throttles_unknown_kid_refresh() {
    let fixture = Fixture::new(
        |_| {},
        IngressConfig {
            jwks_ttl: Duration::from_millis(20),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(fixture.send(&Envelope::new()).await.status(), 202);
    tokio::time::sleep(Duration::from_millis(25)).await;
    problem(
        fixture.send(&Envelope::new()).await,
        401,
        "jwks_unavailable",
    )
    .await;
    assert_eq!(fixture.backend.gets.load(Ordering::SeqCst), 1);
    tokio::time::sleep(Duration::from_millis(5000)).await;
    assert_eq!(fixture.send(&Envelope::new()).await.status(), 202);
    assert_eq!(fixture.backend.gets.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn unknown_kid_rotation_refreshes_once_per_five_seconds() {
    let fixture = Fixture::new(|_| {}, Default::default()).await;
    assert_eq!(fixture.send(&Envelope::new()).await.status(), 202);
    let envelope = Envelope::new();
    let (body, _) = envelope.signed();
    let mut claims = envelope.callback.clone();
    use sha2::{Digest, Sha256};
    claims["body_sha256"] = format!("{:x}", Sha256::digest(&body)).into();
    let token =
        Signer::shared().with_header(&claims, json!({"alg":"RS256","typ":"JWT","kid":"rotated"}));
    for _ in 0..3 {
        problem(
            fixture.request(body.clone(), &token).send().await.unwrap(),
            401,
            "signing_key_unknown",
        )
        .await;
    }
    assert_eq!(fixture.backend.gets.load(Ordering::SeqCst), 1);
    let mut key = Signer::shared().jwks()["keys"][0].clone();
    key["kid"] = "rotated".into();
    fixture.backend.jwks.lock().unwrap()["keys"]
        .as_array_mut()
        .unwrap()
        .push(key);
    tokio::time::sleep(Duration::from_millis(5050)).await;
    assert_eq!(
        fixture.request(body, &token).send().await.unwrap().status(),
        202
    );
    assert_eq!(fixture.backend.gets.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn jwks_fetch_has_a_total_three_second_deadline() {
    let fixture = Fixture::new(|_| {}, Default::default()).await;
    fixture.backend.delay_ms.store(5000, Ordering::SeqCst);
    let start = std::time::Instant::now();
    problem(
        fixture.send(&Envelope::new()).await,
        401,
        "jwks_unavailable",
    )
    .await;
    assert!(start.elapsed() >= Duration::from_millis(2900));
    assert!(start.elapsed() < Duration::from_millis(4000));
    assert!(fixture.recorder.requests.lock().unwrap().is_empty());
}

fn unknown_key_request(fixture: &Fixture, kid: &str) -> reqwest::RequestBuilder {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    let (body, _) = Envelope::new().signed();
    let token = format!(
        "{}.{}.{}",
        URL_SAFE_NO_PAD.encode(json!({"alg":"RS256","typ":"JWT","kid":kid}).to_string()),
        URL_SAFE_NO_PAD.encode("{}"),
        URL_SAFE_NO_PAD.encode([0_u8; 256]),
    );
    fixture.request(body, &token)
}

#[tokio::test]
async fn cached_callback_completes_while_forged_unknown_key_refresh_is_slow() {
    let fixture = Fixture::new(|_| {}, Default::default()).await;
    assert_eq!(fixture.send(&Envelope::new()).await.status(), 202);
    tokio::time::sleep(Duration::from_millis(5050)).await;
    fixture.backend.delay_ms.store(2500, Ordering::SeqCst);
    let request = unknown_key_request(&fixture, "forged-unknown");
    let attack = tokio::spawn(async move { request.send().await.unwrap() });
    tokio::time::timeout(Duration::from_secs(1), async {
        while fixture.backend.gets.load(Ordering::SeqCst) != 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("unknown key starts a JWKS refresh");
    let response = tokio::time::timeout(
        Duration::from_millis(1500),
        fixture.send(&Envelope::new()),
    )
    .await
    .expect("cached callback must finish within 1.5 seconds, before the 2.5-second JWKS response");
    assert_eq!(response.status(), 202);
    assert!(!attack.is_finished());
    problem(attack.await.unwrap(), 401, "signing_key_unknown").await;
    assert_eq!(fixture.backend.gets.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn failed_unknown_key_refresh_keeps_valid_callbacks_available() {
    let fixture = Fixture::new(|_| {}, Default::default()).await;
    assert_eq!(fixture.send(&Envelope::new()).await.status(), 202);
    tokio::time::sleep(Duration::from_millis(5050)).await;
    fixture.backend.status.store(503, Ordering::SeqCst);
    problem(
        unknown_key_request(&fixture, "forged-unknown")
            .send()
            .await
            .unwrap(),
        401,
        "jwks_unavailable",
    )
    .await;
    assert_eq!(fixture.send(&Envelope::new()).await.status(), 202);
    assert_eq!(fixture.backend.gets.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn concurrent_unknown_key_callbacks_share_exactly_one_refresh() {
    let fixture = Fixture::new(|_| {}, Default::default()).await;
    assert_eq!(fixture.send(&Envelope::new()).await.status(), 202);
    tokio::time::sleep(Duration::from_millis(5050)).await;
    fixture.backend.delay_ms.store(100, Ordering::SeqCst);
    let before = fixture.backend.gets.load(Ordering::SeqCst);
    let responses = futures_util::future::join_all(
        (0..20).map(|index| unknown_key_request(&fixture, &format!("forged-{index}")).send()),
    )
    .await;
    for response in responses {
        problem(response.unwrap(), 401, "signing_key_unknown").await;
    }
    assert_eq!(fixture.backend.gets.load(Ordering::SeqCst), before + 1);
}
