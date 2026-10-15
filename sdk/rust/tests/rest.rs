mod support;

#[path = "rest/artifacts.rs"]
mod artifacts;
#[path = "rest/attachments.rs"]
mod attachments;
#[path = "rest/channel_usage.rs"]
mod channel_usage;
#[path = "rest/hierarchy.rs"]
mod hierarchy;
#[path = "rest/lifecycle.rs"]
mod lifecycle;
#[path = "rest/source.rs"]
mod source;

use cma_sdk::{
    Error, EventOptions, IdempotencyKey, PageOptions, agents::*, instances::*, profiles::*,
};
use serde_json::{Value, json};
use support::{Server, fixtures::*, key};

fn page() -> PageOptions {
    PageOptions {
        cursor: Some("a/b ?&".into()),
        limit: Some(3),
    }
}

fn reference_value() -> ProfileReference {
    serde_json::from_value(reference()).unwrap()
}

#[test]
fn isolated_agent_profile_roundtrip_preserves_true_and_omits_the_legacy_default() {
    let legacy = content(ProfileKind::Agent);
    let ordinary: AgentProfileContent = serde_json::from_value(legacy.clone()).unwrap();
    assert!(!ordinary.isolated_agent);
    assert_eq!(serde_json::to_value(ordinary).unwrap(), legacy);
    let mut isolated = legacy.clone();
    isolated["isolated_agent"] = true.into();
    let decoded: AgentProfileContent = serde_json::from_value(isolated.clone()).unwrap();
    assert!(decoded.isolated_agent);
    assert_eq!(serde_json::to_value(decoded).unwrap(), isolated);
    let mut explicit_default = legacy.clone();
    explicit_default["isolated_agent"] = false.into();
    let decoded: AgentProfileContent = serde_json::from_value(explicit_default).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), legacy);
}

async fn profile_contract<C: ProfileContent>(server: &Server, profiles: Profiles<'_, C>) {
    let kind = C::KIND;
    let path = format!("/{}", C::COLLECTION);
    let id = format!("{}-1", kind.as_str());
    let item = format!("{path}/{id}");
    server.expect(
        "GET",
        &format!("{path}?cursor=a%2Fb+%3F%26&limit=3"),
        None,
        200,
        Some(json!({"items":[profile(kind)],"next":"next-1"})),
    );
    let listed = profiles.list(&page()).await.unwrap();
    assert_eq!(listed.items[0].id, id);
    assert_eq!(listed.next.as_deref(), Some("next-1"));
    server.expect("GET", &item, None, 200, Some(profile(kind)));
    assert_eq!(profiles.read(&id).await.unwrap().id, id);
    let draft = json!({"name":"Project","description":"A test profile","content":content(kind)});
    server.expect("POST", &path, Some(draft.clone()), 201, Some(profile(kind)));
    assert_eq!(
        profiles
            .create(&serde_json::from_value(draft).unwrap(), &key())
            .await
            .unwrap()
            .id,
        id
    );
    let update = json!({"draft_version":1,"content":content(kind)});
    server.expect("PUT", &item, Some(update.clone()), 200, Some(profile(kind)));
    profiles
        .save(&id, &serde_json::from_value(update).unwrap(), &key())
        .await
        .unwrap();
    let revisions = format!("{item}/revisions");
    server.expect(
        "POST",
        &revisions,
        Some(json!({"draft_version":1})),
        201,
        Some(revision(kind)),
    );
    assert_eq!(
        profiles
            .publish(&id, 1, &key())
            .await
            .unwrap()
            .summary
            .revision,
        1
    );
    server.expect(
        "GET",
        &revisions,
        None,
        200,
        Some(json!({"items":[revision(kind)]})),
    );
    assert_eq!(
        profiles
            .list_revisions(&id, &PageOptions::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    server.expect(
        "GET",
        &format!("{revisions}/1"),
        None,
        200,
        Some(revision(kind)),
    );
    assert_eq!(profiles.read_revision(&id, 1).await.unwrap().profile_id, id);
    server.expect(
        "PUT",
        &format!("{item}/visibility"),
        Some(json!({"visibility":"public"})),
        200,
        Some(profile(kind)),
    );
    profiles
        .set_visibility(&id, ProfileVisibility::Public, &key())
        .await
        .unwrap();
    server.expect(
        "POST",
        &format!("{item}/retire"),
        None,
        200,
        Some(profile(kind)),
    );
    profiles.retire(&id, &key()).await.unwrap();

    let market = server.client.marketplace::<C>();
    let entry = marketplace(kind);
    server.expect(
        "GET",
        &format!("/marketplace{path}?cursor=a%2Fb+%3F%26&limit=3&q=project+%26+work"),
        None,
        200,
        Some(json!({"items":[entry["summary"]],"next":"market-next"})),
    );
    let found = market
        .list(&MarketplaceListOptions {
            q: Some("project & work".into()),
            page: page(),
        })
        .await
        .unwrap();
    assert_eq!(found.items[0].kind, kind);
    server.expect(
        "GET",
        &format!("/marketplace{item}"),
        None,
        200,
        Some(entry),
    );
    assert_eq!(market.read(&id).await.unwrap().summary.id, id);
    server.expect(
        "POST",
        &format!("/marketplace{item}/fork"),
        Some(json!({"revision":1})),
        201,
        Some(profile(kind)),
    );
    assert_eq!(market.fork(&id, 1, &key()).await.unwrap().id, id);
    server.assert_drained();
}

#[tokio::test]
async fn every_profile_and_marketplace_operation_for_each_kind() {
    let server = Server::new().await;
    profile_contract(&server, server.client.agent_profiles()).await;
    profile_contract(&server, server.client.workspace_profiles()).await;
    profile_contract(&server, server.client.sandbox_profiles()).await;
}

#[tokio::test]
async fn sandbox_workspace_and_worktree_contracts() {
    let server = Server::new().await;
    let sandboxes = server.client.sandboxes();
    server.expect(
        "GET",
        "/sandboxes?cursor=a%2Fb+%3F%26&limit=3&status=active",
        None,
        200,
        Some(json!({"items":[sandbox_summary()],"next":"next-1"})),
    );
    let listed = sandboxes
        .list(&SandboxListOptions {
            page: page(),
            status: Some(SandboxStatus::Active),
        })
        .await
        .unwrap();
    assert_eq!(listed.items[0].agent_count, 1);
    server.expect(
        "GET",
        "/sandboxes/sandbox-1",
        None,
        200,
        Some(sandbox_detail()),
    );
    assert_eq!(
        sandboxes.read("sandbox-1").await.unwrap().workspaces.len(),
        1
    );
    let create = CreateSandboxRequest {
        sandbox_profile: reference_value(),
        name: Some("Project".into()),
        trust_confirmed: true,
        commands_digest: Some("digest-1".into()),
    };
    server.expect(
        "POST",
        "/sandboxes",
        Some(serde_json::to_value(&create).unwrap()),
        201,
        Some(sandbox()),
    );
    assert_eq!(
        sandboxes.create(&create, &key()).await.unwrap().id,
        "sandbox-1"
    );
    server.expect(
        "POST",
        "/sandboxes/sandbox-1/pause",
        None,
        202,
        Some(sandbox()),
    );
    sandboxes.pause("sandbox-1", &key()).await.unwrap();
    server.expect(
        "POST",
        "/sandboxes/sandbox-1/revive",
        None,
        202,
        Some(sandbox()),
    );
    sandboxes.revive("sandbox-1", &key()).await.unwrap();
    server.expect("DELETE", "/sandboxes/sandbox-1", None, 202, Some(sandbox()));
    sandboxes.delete("sandbox-1", &key()).await.unwrap();
    server.expect("GET", "/sandboxes/sandbox-1/events?after=4&limit=3", None, 200,
        Some(json!([{"sandbox_id":"sandbox-1","seq":5,"appended_at":NOW,"kind":{"type":"work_step_completed","step":{"kind":"create_profile_workspace","index":0}}}])));
    let events = sandboxes
        .events(
            "sandbox-1",
            EventOptions {
                after: Some(4),
                limit: Some(3),
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        events[0].kind,
        SandboxEventKind::WorkStepCompleted {
            step: SandboxWorkStep::CreateProfileWorkspace { index: 0 }
        }
    ));

    let workspaces = server.client.workspaces();
    server.expect(
        "GET",
        "/sandboxes/sandbox-1/workspaces",
        None,
        200,
        Some(json!({"items":[workspace_summary()]})),
    );
    assert_eq!(
        workspaces.list("sandbox-1").await.unwrap().items[0].sandbox_id,
        "sandbox-1"
    );
    server.expect(
        "GET",
        "/workspaces/workspace-1",
        None,
        200,
        Some(workspace_detail()),
    );
    assert_eq!(
        workspaces.read("workspace-1").await.unwrap().agents.len(),
        1
    );
    let create = CreateWorkspaceRequest {
        workspace_profile: reference_value(),
        trust_confirmed: false,
        commands_digest: None,
    };
    server.expect(
        "POST",
        "/sandboxes/sandbox-1/workspaces",
        Some(json!({"workspace_profile":reference()})),
        201,
        Some(workspace()),
    );
    workspaces
        .create("sandbox-1", &create, &key())
        .await
        .unwrap();
    server.expect(
        "DELETE",
        "/workspaces/workspace-1",
        None,
        202,
        Some(workspace()),
    );
    workspaces.delete("workspace-1", &key()).await.unwrap();
    let worktrees = server.client.worktrees();
    server.expect(
        "GET",
        "/workspaces/workspace-1/worktrees",
        None,
        200,
        Some(json!({"items":[worktree()]})),
    );
    assert_eq!(worktrees.list("workspace-1").await.unwrap().items.len(), 1);
    server.expect("GET", "/worktrees/worktree-1", None, 200, Some(worktree()));
    assert_eq!(
        worktrees
            .read("worktree-1")
            .await
            .unwrap()
            .agent_id
            .as_deref(),
        Some("agent-1")
    );
    server.assert_drained();
}

#[tokio::test]
async fn every_agent_command_and_durable_read_contract() {
    let server = Server::new().await;
    let agents = server.client.agents();
    server.expect(
        "GET",
        "/agents?cursor=a%2Fb+%3F%26&limit=3&sandbox=sandbox-1&workspace=workspace-1&status=ready",
        None,
        200,
        Some(json!({"items":[agent_summary()]})),
    );
    let found = agents
        .list(&AgentListOptions {
            sandbox: Some("sandbox-1".into()),
            workspace: Some("workspace-1".into()),
            status: Some(AgentStatus::Ready),
            page: page(),
        })
        .await
        .unwrap();
    assert_eq!(found.items[0].worktree_id, "worktree-1");
    server.expect("GET", "/agents/agent-1", None, 200, Some(agent()));
    assert_eq!(
        agents.read("agent-1").await.unwrap().sandbox.id,
        "sandbox-1"
    );
    let create = CreateAgentRequest {
        agent_profile: reference_value(),
        title: None,
        first_message: Some("Review changes.".into()),
    };
    server.expect(
        "POST",
        "/workspaces/workspace-1/agents",
        Some(json!({"agent_profile":reference(),"first_message":"Review changes."})),
        201,
        Some(agent()),
    );
    agents.create("workspace-1", &create, &key()).await.unwrap();
    let message = AgentTextRequest {
        text: "Review changes.".into(),
    };
    server.expect(
        "POST",
        "/agents/agent-1/input",
        Some(json!({"text":message.text})),
        202,
        Some(json!({"queued":true})),
    );
    assert!(
        agents
            .input("agent-1", &message, &key())
            .await
            .unwrap()
            .queued
    );
    server.expect(
        "POST",
        "/agents/agent-1/steer",
        Some(json!({"text":message.text})),
        202,
        None,
    );
    agents.steer("agent-1", &message, &key()).await.unwrap();
    server.expect("POST", "/agents/agent-1/interrupt", None, 202, None);
    agents.interrupt("agent-1", &key()).await.unwrap();
    server.expect(
        "POST",
        "/agents/agent-1/approvals/request%2F1%3F%23",
        Some(json!({"decision":"deny"})),
        202,
        None,
    );
    agents
        .answer_approval(
            "agent-1",
            "request/1?#",
            &ApprovalDecisionRequest {
                decision: ApprovalDecision::Deny,
            },
            &key(),
        )
        .await
        .unwrap();
    server.expect(
        "POST",
        "/agents/agent-1/stop",
        Some(json!({"reason":"owner"})),
        202,
        None,
    );
    agents
        .stop(
            "agent-1",
            &StopAgentRequest {
                reason: Some(StopReason::Owner),
            },
            &key(),
        )
        .await
        .unwrap();
    server.expect("DELETE", "/agents/agent-1", None, 202, None);
    agents.delete("agent-1", &key()).await.unwrap();
    server.expect("GET", "/agents/agent-1/events?after=0&limit=3", None, 200,
        Some(json!({"items":[{"seq":1,"appended_at":NOW,"kind":{"type":"operation_delivery_changed","operation_sequence":1,"operation":"send_message","state":"write_confirmed"}}],"next":1})));
    let events = agents
        .events(
            "agent-1",
            EventOptions {
                after: Some(0),
                limit: Some(3),
            },
        )
        .await
        .unwrap();
    assert_eq!(events.next, Some(1));
    assert!(matches!(
        events.items[0].kind,
        AgentEventKind::OperationDeliveryChanged {
            state: AgentOperationDelivery::WriteConfirmed,
            ..
        }
    ));
    server.expect("GET", "/agents/agent-1/transcript", None, 200,
        Some(json!({"agent_id":"agent-1","through_seq":3,"turns":[{"turn_sequence":1,"started_at":NOW,"owner_message":"Review","agent_messages":["Done"],"messages":[{"sequence":1,"role":"user","content":"Review","appended_at":NOW},{"sequence":2,"role":"assistant","content":"Done","appended_at":NOW}],"outcome":{"type":"completed","at":NOW}}]})));
    assert!(matches!(
        agents.transcript("agent-1").await.unwrap().turns[0].outcome,
        TranscriptOutcome::Completed { .. }
    ));
    server.expect("GET", "/agents/agent-1/usage", None, 200,
        Some(json!({"agent_id":"agent-1","through_seq":3,"input_tokens":10,"output_tokens":20,"context_used":30,"context_window":1000})));
    assert_eq!(agents.usage("agent-1").await.unwrap().input_tokens, 10);
    server.expect(
        "GET",
        "/agents/agent-1/resources",
        None,
        200,
        Some(resources()),
    );
    assert_eq!(
        agents.resources("agent-1").await.unwrap().cpu_usage_usec,
        None
    );
    server.assert_drained();
}

#[tokio::test]
async fn identifiers_are_encoded_as_single_segments_and_ids_are_checked() {
    let server = Server::new().await;
    let id = "agent/with ?#%";
    let mut response = agent();
    response["id"] = json!(id);
    server.expect(
        "GET",
        "/agents/agent%2Fwith%20%3F%23%25",
        None,
        200,
        Some(response),
    );
    assert_eq!(server.client.agents().read(id).await.unwrap().id, id);
    server.expect("GET", "/agents/agent-2", None, 200, Some(agent()));
    assert!(matches!(
        server.client.agents().read("agent-2").await,
        Err(Error::Protocol { .. })
    ));
    server.expect(
        "GET",
        "/sandboxes/sandbox-2",
        None,
        200,
        Some(sandbox_detail()),
    );
    assert!(matches!(
        server.client.sandboxes().read("sandbox-2").await,
        Err(Error::Protocol { .. })
    ));
    server.expect(
        "GET",
        "/workspaces/workspace-2",
        None,
        200,
        Some(workspace_detail()),
    );
    assert!(matches!(
        server.client.workspaces().read("workspace-2").await,
        Err(Error::Protocol { .. })
    ));
    server.expect("GET", "/worktrees/worktree-2", None, 200, Some(worktree()));
    assert!(matches!(
        server.client.worktrees().read("worktree-2").await,
        Err(Error::Protocol { .. })
    ));
    server.assert_drained();
}

#[tokio::test]
async fn marketplace_refuses_wrong_kinds_identities_revisions_and_missing_trust() {
    let server = Server::new().await;
    let market = server.client.marketplace::<SandboxProfileContent>();
    for (pointer, replacement) in [
        ("/summary/kind", json!("agent")),
        ("/summary/id", json!("wrong")),
        ("/revision/profile_id", json!("wrong")),
        ("/summary/latest_revision", json!(2)),
        ("/trust", Value::Null),
    ] {
        let mut response = marketplace(ProfileKind::Sandbox);
        *response.pointer_mut(pointer).unwrap() = replacement;
        server.expect(
            "GET",
            "/marketplace/sandbox-profiles/sandbox-1",
            None,
            200,
            Some(response),
        );
        assert!(matches!(
            market.read("sandbox-1").await,
            Err(Error::Protocol { .. })
        ));
    }
    server.expect(
        "GET",
        "/marketplace/sandbox-profiles",
        None,
        200,
        Some(json!({"items":[marketplace(ProfileKind::Agent)["summary"]]})),
    );
    assert!(matches!(
        market.list(&MarketplaceListOptions::default()).await,
        Err(Error::Protocol { .. })
    ));
    let mut revision = revision(ProfileKind::Agent);
    revision["revision"] = json!(2);
    server.expect(
        "GET",
        "/agent-profiles/agent-1/revisions/1",
        None,
        200,
        Some(revision),
    );
    assert!(matches!(
        server
            .client
            .agent_profiles()
            .read_revision("agent-1", 1)
            .await,
        Err(Error::Protocol { .. })
    ));
    server.assert_drained();
}

#[tokio::test]
async fn rejects_wrong_success_statuses_and_nonempty_agent_command_bodies() {
    let server = Server::new().await;
    server.expect("GET", "/agents/agent-1", None, 201, Some(agent()));
    assert!(matches!(
        server.client.agents().read("agent-1").await,
        Err(Error::Protocol { .. })
    ));
    server.expect(
        "POST",
        "/agents/agent-1/interrupt",
        None,
        202,
        Some(json!({})),
    );
    assert!(matches!(
        server.client.agents().interrupt("agent-1", &key()).await,
        Err(Error::Protocol { .. })
    ));
    server.expect(
        "POST",
        "/agents/agent-1/input",
        Some(json!({"text":"review"})),
        202,
        Some(json!({})),
    );
    assert!(matches!(
        server
            .client
            .agents()
            .input(
                "agent-1",
                &AgentTextRequest {
                    text: "review".into()
                },
                &key()
            )
            .await,
        Err(Error::Protocol { .. })
    ));
    server.assert_drained();
}

#[tokio::test]
async fn rejects_invalid_requests_before_sending() {
    let server = Server::new().await;
    for value in ["", ".", ".."] {
        assert!(matches!(
            server.client.agents().read(value).await,
            Err(Error::Validation { .. })
        ));
    }
    let profiles = server.client.agent_profiles();
    assert!(matches!(
        profiles.publish("agent-1", 0, &key()).await,
        Err(Error::Validation { .. })
    ));
    assert!(matches!(
        profiles.read_revision("agent-1", 0).await,
        Err(Error::Validation { .. })
    ));
    let update = UpdateProfile {
        availability: None,
        draft_version: 0,
        name: None,
        description: None,
        content: serde_json::from_value(content(ProfileKind::Agent)).unwrap(),
    };
    assert!(matches!(
        profiles.save("agent-1", &update, &key()).await,
        Err(Error::Validation { .. })
    ));
    assert!(matches!(
        server
            .client
            .marketplace::<AgentProfileContent>()
            .fork("agent-1", 0, &key())
            .await,
        Err(Error::Validation { .. })
    ));
    let reference = ProfileReference {
        id: "profile-1".into(),
        revision: 0,
    };
    let sandbox = CreateSandboxRequest {
        sandbox_profile: reference.clone(),
        name: None,
        trust_confirmed: false,
        commands_digest: None,
    };
    assert!(matches!(
        server.client.sandboxes().create(&sandbox, &key()).await,
        Err(Error::Validation { .. })
    ));
    let workspace = CreateWorkspaceRequest {
        workspace_profile: reference.clone(),
        trust_confirmed: false,
        commands_digest: None,
    };
    assert!(matches!(
        server
            .client
            .workspaces()
            .create("sandbox-1", &workspace, &key())
            .await,
        Err(Error::Validation { .. })
    ));
    let agent = CreateAgentRequest {
        agent_profile: reference.clone(),
        title: None,
        first_message: None,
    };
    assert!(matches!(
        server
            .client
            .agents()
            .create("workspace-1", &agent, &key())
            .await,
        Err(Error::Validation { .. })
    ));
    let mut workspace_content: WorkspaceProfileContent =
        serde_json::from_value(content(ProfileKind::Workspace)).unwrap();
    workspace_content.agent_profiles.push(reference.clone());
    let workspace = ProfileDraft {
        availability: None,
        name: "Project".into(),
        description: String::new(),
        content: workspace_content,
    };
    assert!(matches!(
        server
            .client
            .workspace_profiles()
            .create(&workspace, &key())
            .await,
        Err(Error::Validation { .. })
    ));
    let mut sandbox_content: SandboxProfileContent =
        serde_json::from_value(content(ProfileKind::Sandbox)).unwrap();
    sandbox_content.workspace_profiles.push(reference);
    let mut sandbox = ProfileDraft {
        availability: None,
        name: "Project".into(),
        description: String::new(),
        content: sandbox_content,
    };
    assert!(matches!(
        server
            .client
            .sandbox_profiles()
            .create(&sandbox, &key())
            .await,
        Err(Error::Validation { .. })
    ));
    sandbox.content.workspace_profiles.clear();
    sandbox.content.cpu_millis = 0;
    assert!(matches!(
        server
            .client
            .sandbox_profiles()
            .create(&sandbox, &key())
            .await,
        Err(Error::Validation { .. })
    ));
    server.assert_drained();
}

#[test]
fn validates_idempotency_keys() {
    for value in [
        String::new(),
        "a b".into(),
        "a\nb".into(),
        "\u{e9}".into(),
        "x".repeat(256),
    ] {
        assert!(matches!(
            IdempotencyKey::new(value),
            Err(Error::Validation { .. })
        ));
    }
    assert_eq!(
        IdempotencyKey::new("x".repeat(255)).unwrap().as_str().len(),
        255
    );
    assert_eq!(key().clone(), key());
}

#[test]
fn typed_models_reject_invalid_fields_and_unknown_tags() {
    for replacement in [json!(0), json!(-1), json!(1.5), json!(4294967296_u64)] {
        let mut response = reference();
        response["revision"] = replacement;
        assert!(serde_json::from_value::<ProfileReference>(response).is_err());
    }
    for pointer in [
        "/cpu_millis",
        "/memory_mib",
        "/max_workspaces",
        "/max_agents_per_workspace",
    ] {
        let mut response = content(ProfileKind::Sandbox);
        *response.pointer_mut(pointer).unwrap() = json!(0);
        assert!(serde_json::from_value::<SandboxProfileContent>(response).is_err());
    }
    let mut response = agent();
    response["status"] = json!("future_state");
    assert!(serde_json::from_value::<Agent>(response).is_err());
    let mut response = profile(ProfileKind::Agent);
    response["draft_version"] = json!(0);
    assert!(serde_json::from_value::<ProfileDocument<AgentProfileContent>>(response).is_err());
    assert!(
        serde_json::from_value::<AgentEventKind>(json!({"type":"live_delta","text":"no"})).is_err()
    );
    assert!(serde_json::from_value::<AgentEventKind>(json!({"type":"owner_message"})).is_err());
    assert!(serde_json::from_value::<SandboxWorkStep>(json!({"kind":"future_step"})).is_err());
    assert!(serde_json::from_value::<WorkspaceLocation>(json!({"type":"unknown"})).is_err());
}

#[test]
fn profile_content_and_dependency_variants_are_typed() {
    let mut agent = content(ProfileKind::Agent);
    agent["model"] = json!("model-1");
    agent["skills"] = json!([{"source":"ornn","id":"skill-1","version":"1"}]);
    let content: AgentProfileContent = serde_json::from_value(agent.clone()).unwrap();
    assert_eq!(serde_json::to_value(content).unwrap(), agent);
    let location = json!({"type":"git_repository","url":"https://example.invalid/project.git","reference":"main"});
    let typed: WorkspaceLocation = serde_json::from_value(location.clone()).unwrap();
    assert_eq!(serde_json::to_value(typed).unwrap(), location);
    let source: WorkspaceSource = serde_json::from_value(location.clone()).unwrap();
    assert_eq!(serde_json::to_value(source).unwrap(), location);
    for kind in [
        ProfileKind::Agent,
        ProfileKind::Workspace,
        ProfileKind::Sandbox,
    ] {
        let dependency = json!({"kind":kind,"name":"Project","revision":revision(kind)});
        serde_json::from_value::<ResolvedProfileRevision>(dependency).unwrap();
    }
    let draft: UpdateProfile<AgentProfileContent> = serde_json::from_value(
        json!({"draft_version":1,"name":"Renamed","description":"Updated","content":agent}),
    )
    .unwrap();
    assert_eq!(serde_json::to_value(draft).unwrap()["name"], "Renamed");
}

#[tokio::test]
async fn accessible_agent_keeps_shared_and_personal_shapes_and_checks_identity() {
    let server = Server::new().await;
    let personal = agent();
    let id = personal["id"].as_str().unwrap();
    let path = format!("/agents/{id}");
    let shared = json!({"access":"shared","id":id,"status":"ready","created_at":"2026-09-05T00:00:00Z","updated_at":"2026-09-05T00:00:00Z"});
    server.expect("GET", &path, None, 200, Some(shared.clone()));
    assert!(matches!(
        server.client.agents().read_accessible(id).await.unwrap(),
        AccessibleAgent::Shared(_)
    ));
    server.expect("GET", &path, None, 200, Some(personal.clone()));
    assert!(matches!(
        server.client.agents().read_accessible(id).await.unwrap(),
        AccessibleAgent::Personal(_)
    ));
    server.expect("GET", &path, None, 200, Some(shared.clone()));
    assert!(server.client.agents().read(id).await.is_err());
    for (key, value) in [
        ("id", json!("other")),
        ("access", json!("future")),
        ("status", Value::Null),
        ("access", Value::Null),
    ] {
        let mut malformed = shared.clone();
        malformed[key] = value;
        server.expect("GET", &path, None, 200, Some(malformed));
        assert!(server.client.agents().read_accessible(id).await.is_err());
    }
}

#[tokio::test]
async fn organization_inventory_parses_shared_summaries_and_rejects_unknown_roles() {
    let server = Server::new().await;
    let shared = json!({"access":"shared","id":"agt_shared","status":"ready","created_at":"2026-09-05T00:00:00Z","updated_at":"2026-09-05T00:00:00Z"});
    let options = AccessibleAgentListOptions {
        organization: Some("org".into()),
        ..Default::default()
    };
    server.expect(
        "GET",
        "/agents?organization=org",
        None,
        200,
        Some(json!({"items":[agent_summary(), shared]})),
    );
    let page = server
        .client
        .agents()
        .list_accessible(&options)
        .await
        .unwrap();
    assert!(matches!(page.items[0], AccessibleAgentSummary::Personal(_)));
    assert!(matches!(page.items[1], AccessibleAgentSummary::Shared(_)));
    server.expect(
        "GET",
        "/agents?organization=org",
        None,
        200,
        Some(json!({"items":[{"access":"future"}]})),
    );
    assert!(
        server
            .client
            .agents()
            .list_accessible(&options)
            .await
            .is_err()
    );
    server.expect(
        "GET",
        "/organizations",
        None,
        200,
        Some(json!({"items":[{"id":"org","role":"viewer"}]})),
    );
    assert_eq!(
        server
            .client
            .resource_access()
            .organizations()
            .await
            .unwrap()
            .items[0]
            .role,
        cma_sdk::resource_access::OrganizationRole::Viewer
    );
    server.expect(
        "GET",
        "/organizations",
        None,
        200,
        Some(json!({"items":[{"id":"org","role":"future"}]})),
    );
    assert!(
        server
            .client
            .resource_access()
            .organizations()
            .await
            .is_err()
    );
}

#[path = "rest/sandbox_commands.rs"]
mod sandbox_commands;

#[test]
fn live_skill_event_and_worker_step_parse_in_both_event_apis() {
    for status in ["pending", "installed", "failed"] {
        let event =
            json!({"type":"skill_installation_changed","skill_id":"research","status":status});
        assert!(serde_json::from_value::<AgentEventKind>(event.clone()).is_ok());
        assert!(serde_json::from_value::<cma_sdk::agent_events_v2::AgentEventKind>(event).is_ok());
    }
    assert!(
        serde_json::from_value::<AgentEventKind>(
            json!({"type":"work_step_completed","step":"install_skill"})
        )
        .is_ok()
    );
    assert!(
        serde_json::from_value::<cma_sdk::agent_events_v2::AgentEventKind>(
            json!({"type":"work_completed","step":"install_skill"})
        )
        .is_ok()
    );
    let invalid =
        json!({"type":"skill_installation_changed","skill_id":"research","status":"loaded"});
    assert!(serde_json::from_value::<AgentEventKind>(invalid.clone()).is_err());
    assert!(serde_json::from_value::<cma_sdk::agent_events_v2::AgentEventKind>(invalid).is_err());
}
