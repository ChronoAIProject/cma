use super::*;

pub(in crate::rest) const ID: &str = "agt_worktree_recovery";
const PATH: &str = "/api/v1/agents/agt_worktree_recovery/recover-worktree";

pub(in crate::rest) async fn recovery_fixture() -> Fixture {
    let f = fixture().await;
    let mut parent = f.store.get_sandbox(&f.sandbox_id).await.unwrap().unwrap();
    let revision = parent.revision;
    parent.credential.delivered = true;
    parent.revision += 1;
    f.store.save_sandbox(revision, parent).await.unwrap();
    let mut workspace = f
        .store
        .get_workspace(&f.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let revision = workspace.revision;
    workspace.next_worktree_number = 2;
    workspace.revision += 1;
    f.store.save_workspace(revision, workspace).await.unwrap();
    let mut agent = seeded_agent(ID, AgentStatus::Failed, 0, None);
    agent.ready_at = None;
    agent.conversation.conversation_id = None;
    agent.conversation.runtime_generation = None;
    agent.failure = Some(layer2_core::sandbox::TypedFailure {
        class: "agent_runtime_rejected".into(),
        detail: "runtime_mutation_outcome_unknown: worktree addition left partial or mismatched Git state; explicit recovery is required".into(),
    });
    let mut worktree = seeded_worktree(&agent);
    worktree.name = "wt-1".into();
    worktree.path = "/home/cma/project/.worktrees/wt-1".into();
    worktree.branch = "cma/wt-1".into();
    worktree.status = WorktreeStatus::Failed;
    let request = layer3_adapter::WorktreeRequest::new(
        "worktree:project:wt-1:add:v1",
        "project",
        "wt-1",
        worktree.base_commit.clone(),
    );
    agent.work.cleanup_debt = vec![layer2_core::sandbox::AgentCleanupDebt {
        step: AgentWorkStep::AddWorktree,
        operation_id: request.operation_id,
        request_sha256: request.request_sha256,
    }];
    f.store.create_worktree(worktree).await.unwrap();
    f.store.create_agent(agent.clone()).await.unwrap();
    f.store
        .append_agent_log(
            &agent.id,
            NOW.into(),
            AgentLogKind::Created {
                workspace_id: agent.workspace_id,
                worktree_id: agent.worktree_id,
            },
        )
        .await
        .unwrap();
    f
}

#[tokio::test]
async fn recovery_requires_current_role_exact_owner_and_original_key() {
    let f = recovery_fixture().await;
    let mut anonymous = request(Method::POST, PATH, OWNER, Some("auth"), None);
    anonymous.extensions_mut().remove::<AuthenticatedSubject>();
    assert_eq!(
        response(&f.router, anonymous).await.0,
        StatusCode::UNAUTHORIZED
    );
    let mut forbidden = request(Method::POST, PATH, OWNER, Some("auth"), None);
    forbidden.extensions_mut().insert(Authorization::none());
    assert_eq!(
        response(&f.router, forbidden).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        response(
            &f.router,
            request(Method::POST, PATH, OTHER, Some("auth"), None)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        response(&f.router, request(Method::POST, PATH, OWNER, None, None))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let first = json_response(
        &f.router,
        request(Method::POST, PATH, OWNER, Some("recover"), None),
    )
    .await;
    assert_eq!(first.0, StatusCode::ACCEPTED, "{}", first.1);
    assert_eq!(first.1["id"], ID);
    assert_eq!(first.1["status"], "starting");
    assert_eq!(first.1["worktree"]["name"], "wt-2");
    assert_eq!(
        json_response(
            &f.router,
            request(Method::POST, PATH, OWNER, Some("recover"), None)
        )
        .await,
        first
    );
    let conflict = json_response(
        &f.router,
        request(Method::POST, PATH, OWNER, Some("another"), None),
    )
    .await;
    assert_eq!(conflict.0, StatusCode::CONFLICT);
    assert_eq!(conflict.1["code"], "agent_worktree_recovery_conflict");
    let retained = f
        .store
        .get_worktree(&WorktreeId(format!("wkt_{ID}")))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.status, WorktreeStatus::Failed);
    assert_eq!(retained.path, "/home/cma/project/.worktrees/wt-1");
    assert!(retained.agent_id.is_none());
}

#[tokio::test]
async fn admin_recovery_returns_a_view_without_impersonating_the_owner() {
    let f = recovery_fixture().await;
    let mut command = request(Method::POST, PATH, "operator", Some("admin"), None);
    command
        .extensions_mut()
        .insert(Authorization::new([Capability::ControlAnySandbox]));
    let result = json_response(&f.router, command).await;
    assert_eq!(result.0, StatusCode::ACCEPTED, "{}", result.1);
    assert_eq!(result.1["id"], ID);
    let agent = f
        .store
        .get_agent(&AgentId(ID.into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(agent.owner.as_str(), OWNER);
    assert_eq!(
        agent.work.worktree_recoveries[0].requested_by.as_str(),
        "operator"
    );
}
