use super::*;
fn association() -> Value {
    json!({"binding_id":"binding-one","binding_generation":1,"provider_conversation_id":"existing_agent","relationship":"borrowed","sandbox_id":"sandbox-1","workspace_id":"workspace-1","worktree_id":"worktree-1","conversation_state":"retired","binding_state":"ready","current_binding_generation":3,"is_current_generation":false,"association_created_at":NOW,"association_updated_at":NOW})
}
fn associations() -> Value {
    json!({"agent_id":"agent-1","owner":"owner","agent_created_at":NOW,"items":[association()]})
}
#[tokio::test]
async fn retained_borrowed_associations_page_and_preserve_generation_and_retirement() {
    let server = Server::new().await;
    let mut first = associations();
    first["next"] = "opaque_cursor".into();
    server.expect(
        "GET",
        "/agents/agent-1/provider-associations?limit=1",
        None,
        200,
        Some(first),
    );
    let page = server
        .client
        .agents()
        .provider_associations(
            "agent-1",
            &PageOptions {
                limit: Some(1),
                cursor: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        page.items[0].relationship,
        ProviderAssociationRelationship::Borrowed
    );
    assert_eq!(
        page.items[0].conversation_state,
        ProviderAssociationConversationState::Retired
    );
    assert!(!page.items[0].is_current_generation);
    server.expect(
        "GET",
        "/agents/agent-1/provider-associations?cursor=opaque_cursor&limit=1",
        None,
        200,
        Some(json!({"agent_id":"agent-1","owner":"owner","agent_created_at":NOW,"items":[]})),
    );
    let next = server
        .client
        .agents()
        .provider_associations(
            "agent-1",
            &PageOptions {
                limit: Some(1),
                cursor: page.next,
            },
        )
        .await
        .unwrap();
    assert!(next.items.is_empty() && next.next.is_none());
    server.assert_drained();
}
#[tokio::test]
async fn associations_refuse_wrong_identity_repeated_memberships_and_stalled_cursor() {
    for kind in 0..5 {
        let server = Server::new().await;
        let mut response = associations();
        match kind {
            0 => response["agent_id"] = "foreign".into(),
            1 => response["items"] = json!([association(), association()]),
            2 => response["items"][0]["is_current_generation"] = true.into(),
            3 => response["items"][0]["binding_generation"] = 4.into(),
            _ => {
                response["items"] = json!([]);
                response["next"] = "unexpected".into();
            }
        }
        server.expect(
            "GET",
            "/agents/agent-1/provider-associations",
            None,
            200,
            Some(response),
        );
        assert!(
            server
                .client
                .agents()
                .provider_associations("agent-1", &PageOptions::default())
                .await
                .is_err()
        );
        server.assert_drained();
    }
}
#[tokio::test]
async fn associations_reject_invalid_request_bounds_without_network() {
    let server = Server::new().await;
    for options in [
        PageOptions {
            limit: Some(0),
            cursor: None,
        },
        PageOptions {
            limit: Some(201),
            cursor: None,
        },
        PageOptions {
            limit: None,
            cursor: Some("a/b".into()),
        },
        PageOptions {
            limit: None,
            cursor: Some(String::new()),
        },
    ] {
        assert!(
            server
                .client
                .agents()
                .provider_associations("agent-1", &options)
                .await
                .is_err()
        );
    }
    server.assert_drained();
}
