use super::*;

#[tokio::test]
async fn accessible_hierarchy_requires_explicit_shapes_and_exact_identity() {
    let server = Server::new().await;
    let workspace_summary = json!({"access":"shared","id":"workspace-1","sandbox_id":"sandbox-1","name":"Project","status":"ready","worktree_count":1,"agent_count":1});
    let worktree = json!({"access":"shared","id":"worktree-1","workspace_id":"workspace-1","sandbox_id":"sandbox-1","name":"wt-1","status":"ready","agent_id":"agent-1"});
    let sandbox = json!({"access":"shared","id":"sandbox-1","name":"Project","status":"active","created_at":NOW,"updated_at":NOW,"last_activity_at":NOW,"workspace_count":1,"worktree_count":1,"agent_count":1,"workspaces":[workspace_summary]});
    let mut workspace = workspace_summary.clone();
    workspace["created_at"] = json!(NOW);
    workspace["worktrees"] = json!([worktree]);
    workspace["agents"] = json!([{"id":"agent-1","worktree_id":"worktree-1","workspace_id":"workspace-1","sandbox_id":"sandbox-1","status":"ready","updated_at":NOW}]);
    macro_rules! check {
        ($resource:ident, $body:expr, $id:literal, $path:literal, $result:ident) => {{
            server.expect("GET", $path, None, 200, Some($body.clone()));
            assert!(matches!(
                server
                    .client
                    .$resource()
                    .read_accessible($id)
                    .await
                    .unwrap(),
                $result::Shared(_)
            ));
            server.expect("GET", $path, None, 200, Some($body.clone()));
            assert!(matches!(
                server.client.$resource().read($id).await,
                Err(Error::Protocol { .. })
            ));
            for (field, invalid) in [
                ("id", json!("other")),
                ("access", json!("future")),
                ("access", Value::Null),
                ("status", json!("unknown")),
            ] {
                let mut body = $body.clone();
                body[field] = invalid;
                server.expect("GET", $path, None, 200, Some(body));
                assert!(matches!(
                    server.client.$resource().read_accessible($id).await,
                    Err(Error::Protocol { .. })
                ));
            }
        }};
    }
    check!(
        sandboxes,
        sandbox,
        "sandbox-1",
        "/sandboxes/sandbox-1",
        AccessibleSandboxDetail
    );
    check!(
        workspaces,
        workspace,
        "workspace-1",
        "/workspaces/workspace-1",
        AccessibleWorkspaceDetail
    );
    check!(
        worktrees,
        worktree,
        "worktree-1",
        "/worktrees/worktree-1",
        AccessibleWorktree
    );
    for count in [json!(-1), json!(1.5), json!("1")] {
        let mut body = sandbox.clone();
        body["agent_count"] = count;
        server.expect("GET", "/sandboxes/sandbox-1", None, 200, Some(body));
        assert!(matches!(
            server.client.sandboxes().read_accessible("sandbox-1").await,
            Err(Error::Protocol { .. })
        ));
    }
    server.expect(
        "GET",
        "/sandboxes/sandbox-1/workspaces",
        None,
        200,
        Some(json!({"items":[workspace_summary]})),
    );
    assert!(matches!(
        server
            .client
            .workspaces()
            .list_accessible("sandbox-1")
            .await
            .unwrap()
            .items[0],
        AccessibleWorkspaceSummary::Shared(_)
    ));
    server.expect(
        "GET",
        "/workspaces/workspace-1/worktrees",
        None,
        200,
        Some(json!({"items":[worktree]})),
    );
    assert!(matches!(
        server
            .client
            .worktrees()
            .list_accessible("workspace-1")
            .await
            .unwrap()
            .items[0],
        AccessibleWorktree::Shared(_)
    ));
    server.assert_drained();
}
