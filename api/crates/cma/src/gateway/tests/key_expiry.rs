use super::*;

#[test]
fn expiry_accepts_only_the_requested_instant_in_provider_metadata() {
    let requested = "2026-10-23T01:35:42Z";
    for observed in [
        requested,
        "2026-10-23T01:35:42+00:00",
        "2026-10-23T01:35:42.000+00:00",
        "2026-10-23T01:35:42.000000000Z",
        "2026-10-23T09:35:42+08:00",
        "2026-10-22T20:35:42-05:00",
    ] {
        assert!(
            keys::matches_expiry(Some(observed), requested),
            "{observed}"
        );
    }
    for observed in [
        None,
        Some(""),
        Some("not-a-timestamp"),
        Some("2026-10-23T01:35:42"),
        Some("2026-10-23T01:35:42+01:00"),
        Some("2026-10-23T01:35:41Z"),
        Some("2026-10-23T01:35:43Z"),
        Some("2026-10-23T01:35:42.001+00:00"),
        Some("2026-10-23T01:35:42.000000001Z"),
        Some("2026-10-23T01:35:42.0000000001Z"),
        Some("2026-10-23T01:35:42Ztrailing"),
        Some("2026-13-23T01:35:42Z"),
    ] {
        assert!(!keys::matches_expiry(observed, requested), "{observed:?}");
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn mint_accepts_provider_fractional_zero_expiry_and_returns_canonical_time() {
    let server = Server::start().await;
    let gateway = Gateway::start(&server).await;
    let external = lifecycle::external(&server, &gateway);
    let bot = bot().await;
    server.identity.lock().unwrap().expiry_suffix = Some(".000+00:00");
    for role in [BotKeyRole::Agent, BotKeyRole::Route] {
        let key = external.mint(&bot, role).await.unwrap();
        assert!(layer2_core::seconds_from_rfc3339(&key.expires_at).is_some());
        external.revoke(&bot, &key.id).await.unwrap();
    }
    assert!(server.identity.lock().unwrap().keys.is_empty());
}
