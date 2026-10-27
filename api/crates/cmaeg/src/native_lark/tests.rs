use super::{message, revision, test_support::*, webhook};
use cmaeg_core::channel::{LarkRegion, Source};
use serde_json::json;
const NOW: i64 = 1_800_000_000;
#[test]
fn encrypted_auth_binds_bytes_timestamp_token_app_and_tenant_before_interpretation() {
    let original = text("om_original", "hello", NOW);
    let (body, signature) = signed(&original, NOW);
    let check = |body: &[u8], sig: &str, now| {
        webhook::verify(
            &identity(),
            &credentials(),
            &NOW.to_string(),
            "fixture_nonce",
            sig,
            body,
            now,
        )
    };
    let webhook::Verified::Event(event) = check(&body, &signature, NOW).unwrap() else {
        panic!()
    };
    assert!(event["header"].get("token").is_none());
    let mut corrupted = body.clone();
    corrupted.push(b' ');
    assert!(check(&corrupted, &signature, NOW).is_err());
    assert!(check(&body, &signature, NOW + 301).is_err());
    for sent in [i64::MIN, i64::MAX] {
        let (bytes, signature) = signed(&original, sent);
        assert!(webhook::verify(
            &identity(),
            &credentials(),
            &sent.to_string(),
            "fixture_nonce",
            &signature,
            &bytes,
            NOW,
        )
        .is_err());
    }
    for key in ["app_id", "tenant_key", "token"] {
        let mut wrong = original.clone();
        wrong["header"][key] = "other".into();
        let (b, s) = signed(&wrong, NOW);
        assert!(check(&b, &s, NOW).is_err());
    }
    let mut other = identity();
    other.region = LarkRegion::Lark;
    // Region has separate configuration and API custody; the wire has no region field.
    assert_ne!(
        message::original_id(&identity(), "om_original"),
        message::original_id(&other, "om_original")
    );
    let challenge = json!({"type":"url_verification","token":credentials().verification_token.as_str(),"challenge":"one-time-value"});
    let (b, s) = signed(&challenge, NOW);
    assert!(
        matches!(check(&b,&s,NOW).unwrap(),webhook::Verified::Challenge(v) if v=="one-time-value")
    );
    assert!(check(br#"{"challenge":"untrusted"}"#, &signature, NOW).is_err());
}
#[test]
fn receive_keys_are_message_scoped_reactions_are_deltas_and_edits_are_not_inferred() {
    let Source::NativeLark(source) = source() else {
        panic!()
    };
    let raw = text("om_original", "hello", NOW);
    let first = message::extract(&source, &raw, NOW).unwrap().unwrap();
    let mut retry = raw.clone();
    retry["header"]["event_id"] = "another_delivery".into();
    retry["event"]["message"]["update_time"] = (NOW * 1000 + 10).to_string().into();
    assert_eq!(
        message::extract(&source, &retry, NOW).unwrap().unwrap(),
        first
    );
    for added in [true, false] {
        let raw = reaction("reaction_delivery", "om_original", added, NOW);
        assert!(
            matches!(message::extract(&source,&raw,NOW).unwrap(),Some(message::Message::Reaction{added:a,..}) if a==added)
        );
    }
    let mut app = reaction("reaction_app", "om_original", true, NOW);
    app["event"]["operator_type"] = "app".into();
    assert!(message::extract(&source, &app, NOW).unwrap().is_none());
    let mut missing = reaction("reaction_anonymous", "om_original", true, NOW);
    missing["event"]["user_id"] = json!({"user_id":"not-an-open-id"});
    assert!(message::extract(&source, &missing, NOW).is_err());
    let mut unknown = raw;
    unknown["header"]["event_type"] = "im.message.updated_v1".into();
    assert!(message::extract(&source, &unknown, NOW).unwrap().is_none());
}
#[test]
fn sealed_source_authoring_gate_and_credential_revision_revoke_exact_authority() {
    let mut value = crate::channels::routing_fixture::routed();
    value["sources"] = json!([source()]);
    let c = credentials();
    value["secrets"]["native_lark"] = json!({"app_secret":c.app_secret.as_str(),"encrypt_key":c.encrypt_key.as_str(),"verification_token":c.verification_token.as_str()});
    let definition = crate::channels::routing_fixture::definition(value);
    definition.validate().unwrap();
    assert!(crate::channels::write_gate::check(None, &definition, Default::default()).is_err());
    crate::channels::write_gate::check(Some(&definition), &definition, Default::default()).unwrap();
    let Source::NativeLark(source) = source() else {
        panic!()
    };
    let mut changed = c.clone();
    changed.encrypt_key = cmaeg_core::channel::Secret::new("another-fixture-encrypt-key");
    assert_ne!(revision(&source, &c), revision(&source, &changed));
    for secret in [
        c.app_secret.as_str(),
        c.encrypt_key.as_str(),
        c.verification_token.as_str(),
    ] {
        assert!(!definition.to_public_json().to_string().contains(secret));
        assert!(definition.to_blob_json().to_string().contains(secret));
    }
}
