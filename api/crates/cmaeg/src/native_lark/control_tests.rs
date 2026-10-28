use super::{
    callback::Callback,
    test_support as native,
    webhook::{self, Verified},
};
use serde_json::{json, Value};
fn raw(now: i64) -> Value {
    let mut value = native::envelope(
        "card.action.trigger",
        "callback_one",
        json!({"host":"im_message","token":"fixture-card-update-credential",
        "operator":{"tenant_key":native::identity().tenant_key,"open_id":"ou_human"},"context":{"open_chat_id":"oc_native_chat","open_message_id":"om_confirmed"},
        "action":{"tag":"button","value":{"cma_action":"cnl1:30d4d6fe46f64e2db382123456789abc:1:0"}}}),
    );
    value["header"]["create_time"] = (now * 1_000_000).to_string().into();
    value
}
#[test]
fn signed_encrypted_v2_controls_strip_both_credentials_and_reject_tampering() {
    let now = 1793145600;
    let (body, signature) = native::signed(&raw(now), now);
    let Verified::Event(clean) = webhook::verify(
        &native::identity(),
        &native::credentials(),
        &now.to_string(),
        "fixture_nonce",
        &signature,
        &body,
        now,
    )
    .unwrap() else {
        panic!()
    };
    let callback = Callback::extract(&clean, &native::identity(), now).unwrap();
    let retained = serde_json::to_string(&callback).unwrap();
    let credentials = native::credentials();
    for credential in [
        credentials.verification_token.as_str(),
        "fixture-card-update-credential",
    ] {
        assert!(!clean.to_string().contains(credential));
        assert!(!retained.contains(credential));
    }
    let mut modified = body.clone();
    modified.push(b' ');
    assert!(webhook::verify(
        &native::identity(),
        &native::credentials(),
        &now.to_string(),
        "fixture_nonce",
        &signature,
        &modified,
        now
    )
    .is_err());
    for field in ["encrypt", "verification"] {
        let mut credentials = native::credentials();
        if field == "encrypt" {
            credentials.encrypt_key =
                cmaeg_core::channel::Secret::new("fixture-rotated-encrypt-key");
        } else {
            credentials.verification_token =
                cmaeg_core::channel::Secret::new("fixture-rotated-verification");
        }
        assert!(webhook::verify(
            &native::identity(),
            &credentials,
            &now.to_string(),
            "fixture_nonce",
            &signature,
            &body,
            now
        )
        .is_err());
    }
    assert!(webhook::verify(
        &native::identity(),
        &native::credentials(),
        &now.to_string(),
        "fixture_nonce",
        &signature,
        &body,
        now + 301
    )
    .is_err());
}
#[test]
fn v2_callback_refuses_foreign_legacy_preview_and_nonissued_action_shapes() {
    let now = 1793145600;
    for case in 0..16 {
        let mut v = raw(now);
        match case {
            0 => v["schema"] = "1.0".into(),
            1 => v["header"]["event_type"] = "card.action.trigger_v1".into(),
            2 => v["header"]["app_id"] = "cli_foreign".into(),
            3 => v["header"]["tenant_key"] = "foreign".into(),
            4 => v["event"]["operator"]["tenant_key"] = "foreign".into(),
            5 => v["event"]["operator"]["open_id"] = native::identity().bot_open_id.into(),
            6 => v["event"]["host"] = "url_preview".into(),
            7 => v["event"]["context"]["url"] = "https://example.test".into(),
            8 => v["event"]["action"]["tag"] = "select_static".into(),
            9 => v["event"]["action"]["option"] = "0".into(),
            10 => v["event"]["action"]["value"]["provider_action"] = "forged".into(),
            11 => v["header"]["create_time"] = (now * 1000).to_string().into(),
            12 => {
                v["event"]["action"]["value"]["cma_action"] =
                    "cl1:30d4d6fe46f64e2db382123456789abc:1:0".into()
            }
            13 => v["event"]["action"]["form_value"] = json!({"f0":"forged"}),
            14 => v["event"]["delivery_type"] = "url_preview".into(),
            _ => v["event"]["context"]["open_message_id"] = "foreign".into(),
        }
        assert!(
            Callback::extract(&v, &native::identity(), now).is_err(),
            "case {case}"
        );
    }
}
#[test]
fn form_callback_uses_official_button_submit_and_bounded_values() {
    let now = 1793145600;
    let mut v = raw(now);
    v["event"]["action"]["value"]["cma_action"] =
        "cnlf1:30d4d6fe46f64e2db382123456789abc:1:0".into();
    v["event"]["action"]["name"] = "submit".into();
    v["event"]["action"]["form_value"] = json!({"f0":"ok"});
    assert!(Callback::extract(&v, &native::identity(), now)
        .unwrap()
        .form
        .is_some());
    v["event"]["action"]["form_value"] = json!({"f0":"x".repeat(16 * 1024)});
    assert!(Callback::extract(&v, &native::identity(), now).is_err());
}
#[test]
fn encrypted_url_challenge_has_no_storage_dependency_and_stays_under_one_second() {
    let now = 1793145600;
    let value = json!({"type":"url_verification","token":native::credentials().verification_token.as_str(),"challenge":"challenge_fixture"});
    let (body, signature) = native::signed(&value, now);
    let start = std::time::Instant::now();
    assert!(
        matches!(webhook::verify(&native::identity(), &native::credentials(), &now.to_string(), "fixture_nonce", &signature, &body, now), Ok(Verified::Challenge(c)) if c == "challenge_fixture")
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
}
