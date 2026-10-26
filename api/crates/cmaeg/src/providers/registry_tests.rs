use super::*;

fn json(base_url: &str) -> String {
    format!(
        r#"[{{"slug":"cma_codex","base_url":"{base_url}","kind":"responses_http","paths":{{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{{binding_id}}","conversation":"/api/v1/cmaeg/bindings/{{binding_id}}/conversations/{{conversation_id}}","responses":"/api/v2/responses"}},"max_inflight":256}}]"#
    )
}

#[test]
fn validates_loopback_and_cluster_hosts_but_rejects_metadata_http() {
    assert_eq!(
        ProviderRegistry::parse(&json("http://127.0.0.1:8080"))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        ProviderRegistry::parse(&json("http://example-host-01.example-namespace.svc"))
            .unwrap()
            .len(),
        1
    );
    assert!(ProviderRegistry::parse(&json("http://169.254.169.254")).is_err());
}

#[test]
fn url_construction_encodes_path_segments_and_never_escapes_the_origin() {
    let registry = ProviderRegistry::parse(&json("https://provider.example/base/")).unwrap();
    let entry = registry.iter().next().unwrap();
    let url = entry
        .url("binding", &[("binding_id", "../../etc/passwd")])
        .unwrap();
    assert_eq!(
        url.origin().ascii_serialization(),
        "https://provider.example"
    );
    assert!(url.as_str().contains("%2f"));
    assert!(!url.as_str().contains(".."));
}

#[test]
fn rejects_duplicate_slugs_and_missing_placeholders() {
    let duplicate = json("https://one.example").replacen(
        "]",
        &format!(",{}]", &json("https://two.example")[1..]),
        1,
    );
    assert!(ProviderRegistry::parse(&duplicate).is_err());
    let missing = json("https://one.example").replace("{binding_id}", "binding");
    assert!(ProviderRegistry::parse(&missing).is_err());
}

#[test]
fn rejects_private_https_targets_and_unsafe_templates() {
    assert!(ProviderRegistry::parse(&json("https://169.254.169.254")).is_err());
    let unknown_placeholder =
        json("https://provider.example").replace("/api/v2/responses", "/api/{tenant}/responses");
    assert!(ProviderRegistry::parse(&unknown_placeholder).is_err());
    let traversal =
        json("https://provider.example").replace("/api/v2/responses", "/api/../responses");
    assert!(ProviderRegistry::parse(&traversal).is_err());
}
#[test]
fn path_templates_cannot_replace_the_allowlisted_origin() {
    for path in ["//attacker.example/x", "/api/}/x", "/../x"] {
        let value = json("https://provider.example").replace("/api/v2/responses", path);
        assert!(ProviderRegistry::parse(&value).is_err());
    }
    let value = json("https://provider.example/base/")
        .replace("/api/v2/responses", "/https://attacker.example/x");
    let registry = ProviderRegistry::parse(&value).unwrap();
    let entry = registry.iter().next().unwrap();
    let url = entry.url("responses", &[]).unwrap();
    assert_eq!(url.host_str(), Some("provider.example"));
    assert_eq!(url.path(), "/https://attacker.example/x");
    for value in ["", ".", ".."] {
        assert!(entry.url("binding", &[("binding_id", value)]).is_err());
    }
    assert!(ProviderRegistry::parse(&json("https://[::ffff:169.254.169.254]")).is_err());
}

fn recovery_json() -> serde_json::Value {
    let mut value: serde_json::Value =
        serde_json::from_str(&json("https://provider.example")).unwrap();
    value[0]["paths"]["recovery"] = serde_json::json!({
        "response": "/api/v2/responses/{response_id}",
        "events": "/api/v2/responses/{response_id}/events"
    });
    value
}

#[test]
fn recovery_is_explicit_and_uses_encoded_same_origin_paths() {
    let legacy = ProviderRegistry::parse(&json("https://provider.example")).unwrap();
    let entry = legacy.iter().next().unwrap();
    assert!(entry.paths.recovery.is_none());
    assert!(entry
        .url("recovery_response", &[("response_id", "r")])
        .is_err());
    let registry = ProviderRegistry::parse(&recovery_json().to_string()).unwrap();
    let entry = registry.iter().next().unwrap();
    for path in [ProviderPath::RecoveryResponse, ProviderPath::RecoveryEvents] {
        let url = entry
            .typed_url(path, &[("response_id", "../other?x=1#f")])
            .unwrap();
        assert_eq!(url.origin(), entry.base_url.origin());
        assert!(url.path().contains("%2e%2e%2fother%3fx%3d1%23f"));
        assert!(url.query().is_none() && url.fragment().is_none());
    }
}

#[test]
fn recovery_templates_refuse_unknown_fields_and_unsafe_or_missing_segments() {
    for path in [
        "//evil.example/{response_id}",
        "/../{response_id}",
        "/%2e/{response_id}",
        "/responses/{unknown}",
        "/responses",
        "/{response_id}/{response_id}",
        "/{response_id}?x=1",
    ] {
        let mut value = recovery_json();
        value[0]["paths"]["recovery"]["events"] = path.into();
        assert!(
            ProviderRegistry::parse(&value.to_string()).is_err(),
            "{path}"
        );
    }
    let mut value = recovery_json();
    value[0]["paths"]["recovery"]["retry_post"] = true.into();
    assert!(ProviderRegistry::parse(&value.to_string()).is_err());
    let mut value = recovery_json();
    value[0]["paths"]["recovery"]
        .as_object_mut()
        .unwrap()
        .remove("response");
    assert!(ProviderRegistry::parse(&value.to_string()).is_err());
}
