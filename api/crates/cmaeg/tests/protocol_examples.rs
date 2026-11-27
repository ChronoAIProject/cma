//! Exercise the published examples through the actual channel and signature validators.
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg::channels::{
    definition::{build_definition, validate_create_request, ChannelCreateRequest},
    write_gate::WriteFlags,
};
use cmaeg_core::{
    channel::{ConformanceLevel, Secret},
    ids::{BindingId, RecordId},
};
use serde_json::Value;

const PROTOCOL: &str = include_str!("fixtures/protocol-examples.md");

#[test]
fn protocol_nested_auth_rejects_unknown_request_fields() {
    for auth in [
        serde_json::json!({"type":"none","credential":"unexpected"}),
        serde_json::json!({"type":"nyxid_service","slug":"service","credential":"unexpected"}),
    ] {
        assert!(serde_json::from_value::<cmaeg_core::profile::McpAuth>(auth).is_err());
    }
    // Internally tagged unit variants ignore unknown fields in Serde, even with
    // deny_unknown_fields. Empty struct variants preserve the wire shape strictly.
    for value in ["source", "sync"] {
        let object = serde_json::json!({"type":value,"unexpected":true});
        assert!(serde_json::from_value::<cmaeg_core::channel::Sink>(object).is_err());
    }
    for value in ["text", "number", "date"] {
        let object = serde_json::json!({"type":value,"unexpected":true});
        assert!(serde_json::from_value::<cmaeg_core::message::FormFieldKind>(object).is_err());
    }
    assert!(
        serde_json::from_value::<cmaeg_core::channel::AdmissionPolicy>(
            serde_json::json!({"type":"all","unexpected":true})
        )
        .is_err()
    );
    for value in ["open", "deny"] {
        assert!(
            serde_json::from_value::<cmaeg_core::channel::IdentityAdmission>(
                serde_json::json!({"type":value,"unexpected":true})
            )
            .is_err()
        );
    }
}

#[test]
fn protocol_create_example_validates_with_runtime_credentials() {
    let raw = PROTOCOL
        .split("```json\n")
        .find(|block| block.starts_with("{\n  \"schema_version\""))
        .unwrap()
        .split("\n```")
        .next()
        .unwrap();
    let credential = format!("nyx_{}", uuid::Uuid::new_v4().simple());
    let hmac = STANDARD.encode(uuid::Uuid::new_v4().as_bytes());
    let raw = raw
        .replace("<creator-subject>", "example-owner")
        .replace("<bot-agent-key>", &credential)
        .replace("<base64-string>", &hmac)
        .replace("<model-service>", "model-service")
        .replace("<supported-model>", "example-model");
    let request: ChannelCreateRequest = serde_json::from_str(&raw).unwrap();
    validate_create_request(&request).unwrap();
    let definition = build_definition(
        &request,
        "example-owner",
        RecordId::new("record").unwrap(),
        BindingId::new("binding").unwrap(),
        ConformanceLevel::L1,
        WriteFlags::default(),
    )
    .unwrap();
    let mut expected = definition.to_public_json();
    expected["version"] = 2.into();
    let view: cmaeg_http::http::schema::PublicChannelDefinition =
        cmaeg::http::schema::PublicChannelDefinition::new(&definition, 2);
    let _: &cmaeg::http::schema::PublicSecrets = &view.secrets;
    assert_eq!(serde_json::to_value(view).unwrap(), expected);
}

#[test]
fn protocol_python_signature_authenticates_exact_body_bytes() {
    let source = PROTOCOL
        .split("```python\n")
        .nth(1)
        .unwrap()
        .split("\n```")
        .next()
        .unwrap();
    let secret = Secret::new(STANDARD.encode(uuid::Uuid::new_v4().as_bytes()));
    let output = std::process::Command::new("python3")
        .arg("-c")
        .arg(source)
        .env("BUILD_WEBHOOK_SECRET", secret.as_str())
        .output()
        .unwrap();
    assert!(output.status.success());
    let request: Value = serde_json::from_slice(&output.stdout).unwrap();
    let mut headers = axum::http::HeaderMap::new();
    for (name, value) in request["headers"].as_object().unwrap() {
        headers.insert(
            axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            value.as_str().unwrap().parse().unwrap(),
        );
    }
    let body = request["body"].as_str().unwrap().as_bytes();
    cmaeg_core::cloudevent::parse_structured(body).unwrap();
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    assert!(cmaeg::auth::webhook::verify(&headers, &secret, body, now));
    let mut changed = body.to_vec();
    changed.push(b' ');
    assert!(!cmaeg::auth::webhook::verify(
        &headers, &secret, &changed, now
    ));
    assert!(!cmaeg::auth::webhook::verify(
        &headers,
        &secret,
        body,
        now + 301
    ));
}
