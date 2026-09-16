use super::super::super::fixture::{Fixture, VerifiedHuman};
use super::*;
use crate::sandbox::{
    channel_children::ChannelChildDriver, channel_provisioning::state::ChannelProvisioningPhase,
};
use layer2_core::resource_access::ResourceAccessService;
use layer2_core::sandbox::AgentOperationService;

pub(super) async fn start(mut f: Fixture, inbox: StoredInbox) -> ReplyFixture {
    let children = ChannelChildDriver {
        store: f.allocations.clone(),
        credentials: Arc::new(VerifiedHuman),
        adapter: f.adapter.clone(),
    };
    for _ in 0..20 {
        if f.record.state == AllocationState::Ready {
            break;
        }
        if let Some(snapshot) = f.step().await.unwrap() {
            f.record = if snapshot.state.phase == ChannelProvisioningPhase::AgentStart {
                f.startup_driver()
                    .advance(&f.authority, &f.lease, &snapshot)
                    .await
                    .unwrap()
            } else {
                children
                    .advance(&f.authority, &f.lease, &snapshot)
                    .await
                    .unwrap()
            };
        }
    }
    assert_eq!(f.record.state, AllocationState::Ready);
    let agent_id = f.record.placement.target().agent_id.clone();
    let registry = crate::sandbox::channel_startup::test_support::publish_connection(
        f.resources.clone(),
        f.adapter.clone(),
        &agent_id,
    )
    .await;
    let store = Arc::new(
        crate::store::MongoBridgeInvokeStore::open(
            f.bridges.clone(),
            f.resources.clone(),
            &f.database,
        )
        .await
        .unwrap(),
    );
    let allocations = Arc::new(
        MongoBridgeAllocationStore::open(f.bridges.clone(), Some(store.clone()), &f.database)
            .await
            .unwrap(),
    );
    let invokes = BridgeInvokeService::new(
        f.svc.clone(),
        Arc::new(ResourceAccessService::new(
            f.resources.clone(),
            Arc::new(Authority),
        )),
        f.resources.clone(),
        store.clone(),
    );
    let bridge = f
        .bridges
        .bridge(&inbox.pin.bridge_id)
        .await
        .unwrap()
        .unwrap();
    let source = NormalizedPartitionSource::from_stored(&inbox, &bridge).unwrap();
    let admission = AllocationAdmission::from_accepted(bridge.clone(), &source).unwrap();
    let (authority, _) = authority_for_admission(&f.svc, bridge, &admission, 3).await;
    let grant = invokes
        .issue_channel(f.lease.clone(), &f.record.id, &inbox.pin, authority)
        .await
        .unwrap();
    let channel = ChannelAgentCommandService::new(Arc::new(
        crate::store::MongoChannelCommandStore::new(store.clone()),
    ));
    let proof = invokes
        .authorize(BridgeInvokeRequest {
            grant_id: grant.id.clone(),
            pin: inbox.pin.clone(),
            target: f.record.placement.target().clone(),
            effect: BridgeInvokeEffect::AdmitMessage {
                inbox_id: inbox.id.clone(),
            },
        })
        .await
        .unwrap();
    let held = channel
        .admit(proof, inbox.clone(), f.record.clone())
        .await
        .unwrap();
    let authority = channel.authorize_execution(&invokes, &held).await.unwrap();
    channel
        .release_execution_hold(&authority, &held)
        .await
        .unwrap();
    let stamp = std::time::SystemTime::now();
    let claimed = f
        .resources
        .claim_next_command(CommandClaimRequest {
            agent_id: agent_id.clone(),
            holder: "wildcard-chat-command".into(),
            now: layer2_core::rfc3339_at(stamp),
            expires_at: layer2_core::rfc3339_at(stamp + Duration::from_secs(30)),
        })
        .await
        .unwrap()
        .unwrap();
    let authority = channel
        .authorize_execution(&invokes, &claimed)
        .await
        .unwrap();
    AgentOperationService::new(f.resources.clone(), registry.clone())
        .execute_channel_claim(&authority, &claimed)
        .await
        .unwrap();
    let command = f
        .resources
        .get_command(&claimed.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(command.status, AgentResponseStatus::InProgress);
    let credential = f.tools.fixture_credential(&agent_id).await;
    let replies = Arc::new(
        crate::store::MongoBridgeReplyToolStore::new(store, Arc::new(Authority))
            .await
            .unwrap(),
    );
    let router = crate::bridge::channel_tools::router(
        Arc::new(crate::bridge::channel_tools::StoredChannelTools {
            capabilities: f.tools.clone(),
            replies: replies.clone(),
            control_runtime: None,
        }),
        "https://cma.example",
    );
    let runtime = RuntimeHandle::new(
        &f.resources
            .get_sandbox(&f.record.placement.target().sandbox_id)
            .await
            .unwrap()
            .unwrap()
            .runtime
            .unwrap()
            .runtime_id,
    );
    ReplyFixture {
        client: f.client,
        database: f.database,
        resources: f.resources,
        invokes,
        grant,
        command,
        credential,
        router,
        backend: f.backend,
        runtime,
        registry,
        inbox,
        allocation: f.record,
        allocations,
        bridges: f.bridges,
        svc: f.svc,
        replies,
        tools: f.tools,
    }
}
