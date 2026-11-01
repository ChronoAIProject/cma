use super::*;

#[tokio::test]
async fn agent_worktree_recovery_is_keyed_bodyless_and_checks_exact_identity_and_status() {
    let server = Server::new().await;
    let path = "/agents/agent-1/recover-worktree";
    let mut pending = agent();
    pending["status"] = json!("starting");
    for _ in 0..2 {
        server.expect("POST", path, None, 202, Some(pending.clone()));
        let agent = server
            .client
            .agents()
            .recover_worktree("agent-1", &key())
            .await
            .unwrap();
        assert_eq!(agent.id, "agent-1");
        assert_eq!(agent.status, AgentStatus::Starting);
    }
    let mut foreign = pending.clone();
    foreign["id"] = json!("another-agent");
    for (status, response) in [(202, foreign), (200, pending)] {
        server.expect("POST", path, None, status, Some(response));
        assert!(matches!(
            server
                .client
                .agents()
                .recover_worktree("agent-1", &key())
                .await,
            Err(Error::Protocol { .. })
        ));
    }
    server.assert_drained();
}

#[tokio::test]
async fn workspace_prepare_recovery_is_keyed_bodyless_and_checks_exact_identity_and_status() {
    let server = Server::new().await;
    let path = "/workspaces/workspace-1/recover-prepare";
    let mut pending = workspace();
    pending["status"] = json!("preparing");
    for _ in 0..2 {
        server.expect("POST", path, None, 202, Some(pending.clone()));
        let workspace = server
            .client
            .workspaces()
            .recover_prepare("workspace-1", &key())
            .await
            .unwrap();
        assert_eq!(workspace.id, "workspace-1");
        assert_eq!(workspace.status, WorkspaceStatus::Preparing);
    }
    let mut foreign = pending.clone();
    foreign["id"] = json!("another-workspace");
    for (status, response) in [(202, foreign), (200, pending)] {
        server.expect("POST", path, None, status, Some(response));
        assert!(matches!(
            server
                .client
                .workspaces()
                .recover_prepare("workspace-1", &key())
                .await,
            Err(Error::Protocol { .. })
        ));
    }
    server.assert_drained();
}

#[test]
fn sandbox_detail_preserves_optional_pinned_providers() {
    let legacy: SandboxDetail = serde_json::from_value(sandbox_detail()).unwrap();
    assert!(legacy.agent_provider.is_none());
    assert!(legacy.model_access.is_none());
    for access in [ModelAccess::ChronoaiManaged, ModelAccess::OwnCredentials] {
        let mut body = sandbox_detail();
        body["agent_provider"] = json!("codex");
        body["model_access"] = serde_json::to_value(access).unwrap();
        let detail: SandboxDetail = serde_json::from_value(body).unwrap();
        assert_eq!(detail.agent_provider, Some(AgentProvider::Codex));
        assert_eq!(detail.model_access, Some(access));
    }
    for (field, value) in [("agent_provider", "unknown"), ("model_access", "unknown")] {
        let mut body = sandbox_detail();
        body[field] = json!(value);
        assert!(serde_json::from_value::<SandboxDetail>(body).is_err());
    }
}

#[test]
fn sandbox_colour_is_optional_and_only_in_its_single_spelling() {
    let legacy: SandboxDetail = serde_json::from_value(sandbox_detail()).unwrap();
    assert!(legacy.summary.sandbox.color.is_none());
    let mut body = sandbox_detail();
    body["color"] = json!("#2f96fa");
    let detail: SandboxDetail = serde_json::from_value(body).unwrap();
    assert_eq!(detail.summary.sandbox.color.as_deref(), Some("#2f96fa"));
    for invalid in [
        json!("#2F96FA"),
        json!("2f96fa"),
        json!("#2f96f"),
        json!("blue"),
        json!(null),
        json!(7),
    ] {
        let mut body = sandbox_detail();
        body["color"] = invalid;
        assert!(serde_json::from_value::<SandboxDetail>(body).is_err());
    }
}

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

#[tokio::test]
async fn shared_sandbox_inventory_preserves_safe_summaries_and_personal_method_types() {
    let server = Server::new().await;
    let summary = json!({"access":"shared","id":"sandbox-1","name":"Project","status":"active","created_at":NOW,"updated_at":NOW,"last_activity_at":NOW,"workspace_count":1,"worktree_count":2,"agent_count":2});
    let options = AccessibleSandboxListOptions {
        organization: Some("org".into()),
        shared: false,
        filters: SandboxListOptions {
            status: Some(SandboxStatus::Active),
            page: PageOptions {
                cursor: None,
                limit: Some(1),
            },
        },
    };
    server.expect(
        "GET",
        "/sandboxes?limit=1&status=active&organization=org",
        None,
        200,
        Some(json!({"items":[summary],"next":"sandbox-1"})),
    );
    let page = server
        .client
        .sandboxes()
        .list_accessible(&options)
        .await
        .unwrap();
    assert!(matches!(page.items[0], AccessibleSandboxSummary::Shared(_)));
    assert_eq!(page.next.as_deref(), Some("sandbox-1"));
    server.expect(
        "GET",
        "/sandboxes",
        None,
        200,
        Some(json!({"items":[summary]})),
    );
    assert!(matches!(
        server
            .client
            .sandboxes()
            .list(&SandboxListOptions::default())
            .await,
        Err(Error::Protocol { .. })
    ));
    for access in [json!("future"), Value::Null] {
        let mut invalid = summary.clone();
        invalid["access"] = access;
        server.expect(
            "GET",
            "/sandboxes?limit=1&status=active&organization=org",
            None,
            200,
            Some(json!({"items":[invalid]})),
        );
        assert!(matches!(
            server.client.sandboxes().list_accessible(&options).await,
            Err(Error::Protocol { .. })
        ));
    }
    server.assert_drained();
}

#[tokio::test]
async fn shared_sandbox_detail_revision_is_optional_but_positive_when_present() {
    let server = Server::new().await;
    let base = json!({"access":"shared","id":"sandbox-1","name":"Project","status":"active","created_at":NOW,"updated_at":NOW,"last_activity_at":NOW,"workspace_count":0,"worktree_count":0,"agent_count":0,"workspaces":[]});
    for revision in [None, Some(7)] {
        let mut body = base.clone();
        if let Some(revision) = revision {
            body["revision"] = json!(revision);
        }
        server.expect("GET", "/sandboxes/sandbox-1", None, 200, Some(body));
        let AccessibleSandboxDetail::Shared(detail) = server
            .client
            .sandboxes()
            .read_accessible("sandbox-1")
            .await
            .unwrap()
        else {
            panic!("shared detail required")
        };
        assert_eq!(detail.revision, revision);
    }
    for revision in [json!(0), json!(-1), json!(1.5), json!("7"), Value::Null] {
        let mut body = base.clone();
        body["revision"] = revision;
        server.expect("GET", "/sandboxes/sandbox-1", None, 200, Some(body));
        assert!(matches!(
            server.client.sandboxes().read_accessible("sandbox-1").await,
            Err(Error::Protocol { .. })
        ));
    }
    server.assert_drained();
}

#[tokio::test]
async fn aggregate_shared_sandbox_scope_pages_and_refuses_mixed_scope_locally() {
    let server = Server::new().await;
    let summary = json!({"access":"shared","id":"sandbox-1","name":"Project","status":"active","created_at":NOW,"updated_at":NOW,"last_activity_at":NOW,"workspace_count":1,"worktree_count":2,"agent_count":2});
    let mut options = AccessibleSandboxListOptions {
        shared: true,
        organization: None,
        filters: SandboxListOptions {
            status: Some(SandboxStatus::Active),
            page: PageOptions {
                cursor: Some("sandbox-0".into()),
                limit: Some(1),
            },
        },
    };
    server.expect(
        "GET",
        "/sandboxes?cursor=sandbox-0&limit=1&status=active&shared=true",
        None,
        200,
        Some(json!({"items":[summary],"next":"sandbox-1"})),
    );
    let page = server
        .client
        .sandboxes()
        .list_accessible(&options)
        .await
        .unwrap();
    assert!(matches!(page.items[0], AccessibleSandboxSummary::Shared(_)));
    assert_eq!(page.next.as_deref(), Some("sandbox-1"));
    options.organization = Some("org".into());
    assert!(matches!(
        server.client.sandboxes().list_accessible(&options).await,
        Err(Error::Validation { .. })
    ));
    server.expect("GET", "/sandboxes", None, 200, Some(json!({"items":[]})));
    assert!(
        server
            .client
            .sandboxes()
            .list_accessible(&AccessibleSandboxListOptions::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    server.assert_drained();
}
