use super::*;

fn binding() -> Value {
    json!({"binding_id":"binding-one","bridge_id":"bridge-one","partition_id":"partition-one","generation":1,"previous_binding_id":null,"attached_at":NOW,"first_used_at":null,"last_used_at":null})
}
fn usage() -> Value {
    json!({"agent_id":"agent-1","channel_usage":{"active":true,"has_history":true,"first_used_at":null,"last_used_at":null}})
}

#[tokio::test]
async fn channel_inventory_filters_keep_empty_continuations_and_read_older_rows() {
    let server = Server::new().await;
    let mut options = AgentProvenanceInventoryOptions {
        shared: true,
        creation_origin: Some(AgentCreationCategory::Manual),
        channel_active: Some(false),
        channel_history: Some(true),
        ..Default::default()
    };
    server.expect("GET","/agents/provenance?shared=true&creation_origin=manual&channel_active=false&channel_history=true",None,200,Some(json!({"items":[],"next":"agt_scan"})));
    let first = server
        .client
        .agents()
        .provenance_inventory(&options)
        .await
        .unwrap();
    options.scope.filters.page.cursor = first.next;
    let row = json!({"agent_id":"agent-1","status":"deleted","created_at":NOW,"updated_at":NOW,"creation_origin":"manual","channel_usage":{"active":false,"has_history":true,"first_used_at":null,"last_used_at":null}});
    server.expect("GET","/agents/provenance?cursor=agt_scan&shared=true&creation_origin=manual&channel_active=false&channel_history=true",None,200,Some(json!({"items":[row.clone()]})));
    let page = server
        .client
        .agents()
        .provenance_inventory(&options)
        .await
        .unwrap();
    assert!(!page.items[0].channel_usage.as_ref().unwrap().active);
    assert_eq!(page.items[0].creation_origin, AgentCreationCategory::Manual);
    let mut older = row;
    older.as_object_mut().unwrap().remove("channel_usage");
    server.expect(
        "GET",
        "/agents/provenance",
        None,
        200,
        Some(json!({"items":[older]})),
    );
    assert!(
        server
            .client
            .agents()
            .provenance_inventory(&AgentProvenanceInventoryOptions::default())
            .await
            .unwrap()
            .items[0]
            .channel_usage
            .is_none()
    );
    server.assert_drained();
}
fn event(revision: u64, kind: &str) -> Value {
    let reason = match kind {
        "attach" => "attachment",
        "use" => "accepted_command",
        _ => "administrative_intervention",
    };
    let change = if kind == "replace" {
        json!({"kind":kind,"binding_id":"binding-one","before_binding_id":"previous"})
    } else {
        json!({"kind":kind,"binding_id":"binding-one"})
    };
    json!({"revision":revision,"actor":{"kind":"human","subject":"manager"},"at":NOW,"detached_at":if kind == "detach" || kind == "replace" {Some(NOW)} else {None},"reason":reason,"change":change,"before":if kind == "attach" {json!([])} else {json!([binding()])},"after":if kind == "detach" {json!([])} else {json!([binding()])}})
}
#[tokio::test]
async fn channel_usage_reads_shared_manager_and_historical_summaries() {
    let server = Server::new().await;
    server.expect(
        "GET",
        "/agents/agent-1/channel-usage",
        None,
        200,
        Some(usage()),
    );
    let shared = server
        .client
        .agents()
        .channel_usage("agent-1")
        .await
        .unwrap();
    assert!(shared.channel_usage.active);
    assert!(shared.bindings.is_none());
    let mut manager = usage();
    manager["bindings"] = json!([binding()]);
    server.expect(
        "GET",
        "/agents/agent-1/channel-usage",
        None,
        200,
        Some(manager),
    );
    assert_eq!(
        server
            .client
            .agents()
            .channel_usage("agent-1")
            .await
            .unwrap()
            .bindings
            .unwrap()[0]
            .binding_id,
        "binding-one"
    );
    for summary in [
        json!({"active":false,"has_history":false,"first_used_at":null,"last_used_at":null}),
        json!({"active":false,"has_history":true,"first_used_at":NOW,"last_used_at":NOW}),
    ] {
        server.expect(
            "GET",
            "/agents/agent-1/channel-usage",
            None,
            200,
            Some(json!({"agent_id":"agent-1","channel_usage":summary,"bindings":[]})),
        );
        assert!(
            !server
                .client
                .agents()
                .channel_usage("agent-1")
                .await
                .unwrap()
                .channel_usage
                .active
        );
    }
    server.assert_drained();
}
#[tokio::test]
async fn channel_usage_history_pages_all_transition_kinds() {
    let server = Server::new().await;
    let items: Vec<_> = ["attach", "use", "replace", "detach"]
        .into_iter()
        .enumerate()
        .map(|(i, kind)| event(i as u64 + 1, kind))
        .collect();
    server.expect(
        "GET",
        "/agents/agent-1/channel-usage/history?limit=4",
        None,
        200,
        Some(json!({"agent_id":"agent-1","items":items,"next":"4"})),
    );
    let first = server
        .client
        .agents()
        .channel_usage_history(
            "agent-1",
            &PageOptions {
                limit: Some(4),
                cursor: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(first.items.len(), 4);
    assert_eq!(first.items[3].detached_at.as_deref(), Some(NOW));
    server.expect(
        "GET",
        "/agents/agent-1/channel-usage/history?cursor=4&limit=4",
        None,
        200,
        Some(json!({"agent_id":"agent-1","items":[]})),
    );
    assert!(
        server
            .client
            .agents()
            .channel_usage_history(
                "agent-1",
                &PageOptions {
                    limit: Some(4),
                    cursor: first.next
                }
            )
            .await
            .unwrap()
            .items
            .is_empty()
    );
    server.assert_drained();
}
#[tokio::test]
async fn channel_usage_rejects_wrong_identity_shape_inconsistent_summary_and_duplicate_bindings() {
    let server = Server::new().await;
    let mut wrong = usage();
    wrong["agent_id"] = json!("another-agent");
    let mut pair = usage();
    pair["channel_usage"]["first_used_at"] = json!(NOW);
    let mut history = usage();
    history["channel_usage"]["has_history"] = json!(false);
    let mut empty = usage();
    empty["bindings"] = json!([]);
    let mut duplicate = usage();
    duplicate["bindings"] = json!([binding(), binding()]);
    let mut zero = usage();
    let mut reference = binding();
    reference["generation"] = json!(0);
    zero["bindings"] = json!([reference]);
    for malformed in [wrong, pair, history, empty, duplicate, zero] {
        server.expect(
            "GET",
            "/agents/agent-1/channel-usage",
            None,
            200,
            Some(malformed),
        );
        assert!(matches!(
            server.client.agents().channel_usage("agent-1").await,
            Err(Error::Protocol { .. })
        ));
    }
    server.assert_drained();
}
#[tokio::test]
async fn channel_usage_history_refuses_invalid_queries_and_page_integrity() {
    let server = Server::new().await;
    for cursor in ["", "0", "-1", "01", "1.1", "9223372036854775808"] {
        assert!(matches!(
            server
                .client
                .agents()
                .channel_usage_history(
                    "agent-1",
                    &PageOptions {
                        cursor: Some(cursor.into()),
                        limit: None
                    }
                )
                .await,
            Err(Error::Validation { .. })
        ));
    }
    for limit in [0, 201] {
        assert!(matches!(
            server
                .client
                .agents()
                .channel_usage_history(
                    "agent-1",
                    &PageOptions {
                        cursor: None,
                        limit: Some(limit)
                    }
                )
                .await,
            Err(Error::Validation { .. })
        ));
    }
    let mut detach = event(1, "attach");
    detach["detached_at"] = json!(NOW);
    for malformed in [
        json!({"agent_id":"another-agent","items":[]}),
        json!({"agent_id":"agent-1","items":[event(1,"attach"),event(2,"use")]}),
        json!({"agent_id":"agent-1","items":[event(2,"use")]}),
        json!({"agent_id":"agent-1","items":[],"next":"1"}),
        json!({"agent_id":"agent-1","items":[event(1,"attach")],"next":"2"}),
        json!({"agent_id":"agent-1","items":[detach]}),
    ] {
        server.expect(
            "GET",
            "/agents/agent-1/channel-usage/history?limit=1",
            None,
            200,
            Some(malformed),
        );
        assert!(matches!(
            server
                .client
                .agents()
                .channel_usage_history(
                    "agent-1",
                    &PageOptions {
                        cursor: None,
                        limit: Some(1)
                    }
                )
                .await,
            Err(Error::Protocol { .. })
        ));
    }
    server.assert_drained();
}
