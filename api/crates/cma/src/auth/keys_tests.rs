use super::*;

fn valid_response() -> MintedKeyResponse {
    MintedKeyResponse {
        id: "provider-key".into(),
        full_key: "secret-value".into(),
        scopes: KEY_SCOPES.into(),
        state_version: 7,
        allowed_service_ids: vec!["service-b".into(), "service-a".into()],
        allowed_node_ids: Vec::new(),
        allow_all_services: false,
        allow_all_nodes: false,
        allow_auto_connected_services: false,
    }
}

#[test]
fn deployment_key_names_round_trip_only_valid_resource_identifiers() {
    let name = key_name("production", "sbx_1234").unwrap();
    assert_eq!(name, "cma:production:sbx_1234");
    assert_eq!(
        credential_of("production", &name).as_deref(),
        Some("sbx_1234")
    );
    assert_eq!(credential_of("staging", &name), None);
    assert!(key_name("production", "bad:id").is_err());
}

#[test]
fn mint_body_closes_both_provider_wildcards() {
    let body = mint_request_body(
        "production",
        "resource_1",
        &["service-a".into()],
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000),
    )
    .unwrap();
    assert_eq!(body["allow_all_services"], false);
    assert_eq!(body["allow_all_nodes"], false);
    assert_eq!(body["scopes"], KEY_SCOPES);
}

#[test]
fn mint_body_preserves_bson_millisecond_precision() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../.github/scripts/work-recovery/fixtures/mint-expiry.json"
    )))
    .unwrap();
    let base = fixture["base_unix_seconds"].as_u64().unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let millis = case[0].as_u64().unwrap();
        let expires_at =
            SystemTime::UNIX_EPOCH + Duration::from_secs(base) + Duration::from_millis(millis);
        let body = mint_request_body(
            "work-recovery-fixture",
            "resource_1",
            &["fixture-service".into()],
            expires_at,
        )
        .unwrap();
        assert_eq!(body["expires_at"], case[1], "millisecond {millis}");
        let parsed =
            mongodb::bson::DateTime::parse_rfc3339_str(body["expires_at"].as_str().unwrap())
                .unwrap();
        assert_eq!(parsed.to_system_time(), expires_at);
    }
}

#[test]
fn mint_response_must_exactly_match_requested_authority() {
    let expiry = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    let minted = minted_key_from_response(
        valid_response(),
        &["service-a".into(), "service-b".into()],
        &[],
        expiry,
    )
    .unwrap();
    assert_eq!(minted.id, "provider-key");
    assert_eq!(minted.value.expose(), "secret-value");
    assert_eq!(minted.expires_at, expiry);
    assert_eq!(minted.state_version, 7);
    assert!(!minted.grant_fingerprint.is_empty());

    let mut open = valid_response();
    open.allow_all_services = true;
    assert!(minted_key_from_response(open, &["service-a".into()], &[], expiry).is_none());
}

#[test]
fn secret_diagnostics_do_not_reveal_new_key_values() {
    let minted = minted_key_from_response(
        valid_response(),
        &["service-a".into(), "service-b".into()],
        &[],
        SystemTime::UNIX_EPOCH,
    )
    .unwrap();
    assert!(!format!("{minted:?}").contains("secret-value"));
}

#[test]
fn revocation_accepts_only_provider_confirmed_success() {
    assert!(revoke_status(204).is_ok());
    assert!(matches!(
        revoke_status(404),
        Err(KeyIssuerError::Refused(404))
    ));
    assert!(matches!(
        revoke_status(401),
        Err(KeyIssuerError::Refused(401))
    ));
    assert!(matches!(
        revoke_status(503),
        Err(KeyIssuerError::Refused(503))
    ));
}

#[test]
fn provider_key_ids_cannot_escape_the_revoke_path() {
    assert!(valid_provider_key_id("key_123.example"));
    for invalid in ["", "../key", "key?admin=true", "key/value", " key"] {
        assert!(!valid_provider_key_id(invalid));
    }

    let mut response = valid_response();
    response.id = "key/another-path".into();
    assert!(minted_key_from_response(
        response,
        &["service-a".into(), "service-b".into()],
        &[],
        SystemTime::UNIX_EPOCH,
    )
    .is_none());
}
