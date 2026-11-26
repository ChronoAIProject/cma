use super::*;
use crate::store::{
    agent_log_segments::mongo_tests::fixtures as segment_fixtures, agents::agent_document,
};

pub(super) async fn seed(
    store: &MongoSandboxStore,
    mut legacy: Agent,
    parent: &Sandbox,
) -> (Agent, AgentId) {
    let rows = completed_turn(&legacy);
    legacy.conversation.last_seq = rows.len() as u64;
    store
        .agents
        .replace_one(
            doc! {"_id": legacy.id.as_str()},
            agent_document(&legacy).unwrap(),
        )
        .await
        .unwrap();
    store
        .agent_log
        .insert_many(rows[2..].iter().map(|row| log_document(row).unwrap()))
        .await
        .unwrap();

    let mut segmented = ready_agent(parent, "agt_segmented_response");
    segmented.created_at = legacy.created_at.clone();
    segmented.updated_at = legacy.updated_at.clone();
    segmented.ready_at = legacy.ready_at.clone();
    let mut tree = store
        .get_worktree(&legacy.worktree_id)
        .await
        .unwrap()
        .unwrap();
    tree.id = segmented.worktree_id.clone();
    tree.agent_id = Some(segmented.id.clone());
    tree.name = "wt-2".into();
    tree.path = "/home/cma/project/.worktrees/wt-2".into();
    tree.branch = "cma/wt-2".into();
    store.create_worktree(tree).await.unwrap();
    let rows = completed_turn(&segmented);
    // Put the turn start in a sealed range and leave the completion unsealed.
    let (segmented, rows) = segment_fixtures::install(store, segmented, rows, 1, 3).await;
    segment_fixtures::certify(store, &segmented, &rows).await;
    segment_fixtures::certify_output(store, &segmented, &rows).await;
    (legacy, segmented.id)
}

fn completed_turn(agent: &Agent) -> Vec<AgentLogEntry> {
    [
        AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: agent.conversation.conversation_id.clone().unwrap(),
            runtime_generation: 1,
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            origin_key: None,
            model: None,
        },
        AgentLogKind::OwnerMessage {
            content: "Fixture request".into(),
        },
        AgentLogKind::AgentMessage {
            content: "Fixture response".into(),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
    ]
    .into_iter()
    .enumerate()
    .map(|(offset, kind)| AgentLogEntry {
        agent_id: agent.id.clone(),
        seq: 1 + offset as u64,
        appended_at: NOW.into(),
        kind,
    })
    .collect()
}

pub(super) async fn assert_refusals(
    owner: &Router,
    foreign: &Router,
    unauthenticated: &Router,
    id: &AgentId,
) {
    let path = format!("/api/v1/responses/resp_{id}_3");
    let (status, response) = request(owner, Method::GET, &path, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{path} {response}");
    assert_eq!(response["code"], "sandbox_backend_unavailable");
    let hidden = request(foreign, Method::GET, &path, None).await;
    assert_eq!(hidden.0, StatusCode::NOT_FOUND, "{path} {}", hidden.1);
    assert_eq!(hidden.1["code"], "response_not_found");
    for path in [
        "/api/v1/responses/resp_agt_absent_3".into(),
        format!("/api/v1/responses/resp_{id}_4"),
        format!("/api/v1/responses/resp_{id}_7"),
        format!("/api/v1/responses/resp_{id}_03"),
    ] {
        assert_eq!(
            request(owner, Method::GET, &path, None).await,
            hidden,
            "{path}"
        );
    }
    assert_eq!(
        request(unauthenticated, Method::GET, &path, None).await.0,
        StatusCode::UNAUTHORIZED
    );
}
