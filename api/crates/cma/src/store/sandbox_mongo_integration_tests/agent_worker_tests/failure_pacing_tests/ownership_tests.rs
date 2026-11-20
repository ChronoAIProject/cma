//! Independent clients preserve pacing, schema adoption and owned pagination.
use super::*;
use crate::store::agent_log_segments::{schema, writer_gate};
use layer2_core::agent_ownership::{
    Member, OwnershipCoordinator, OwnershipStore, DEFAULT_MAX_OWNED_AGENTS, PROTOCOL_VERSION,
};
use layer2_core::sandbox::store::AgentLogPublicationIdentity;

fn member(id: &str) -> Member {
    Member {
        instance_id: id.into(),
        protocol: PROTOCOL_VERSION,
        relay_address: format!("http://{id}:8080"),
        expires_at_ms: 0,
        draining: false,
        owned_agents: 0,
        max_owned_agents: DEFAULT_MAX_OWNED_AGENTS,
    }
}

async fn independent_owner(name: &str, id: &str) -> (MongoSandboxStore, Arc<OwnershipCoordinator>) {
    let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let store = MongoSandboxStore::open(&client, name)
        .await
        .unwrap()
        .enable_agent_ownership(id.into())
        .await
        .unwrap();
    let owner =
        OwnershipCoordinator::register(store.agent_ownership.as_ref().unwrap().clone(), member(id))
            .await
            .unwrap();
    (store, owner)
}

async fn claim(store: &MongoSandboxStore, owner: &OwnershipCoordinator, id: &AgentId, at: u64) {
    owner
        .acquire(id)
        .await
        .unwrap()
        .unwrap()
        .run(store.claim_agent_work(id, "owned-worker", stamp(at), stamp(at + 600)))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn owned_discovery_filters_held_work_before_limit_and_retains_fallback() {
    let (client, name) = database("owned_pacing_limit").await;
    let off = MongoSandboxStore::open(&client, &name).await.unwrap();
    let held = ready_conversation_fixture(&off, "a_held").await;
    let observed = observation(&off, &held).await;
    for at in [0, 30, 90, 210, 450] {
        fail(&off, &held.id, at, Some(observed.clone()), CLASS).await;
    }
    let fresh = ready_conversation_fixture(&off, "z_fresh").await;
    let (on, owner) = independent_owner(&name, "owner").await;
    assert!(held.id < fresh.id);
    for store in [&off, &on] {
        assert_eq!(
            store.due_agent_work(&stamp(451), 1).await.unwrap(),
            vec![fresh.id.clone()]
        );
    }
    claim(&on, &owner, &fresh.id, 451).await;
    for store in [&off, &on] {
        assert!(store
            .due_agent_work(&stamp(749), 1)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            store.due_agent_work(&stamp(750), 1).await.unwrap(),
            vec![held.id.clone()]
        );
    }
    claim(&on, &owner, &held.id, 750).await;
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn owned_discovery_wakes_for_each_parent_before_base_due() {
    let (client, name) = database("owned_pacing_wake").await;
    let off = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut agents = Vec::new();
    for parent in ["sandbox", "workspace", "worktree"] {
        let agent = ready_conversation_fixture(&off, parent).await;
        let observed = observation(&off, &agent).await;
        fail(&off, &agent.id, 0, Some(observed.clone()), CLASS).await;
        fail(&off, &agent.id, 30, Some(observed), CLASS).await;
        agents.push((parent, agent));
    }
    let (on, owner) = independent_owner(&name, "owner").await;
    for (parent, agent) in agents {
        for store in [&off, &on] {
            assert!(!store
                .due_agent_work(&stamp(31), 200)
                .await
                .unwrap()
                .contains(&agent.id));
        }
        let (collection, id) = match parent {
            "sandbox" => (&off.sandboxes, agent.sandbox_id.as_str()),
            "workspace" => (&off.workspaces, agent.workspace_id.as_str()),
            _ => (&off.worktrees, agent.worktree_id.as_str()),
        };
        collection
            .update_one(
                doc! {"_id": id},
                doc! {"$inc": {"revision": 1_i64, "record.revision": 1_i64}},
            )
            .await
            .unwrap();
        for store in [&off, &on] {
            assert!(store
                .due_agent_work(&stamp(31), 200)
                .await
                .unwrap()
                .contains(&agent.id));
        }
        claim(&on, &owner, &agent.id, 31).await;
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn owned_discovery_claims_a_real_segmented_agent_after_restart() {
    let (client, name) = database("owned_segmented_work").await;
    let off = MongoSandboxStore::open(&client, &name)
        .await
        .unwrap()
        .with_segment_adoption_enabled(true);
    // Use durable admission so adoption validates a real creation prefix. The
    // older ready-work fixture intentionally starts at ConversationOpened.
    let parent = sandbox("sbx_segmented_discovery");
    off.create_sandbox(parent.clone()).await.unwrap();
    let workspace = ready_workspace(&parent, "wks_segmented_discovery", "segmented");
    off.create_workspace(workspace.clone()).await.unwrap();
    let agent = created_agent(
        off.admit_agent(agent_admission(&workspace, "segmented", "segmented"))
            .await
            .unwrap(),
    )
    .agent;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    crate::store::migrations::agent_log_source_keys::prepare_for_test(&db).await;
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    writer_gate::replace(
        &db,
        writer_gate::GateName::Segments,
        0,
        writer_gate::Policy {
            enabled: true,
            agents: vec![agent.id.to_string()],
            ..Default::default()
        },
        &mut session,
    )
    .await
    .unwrap();
    session.commit_transaction().await.unwrap();
    off.append_agent_log_keyed(
        &agent.id,
        &AgentLogPublicationIdentity {
            key: "owned-work-adoption".into(),
            fingerprint: [7; 32],
            appended_at: NOW.into(),
        },
        AgentLogKind::AvailableCommandsSet { commands: vec![] },
    )
    .await
    .unwrap();
    assert_eq!(
        off.agents
            .find_one(doc! {"_id": agent.id.as_str()})
            .await
            .unwrap()
            .unwrap()
            .get_i64("document_schema_version")
            .unwrap(),
        2
    );
    let (on, owner) = independent_owner(&name, "owner").await;
    for store in [&off, &on] {
        assert_eq!(
            store.due_agent_work(NOW, 1).await.unwrap(),
            vec![agent.id.clone()]
        );
    }
    claim(&on, &owner, &agent.id, 0).await;
    assert!(on.due_agent_work(NOW, 1).await.unwrap().is_empty());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn owned_discovery_pages_past_a_full_peer_page_without_starving_local_work() {
    let (client, name) = database("owned_discovery_pages").await;
    let off = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut peers = Vec::new();
    for index in 0..101 {
        peers.push(ready_conversation_fixture(&off, &format!("a_peer_{index:03}")).await);
    }
    let local = ready_conversation_fixture(&off, "z_local").await;
    let (peer_store, _) = independent_owner(&name, "peer").await;
    let ownership = peer_store.agent_ownership.as_ref().unwrap();
    for agent in &peers {
        assert!(ownership
            .acquire(&agent.id, "peer")
            .await
            .unwrap()
            .is_some());
    }
    // Drain retains every live peer lease but lets the second incarnation be
    // the rendezvous candidate for the remaining, unowned Agent.
    ownership.heartbeat("peer", true).await.unwrap();
    let (on, owner) = independent_owner(&name, "owner").await;
    assert_eq!(
        on.due_agent_work(NOW, 1).await.unwrap(),
        vec![local.id.clone()]
    );
    claim(&on, &owner, &local.id, 0).await;
    assert!(on.due_agent_work(NOW, 1).await.unwrap().is_empty());
    client.database(&name).drop().await.unwrap();
}
