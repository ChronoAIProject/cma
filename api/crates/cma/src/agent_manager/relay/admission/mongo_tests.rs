//! Production Mongo reservation, two independently composed authenticated peers.
use super::*;
use crate::auth::{relay::tests::Fixture, test_resources};
use layer2_core::{
    agent_ownership::{candidates, Member, OwnershipCoordinator, OwnershipStore, PROTOCOL_VERSION},
    resource_access::ResourceAccessService,
    sandbox::{
        store::{WorkspaceStore, WorktreeStore},
        DurableAgentUpdateRecorder,
    },
};
use std::sync::Mutex;

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn real_mongo_peer_admission_commits_once_and_refuses_revoked_owner() {
    use crate::store::sandbox_maintenance::mongo_integration_tests::{database, fixture};
    let (client, database_name, raw) = database().await;
    let (parent, agent) = fixture(&raw, "m1-peer-admission").await;
    raw.create_workspace(test_resources::workspace(
        agent.workspace_id.as_str(),
        parent.id.as_str(),
        agent.owner.as_str(),
    ))
    .await
    .unwrap();
    raw.create_worktree(test_resources::worktree(
        agent.worktree_id.as_str(),
        agent.workspace_id.as_str(),
        parent.id.as_str(),
        agent.id.as_str(),
        agent.owner.as_str(),
    ))
    .await
    .unwrap();
    let auth = Fixture::new(true).await;
    let mut peers = Vec::new();
    let mut servers = Vec::new();
    for index in 0..2 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let instance = format!("mongo-admission-{index}");
        let store = Arc::new(
            raw.clone()
                .with_agent_control_write_coordination(true)
                .enable_agent_ownership(instance.clone())
                .await
                .unwrap(),
        );
        let ownership = store.agent_ownership.as_ref().unwrap().clone();
        let coordinator = OwnershipCoordinator::register(
            ownership.clone(),
            Member {
                instance_id: instance.clone(),
                protocol: PROTOCOL_VERSION,
                relay_address: format!("http://{}", listener.local_addr().unwrap()),
                expires_at_ms: 0,
                draining: false,
                owned_agents: 0,
                max_owned_agents: 16,
            },
        )
        .await
        .unwrap();
        let observed = Arc::new(super::tests::fixture::ObservedStore {
            inner: store.clone(),
            instance,
            calls: Mutex::default(),
            lookups: std::sync::atomic::AtomicUsize::new(0),
        });
        let service = AgentCommandService::new(
            observed.clone(),
            Arc::new(ResourceAccessService::new(store.clone(), auth.auth.clone())),
            auth.auth.clone(),
        );
        let router = AdmissionRouter::new("public", auth.auth.clone());
        let service = router.compose(service);
        let updates = DurableAgentUpdateRecorder::new(store.clone());
        let relay = Relay::new(
            coordinator.clone(),
            &updates,
            ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"mongo-admission-fixture"),
        )
        .unwrap();
        relay.install_admissions(vec![router]).unwrap();
        let app = relay.routes(axum::Router::new());
        let app = crate::with_request_context(app);
        servers.push(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }));
        peers.push((coordinator, service, observed, ownership, updates, relay));
    }
    let membership = peers[0].3.membership().await.unwrap();
    let selected = candidates(&agent.id, &membership, None).unwrap()[0]
        .instance_id
        .clone();
    let owner = peers
        .iter()
        .position(|peer| peer.0.instance_id == selected)
        .unwrap();
    let authority = peers[owner].0.acquire(&agent.id).await.unwrap().unwrap();
    let evidence = Origin::capture(
        &axum::http::Method::POST,
        &"/api/v2/responses".parse().unwrap(),
        &auth.headers,
    )
    .unwrap()
    .evidence()
    .unwrap();
    let submission = CommandSubmission {
        agent_id: agent.id.clone(),
        command: AgentCommand::message("one durable owner admission"),
        idempotency_key: "mongo-original-key".into(),
        expected_agent_revision: None,
        requested_model: None,
    };
    let mut receipts = Vec::new();
    for ingress in [1 - owner, owner] {
        receipts.push(
            evidence
                .clone()
                .scope(peers[ingress].1.admit(
                    &auth.context,
                    "test-access",
                    CommandSubmissionSource::Api,
                    submission.clone(),
                ))
                .await
                .unwrap()
                .receipt,
        );
    }
    assert_eq!(receipts[0], receipts[1]);
    let retained = peers[owner]
        .2
        .inner
        .get_command(&receipts[0].command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.receipt.envelope.agent_id, agent.id);
    assert_eq!(retained.receipt.envelope.command, submission.command);
    assert_eq!(retained.receipt.agent_order, 1);
    assert!(peers[1 - owner].2.calls.lock().unwrap().is_empty());
    assert!(peers[owner]
        .2
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|token| token == &authority.token));
    peers[owner].0.fail().await;
    let mut denied = submission;
    denied.idempotency_key = "closed-owner-key".into();
    assert!(evidence
        .scope(peers[1 - owner].1.admit(
            &auth.context,
            "test-access",
            CommandSubmissionSource::Api,
            denied
        ))
        .await
        .is_err());
    assert!(peers[1 - owner].2.calls.lock().unwrap().is_empty());
    for server in servers {
        server.abort();
        let _ = server.await;
    }
    drop(peers);
    client.database(&database_name).drop().await.unwrap();
}
