//! Management and late DrainOriginal inputs use actual service authority,
//! genuine signed inbox records, and the same durable Mongo transaction.
use super::*;
use layer2_core::sandbox::{
    channel_usage::{
        AgentChannelUsageState, ChannelBindingChangeReason, ChannelUsageChange, ChannelUsageEvent,
    },
    store::{AgentChannelUsageStore, PageRequest, WorktreeStore},
};

async fn history(c: &ChannelFixture) -> (AgentChannelUsageState, Vec<ChannelUsageEvent>) {
    let (snapshot, page) = c
        .f
        .resources
        .agent_channel_usage_history(&c.f.target.agent_id, &PageRequest::new(100, None).unwrap())
        .await
        .unwrap();
    (snapshot.state, page.items)
}
async fn allocation_document(c: &ChannelFixture) -> Document {
    c.f.client
        .database(&c.f.database)
        .collection::<Document>("bridge_allocations")
        .find_one(doc! {"_id":&c.allocation.id})
        .await
        .unwrap()
        .unwrap()
}
fn retire(c: &ChannelFixture, policy: AcceptedWorkPolicy) -> BridgeMutation {
    BridgeMutation {
        id: c.f.bridge.id.clone(),
        expected_revision: c.f.bridge.revision,
        idempotency_key: "usage-retire".into(),
        change: BridgeChange::Retire {
            accepted_work: policy,
        },
    }
}
async fn mutate(
    c: &ChannelFixture,
    mutation: BridgeMutation,
) -> Result<BridgeReceipt, BridgeError> {
    c.f.definitions
        .mutate(&c.f.context, Some("fixture-credential"), mutation)
        .await
}
async fn fail_events(c: &ChannelFixture, enabled: bool) {
    c.f.client.database(&c.f.database).run_command(doc! {
        "collMod":"channel_usage_events", "validator":if enabled {doc! {"forced_failure":true}} else {doc! {}},
        "validationLevel":"strict", "validationAction":"error"
    }).await.unwrap();
}
async fn second_agent(c: &ChannelFixture) -> BridgeAgentTarget {
    let mut agent =
        c.f.resources
            .get_agent(&c.f.target.agent_id)
            .await
            .unwrap()
            .unwrap();
    let mut worktree =
        c.f.resources
            .get_worktree(&agent.worktree_id)
            .await
            .unwrap()
            .unwrap();
    agent.id = layer2_core::AgentId("agt_usage_sibling".into());
    agent.worktree_id = layer2_core::WorktreeId("wkt_usage_sibling".into());
    agent.creation_key = "create-usage-sibling".into();
    agent.conversation.last_seq = 0;
    agent.conversation.conversation_id = Some("conversation-usage-sibling".into());
    worktree.id = agent.worktree_id.clone();
    worktree.name = "usage-sibling".into();
    worktree.path = "/workspace/usage-sibling".into();
    worktree.branch = "usage-sibling".into();
    worktree.agent_id = Some(agent.id.clone());
    c.f.resources.create_worktree(worktree).await.unwrap();
    let target = BridgeAgentTarget {
        agent_id: agent.id.clone(),
        sandbox_id: agent.sandbox_id.clone(),
        workspace_id: agent.workspace_id.clone(),
        worktree_id: agent.worktree_id.clone(),
    };
    c.f.resources.create_agent(agent).await.unwrap();
    target
}
async fn rebind(c: &ChannelFixture, target: BridgeAgentTarget) -> BridgeMutation {
    let snapshot =
        c.f.resources
            .resource_access_snapshot(&ResourceTarget::Agent {
                agent_id: target.agent_id.clone(),
            })
            .await
            .unwrap();
    *c.f.authority.snapshot.lock().unwrap() = Some(snapshot);
    let mut config = c.f.bridge.current().unwrap().config.clone();
    config.placement = BridgePlacementPolicy::ExistingAgent { target };
    BridgeMutation {
        id: c.f.bridge.id.clone(),
        expected_revision: c.f.bridge.revision,
        idempotency_key: "usage-rebind".into(),
        change: BridgeChange::Update {
            config: Box::new(config),
            previous_work: Some(AcceptedWorkPolicy::DrainOriginal),
        },
    }
}

mod rebinding;
mod retirement;
mod shared_agent;

async fn accept_next_inbox(c: &mut ChannelFixture, number: i64) {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use layer2_core::bridge::ingress::CallbackInboxStore;
    let validation =
        c.f.authority
            .validate_connection(&c.f.bridge.owner, &c.f.bridge.current().unwrap().config)
            .await
            .unwrap();
    let write = fixtures::signed_inbox_named(
        &c.f.bridge,
        validation,
        &format!("usage-external-{number}"),
        &format!("usage-callback-{number}"),
        number,
    )
    .await;
    c.inbox = write.record().clone();
    let key = crate::store::AuthEncryptionKey::from_base64(&STANDARD.encode([7_u8; 32])).unwrap();
    crate::store::MongoCallbackInbox::open(&c.f.bridges, &c.f.database, "fixture-deployment", &key)
        .await
        .unwrap()
        .accept(write)
        .await
        .unwrap();
}

async fn select_connection_target(c: &ChannelFixture, id: &layer2_core::AgentId) {
    let snapshot =
        c.f.resources
            .resource_access_snapshot(&ResourceTarget::Agent {
                agent_id: id.clone(),
            })
            .await
            .unwrap();
    *c.f.authority.snapshot.lock().unwrap() = Some(snapshot);
}
