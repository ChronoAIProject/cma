use super::*;
use ring::{
    rand::SystemRandom,
    signature::{Ed25519KeyPair, KeyPair},
};

fn fixture() -> (Ed25519KeyPair, BootstrapGrant) {
    let encoded = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
    let key = Ed25519KeyPair::from_pkcs8(encoded.as_ref()).unwrap();
    let image = format!(
        "local/runtime@sha256:{:x}",
        Sha256::digest(b"qualified image")
    );
    let grant = BootstrapGrant {
        version: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
        operator_subject: "local-operator".into(),
        process: ProcessIdentity {
            deployment: "local-fixture".into(),
            instance_id: uuid::Uuid::new_v4().to_string(),
            config_sha256: format!("{:x}", Sha256::digest(b"exact configuration")),
            binary_image: image.clone(),
            executable_sha256: "a".repeat(64),
            agent_image: image.clone(),
            machine_image: Some(image),
        },
        issued_at_unix: 1000,
        expires_at_unix: 1100,
        scope: BootstrapScope::InertOnly,
    };
    (key, grant)
}

fn envelope(key: &Ed25519KeyPair, grant: &BootstrapGrant) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "grant": grant,
        "signature": STANDARD.encode(key.sign(&message(grant).unwrap()).as_ref()),
    }))
    .unwrap()
}

fn public(key: &Ed25519KeyPair) -> [u8; 32] {
    key.public_key().as_ref().try_into().unwrap()
}

#[test]
fn exact_signed_process_receives_only_inert_authority() {
    let (key, grant) = fixture();
    let verified = verify(&envelope(&key, &grant), &public(&key), &grant.process, 1001).unwrap();
    assert_eq!(verified.process(), &grant.process);
    assert_eq!(verified.expires_at_unix(), 1100);
    assert!(digest(verified.sha256()));
}

#[test]
fn same_configuration_after_process_restart_refuses_old_grant() {
    let (key, grant) = fixture();
    let mut restarted = grant.process.clone();
    restarted.instance_id = uuid::Uuid::new_v4().to_string();
    assert!(verify(&envelope(&key, &grant), &public(&key), &restarted, 1001).is_err());
}

#[test]
fn different_key_changed_image_and_unsigned_record_refuse() {
    let (key, grant) = fixture();
    let (other, _) = fixture();
    assert!(verify(
        &envelope(&key, &grant),
        &public(&other),
        &grant.process,
        1001
    )
    .is_err());
    let mut changed = grant.process.clone();
    changed.agent_image = changed.agent_image.replace("local/", "different/");
    assert!(verify(&envelope(&key, &grant), &public(&key), &changed, 1001).is_err());
    assert!(verify(
        &serde_json::to_vec(&grant).unwrap(),
        &public(&key),
        &grant.process,
        1001
    )
    .is_err());
}

#[test]
fn validity_is_bounded_and_never_renews_on_replay() {
    let (key, mut grant) = fixture();
    for now in [999, 1100, 1101] {
        assert!(verify(&envelope(&key, &grant), &public(&key), &grant.process, now).is_err());
    }
    grant.expires_at_unix = 1301;
    assert!(verify(&envelope(&key, &grant), &public(&key), &grant.process, 1001).is_err());
}

#[test]
fn malformed_unknown_fields_and_oversized_envelopes_refuse() {
    let (key, grant) = fixture();
    let mut value: serde_json::Value = serde_json::from_slice(&envelope(&key, &grant)).unwrap();
    value["writer_authorized"] = serde_json::Value::Bool(true);
    assert!(verify(
        &serde_json::to_vec(&value).unwrap(),
        &public(&key),
        &grant.process,
        1001
    )
    .is_err());
    assert!(verify(
        &vec![b' '; MAX_GRANT_BYTES + 1],
        &public(&key),
        &grant.process,
        1001
    )
    .is_err());
}
