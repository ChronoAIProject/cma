use super::AgentProvider;

#[test]
fn provider_selection_preserves_its_closed_wire_contract() {
    let provider = AgentProvider::default();
    assert_eq!(provider, AgentProvider::Codex);
    assert_eq!(serde_json::to_string(&provider).unwrap(), "\"codex\"");
    assert_eq!(
        serde_json::from_str::<AgentProvider>("\"codex\"").unwrap(),
        provider
    );
    for invalid in ["\"unknown\"", "\"Codex\"", "null", "{}"] {
        assert!(serde_json::from_str::<AgentProvider>(invalid).is_err());
    }
}

#[test]
fn provider_policy_protects_the_agent_home_environment_key() {
    assert_eq!(
        AgentProvider::Codex.reserved_environment_names(),
        &["CODEX_HOME"]
    );
}
