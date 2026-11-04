use super::*;

#[test]
fn digest_matches_all_pinned_native_vectors() {
    let vectors: Vec<Value> =
        serde_json::from_str(include_str!("../codex/open_completion_vectors.json")).unwrap();
    assert_eq!(vectors.len(), 11);
    for vector in vectors {
        let input = &vector["input"];
        let servers = input["mcpServers"].as_array().cloned().unwrap_or_default();
        let snapshot = json!({"cwd":input["cwd"], "mcpServers":servers});
        assert_eq!(canonical_json(&snapshot).unwrap(), vector["canonical"]);
        assert_eq!(
            binding_digest(input["cwd"].as_str().unwrap(), &servers).unwrap(),
            vector["bindingDigest"],
            "{}",
            vector["name"]
        );
    }
}

#[test]
fn unsupported_numeric_and_oversize_snapshots_are_refused() {
    assert!(binding_digest("relative", &[]).is_err());
    assert!(binding_digest("/home", &[json!({"value": 1.0})]).is_err());
    assert!(binding_digest("/home", &[json!("x".repeat(65536))]).is_err());
    assert!(binding_digest("/home", &vec![json!({}); 33]).is_err());
    assert!(!uuid4("00000000-0000-0000-0000-000000000000"));
}
