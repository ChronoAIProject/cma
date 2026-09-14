use super::*;

#[tokio::test]
async fn shared_source_categories_are_explicit_and_identity_checked() {
    let server = Server::new().await;
    let path = "/agents/agent-1/source";
    for kind in ["manual", "workspace_profile", "trigger"] {
        let shared = json!({"access":"shared","agent_id":"agent-1","origin":{"type":kind}});
        server.expect("GET", path, None, 200, Some(shared.clone()));
        assert!(matches!(
            server
                .client
                .agents()
                .source_accessible("agent-1")
                .await
                .unwrap(),
            AccessibleAgentSource::Shared(_)
        ));
        server.expect("GET", path, None, 200, Some(shared));
        assert!(matches!(
            server.client.agents().source("agent-1").await,
            Err(Error::Protocol { .. })
        ));
    }
    let personal = json!({"agent_id":"agent-1","origin":{"type":"trigger","trigger_id":"trg_one","launch_id":"lch_one"}});
    server.expect("GET", path, None, 200, Some(personal));
    assert!(matches!(
        server
            .client
            .agents()
            .source("agent-1")
            .await
            .unwrap()
            .origin,
        AgentOrigin::Trigger { .. }
    ));
    for malformed in [
        json!({"access":"shared","agent_id":"other","origin":{"type":"manual"}}),
        json!({"access":"future","agent_id":"agent-1","origin":{"type":"manual"}}),
        json!({"access":"shared","agent_id":"agent-1","origin":{"type":"future"}}),
        json!({"agent_id":"agent-1","origin":{"type":"trigger"}}),
    ] {
        server.expect("GET", path, None, 200, Some(malformed));
        assert!(matches!(
            server.client.agents().source_accessible("agent-1").await,
            Err(Error::Protocol { .. })
        ));
    }
    server.assert_drained();
}
