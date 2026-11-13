use super::*;
#[path = "source/transcript.rs"]
mod transcript;

#[tokio::test]
async fn provenance_inventory_preserves_empty_continuations_and_typed_scope_filters() {
    let server = Server::new().await;
    let mut options = AgentProvenanceInventoryOptions {
        shared: false,
        scope: AccessibleAgentListOptions {
            organization: Some("org".into()),
            filters: AgentListOptions {
                status: Some(AgentStatus::Deleted),
                ..Default::default()
            },
        },
        creation_origin: Some(AgentCreationCategory::ChannelBridge),
        ..Default::default()
    };
    server.expect(
        "GET",
        "/agents/provenance?status=deleted&organization=org&creation_origin=channel_bridge",
        None,
        200,
        Some(json!({"items":[],"next":"agt_scanned"})),
    );
    let first = server
        .client
        .agents()
        .provenance_inventory(&options)
        .await
        .unwrap();
    assert!(first.items.is_empty());
    options.scope.filters.page.cursor = first.next;
    server.expect("GET", "/agents/provenance?cursor=agt_scanned&status=deleted&organization=org&creation_origin=channel_bridge", None, 200, Some(json!({"items":[{"agent_id":"agt_later","status":"deleted","created_at":"2026-09-05T00:00:00Z","updated_at":"2026-09-05T00:00:00Z","creation_origin":"channel_bridge"}]})));
    let page = server
        .client
        .agents()
        .provenance_inventory(&options)
        .await
        .unwrap();
    assert_eq!(
        page.items[0].creation_origin,
        AgentCreationCategory::ChannelBridge
    );
    assert!(page.next.is_none());
    server.assert_drained();
}

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

#[tokio::test]
async fn provenance_rejects_mismatched_identity_and_unknown_source_or_actor() {
    let server = Server::new().await;
    let path = "/agents/agent-1/provenance";
    for kind in ["manual", "trigger", "channel_bridge", "legacy_unknown"] {
        server.expect("GET", path, None, 200, Some(json!({"agent_id":"agent-1","created_at":"2026-09-05T00:00:00Z","creation_origin":kind})));
        assert!(
            server
                .client
                .agents()
                .provenance("agent-1")
                .await
                .unwrap()
                .details
                .is_none()
        );
    }
    let actor = json!({"kind":"api","subject":"actual-creator","delegation_id":null});
    let detailed = json!({"agent_id":"agent-1","created_at":"2026-09-05T00:00:00Z","creation_origin":"trigger","details":{"created_by":actor,"trigger":{"trigger_id":"original-trigger","launch_id":"original-launch","launching_actor":actor},"channel_bridge":null,"workspace_profile_index":null}});
    server.expect("GET", path, None, 200, Some(detailed.clone()));
    assert!(
        server
            .client
            .agents()
            .provenance("agent-1")
            .await
            .unwrap()
            .details
            .unwrap()
            .trigger
            .is_some()
    );
    for (pointer, replacement) in [
        ("/agent_id", json!("another-agent")),
        ("/creation_origin", json!("future")),
        ("/details/created_by", json!({"kind":"future"})),
        ("/details/trigger", json!({"trigger_id":"missing-launch"})),
    ] {
        let mut malformed = detailed.clone();
        *malformed.pointer_mut(pointer).unwrap() = replacement;
        server.expect("GET", path, None, 200, Some(malformed));
        assert!(matches!(
            server.client.agents().provenance("agent-1").await,
            Err(Error::Protocol { .. })
        ));
    }
    server.assert_drained();
}

#[tokio::test]
async fn provenance_inventory_shared_scope_keeps_filtered_continuation() {
    let server = Server::new().await;
    let mut options = AgentProvenanceInventoryOptions {
        shared: true,
        creation_origin: Some(AgentCreationCategory::Trigger),
        ..Default::default()
    };
    server.expect(
        "GET",
        "/agents/provenance?shared=true&creation_origin=trigger",
        None,
        200,
        Some(json!({"items":[],"next":"agt_scan"})),
    );
    let first = server
        .client
        .agents()
        .provenance_inventory(&options)
        .await
        .unwrap();
    options.scope.filters.page.cursor = first.next;
    server.expect(
        "GET",
        "/agents/provenance?cursor=agt_scan&shared=true&creation_origin=trigger",
        None,
        200,
        Some(json!({"items":[]})),
    );
    assert!(
        server
            .client
            .agents()
            .provenance_inventory(&options)
            .await
            .unwrap()
            .next
            .is_none()
    );
    server.assert_drained();
}
