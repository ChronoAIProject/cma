use cmaeg_core::profile::EventWorkerProfile;
use serde_json::{json, Value};

fn profile() -> Value {
    let channel: Value = serde_json::from_str(include_str!("fixtures/channel.json")).unwrap();
    let mut value = channel["profile"].clone();
    value["context"]["skills"] = json!([{"source":"ornn","id":"skill","version":"1"}]);
    value["context"]["mcp_servers"] =
        json!([{"url":"https://tools.example/mcp","auth":{"type":"nyxid_service","slug":"tools"}}]);
    value
}

fn validate(value: Value) -> Result<(), cmaeg_core::ValidationError> {
    serde_json::from_value::<EventWorkerProfile>(value)
        .unwrap()
        .validate()
}

#[test]
fn profile_string_bounds_are_bytes_and_report_the_offending_field() {
    for (pointer, min, max) in [
        ("/metadata/name", 1, 80),
        ("/metadata/description", 0, 2_000),
        ("/metadata/owner/subject", 1, 512),
        ("/model/service", 1, 512),
        ("/model/model", 1, 512),
        ("/permission/service_scope/0", 1, 512),
        ("/context/skills/0/id", 1, 512),
        ("/context/skills/0/version", 1, 128),
        ("/context/system_prompt", 0, 32 * 1_024),
    ] {
        for size in [min, max] {
            let mut value = profile();
            *value.pointer_mut(pointer).unwrap() = json!("x".repeat(size));
            validate(value).unwrap_or_else(|error| panic!("{pointer}: {error}"));
        }
        let mut value = profile();
        *value.pointer_mut(pointer).unwrap() = json!("é".repeat(max / 2 + 1));
        assert_eq!(validate(value).unwrap_err().pointer, pointer);
        if min > 0 {
            let mut value = profile();
            *value.pointer_mut(pointer).unwrap() = json!("");
            assert_eq!(validate(value).unwrap_err().pointer, pointer);
        }
    }
}

#[test]
fn rejects_profile_collection_overflow_at_each_public_limit() {
    for (pointer, limit) in [
        ("/permission/service_scope", 64),
        ("/context/skills", 32),
        ("/context/mcp_servers", 8),
    ] {
        let mut value = profile();
        let item = value.pointer(pointer).unwrap()[0].clone();
        *value.pointer_mut(pointer).unwrap() = json!(vec![item.clone(); limit]);
        validate(value.clone()).unwrap();
        value
            .pointer_mut(pointer)
            .unwrap()
            .as_array_mut()
            .unwrap()
            .push(item);
        assert_eq!(validate(value).unwrap_err().pointer, pointer);
    }
}

#[test]
fn rejects_unsupported_profile_schema() {
    let mut value = profile();
    value["schema_version"] = json!(2);
    assert_eq!(validate(value).unwrap_err().pointer, "/schema_version");
}

#[test]
fn environment_limit_counts_json_encoding_overhead() {
    let mut value = profile();
    value["environment"] = json!("x".repeat(64 * 1_024 - 2));
    validate(value.clone()).unwrap();
    value["environment"] = json!("x".repeat(64 * 1_024 - 1));
    assert_eq!(validate(value).unwrap_err().pointer, "/environment");
}

#[test]
fn rejects_non_https_or_overlong_profile_mcp_urls() {
    for url in [
        String::new(),
        "http://tools.example".into(),
        "https://".to_owned() + &"x".repeat(4_096),
    ] {
        let mut value = profile();
        value["context"]["mcp_servers"][0]["url"] = json!(url);
        assert_eq!(
            validate(value).unwrap_err().pointer,
            "/context/mcp_servers/0/url"
        );
    }
    let mut value = profile();
    let prefix = "https://tools.example/";
    value["context"]["mcp_servers"][0]["url"] =
        json!(prefix.to_owned() + &"x".repeat(4_096 - prefix.len()));
    validate(value).unwrap();
}

#[test]
fn rejects_unknown_fields_in_nested_profile_contracts() {
    for pointer in [
        "",
        "/metadata",
        "/metadata/owner",
        "/model",
        "/permission",
        "/context",
        "/context/skills/0",
        "/context/mcp_servers/0",
        "/context/mcp_servers/0/auth",
    ] {
        let mut value = profile();
        value
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unexpected".into(), json!(true));
        assert!(
            serde_json::from_value::<EventWorkerProfile>(value).is_err(),
            "{pointer}"
        );
    }
}

#[test]
fn digest_pins_semantics_across_json_key_order_and_changes_with_profile_content() {
    let a: EventWorkerProfile = serde_json::from_value(profile()).unwrap();
    let mut b = a.clone();
    b.environment = Some(serde_json::from_str(r#"{"b":1,"a":[2,3]}"#).unwrap());
    let mut c = a.clone();
    c.environment = Some(serde_json::from_str(r#"{"a":[2,3],"b":1}"#).unwrap());
    assert_eq!(b.digest(), c.digest());
    assert_ne!(a.digest(), b.digest());
    c.environment = Some(json!({"a":[3,2],"b":1}));
    assert_ne!(b.digest(), c.digest());
    assert_eq!(a.digest().len(), 71);
}
