use super::super::tests::fixtures::Fixture;
use super::*;
use layer2_core::resource_access::ResourceAccessStore;
use serde_json::json;
mod feature_gates_tests;
mod management;

/// Retained historical outbox fixture, not a production admission proof. New
/// Channel-created admission remains unavailable until the real creator exists.
pub(super) fn retained_item(f: &Fixture, reference: &str) -> OutboxItem {
    let config = &f.bridge.current().unwrap().config;
    let source:OutboxSource=serde_json::from_value(json!({
        "inbox_id":"1".repeat(64),"inbox_sha256":"2".repeat(64),"allocation_id":"allocation",
        "command_id":format!("command-{reference}"),"response_id":format!("response-{reference}"),"admission_sha256":"3".repeat(64),
        "owner":f.bridge.owner,"pin":f.bridge.admission_pin().unwrap(),"config":config,"target":f.target,
        "message_id":"original-inbound","chat_id":"original-chat","source_sha256":"4".repeat(64),
        "reply_expires_at_ms":null,"route_expires_at_ms":null,
    })).unwrap();
    let invocation = format!(
        "brti_{}",
        layer2_core::profiles::content_digest(&reference).unwrap()
    );
    let operation = RelayOperation::InitialReply;
    let id = format!(
        "bro_{}",
        layer2_core::profiles::content_digest(&(
            "cma.bridge.outbox.v1",
            source.fingerprint().unwrap(),
            &invocation,
            1_u64,
            0_u32,
            &operation,
        ))
        .unwrap()
    );
    let now = now_ms() - 1000;
    let item:OutboxItem=serde_json::from_value(json!({
        "id":id,"source":source,"tool_invocation_id":invocation,"turn_sequence":1,"sequence":1,"chunk_index":0,"chunk_count":1,"last_chunk":true,
        "predecessor":null,"content_sha256":layer2_core::profiles::content_digest(&"Progress update").unwrap(),"text":"Progress update",
        "credential":{"kind":"retained_key","reference":config.connector.custody,"revision":1,"key_state_version":3},
        "operation":operation,"revision":1,"state":{"kind":"pending"},"attempts":[],"duplicate_risk":false,
        "created_at_ms":now,"updated_at_ms":now,
    })).unwrap();
    item.validate().unwrap();
    item
}
pub(super) async fn enabled_fixture() -> Fixture {
    let mut fixture = Fixture::new().await;
    fixture
        .service
        .issue(
            &fixture.context,
            None,
            fixture.issue("outbox-retained-consent"),
        )
        .await
        .unwrap();
    fixture.enable("outbox-retained-enable").await;
    fixture
}
pub(super) async fn store(f: &Fixture) -> MongoBridgeReplyToolStore {
    MongoBridgeReplyToolStore::new(f.store.clone(), f.authority.clone())
        .await
        .unwrap()
}
pub(super) async fn seed(store: &MongoBridgeReplyToolStore, item: &OutboxItem) {
    store
        .items
        .insert_one(records::item_document(item).unwrap())
        .await
        .unwrap();
    let invocation = records::Invocation {
        id: item.tool_invocation_id.clone(),
        namespace: "5".repeat(64),
        request_sha256: layer2_core::profiles::content_digest(&"Progress update").unwrap(),
        source_sha256: item.source.fingerprint().unwrap(),
        items: vec![item.id.clone()],
        created_at_ms: item.created_at_ms,
    };
    store
        .invocations
        .insert_one(invocation.document().unwrap())
        .await
        .unwrap();
}
pub(super) async fn retained(store: &MongoBridgeReplyToolStore, id: &str) -> OutboxItem {
    records::read_item(
        &store
            .items
            .find_one(doc! {"_id":id})
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn concurrent_claims_and_restart_retain_one_attempt_before_any_delivery() {
    let f = enabled_fixture().await;
    let store = store(&f).await;
    let item = retained_item(&f, "concurrent");
    seed(&store, &item).await;
    let (a, b) = tokio::join!(
        store.claim_item(&item.id, "worker-a"),
        store.claim_item(&item.id, "worker-b")
    );
    assert_eq!([a.unwrap(), b.unwrap()].into_iter().flatten().count(), 1);
    let reopened = MongoBridgeReplyToolStore::new(f.store.clone(), f.authority.clone())
        .await
        .unwrap();
    let current = retained(&reopened, &item.id).await;
    assert!(matches!(current.state, DeliveryState::Claimed { .. }));
    assert_eq!(current.attempts.len(), 1);
    assert_eq!(reopened.audits.count_documents(doc! {}).await.unwrap(), 1);
    f.client.database(&f.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn expired_claim_recovers_but_expired_sending_is_permanently_unknown() {
    let f = enabled_fixture().await;
    let store = store(&f).await;
    let mut before = retained_item(&f, "before");
    let mut after = retained_item(&f, "after");
    let time = now_ms() - 500;
    before
        .claim("worker".into(), time, time + 100, None)
        .unwrap();
    let attempt = after
        .claim("worker".into(), time, time + 100, None)
        .unwrap();
    after.begin_send("worker", &attempt, time + 1).unwrap();
    seed(&store, &before).await;
    seed(&store, &after).await;
    assert!(store
        .claim_item(&before.id, "recover")
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        retained(&store, &before.id).await.state,
        DeliveryState::Pending
    );
    assert!(store
        .claim_item(&after.id, "recover")
        .await
        .unwrap()
        .is_none());
    let unknown = retained(&store, &after.id).await;
    assert!(matches!(
        unknown.state,
        DeliveryState::Unknown {
            reason: DeliveryFailure::WorkerLost,
            ..
        }
    ));
    assert!(store
        .claim_item(&after.id, "retry")
        .await
        .unwrap()
        .is_none());
    assert_eq!(retained(&store, &after.id).await.attempts.len(), 1);
    assert!(store
        .scan("scan")
        .await
        .unwrap()
        .iter()
        .all(|doc| doc.get_str("_id").unwrap() != after.id));
    f.client.database(&f.database).drop().await.unwrap();
}

struct Delivered(std::sync::atomic::AtomicUsize);
#[async_trait::async_trait]
impl RelayDeliveryPort for Delivered {
    async fn deliver(&self, request: RelayDeliveryRequest) -> DeliveryOutcome {
        assert!(matches!(
            request.item().state,
            DeliveryState::Sending { .. }
        ));
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        DeliveryOutcome::Confirmed(DeliveryReceipt {
            nyxid_message_id: "new-outbound".into(),
            platform_message_id: Some("platform-reply".into()),
        })
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn delivery_commits_sending_then_preserves_receipt_without_resending_after_restart() {
    let f = enabled_fixture().await;
    let store = store(&f).await;
    let item = retained_item(&f, "sent");
    seed(&store, &item).await;
    let port = Delivered(std::sync::atomic::AtomicUsize::new(0));
    assert!(store.deliver_one("worker", &port).await.unwrap());
    let reopened = MongoBridgeReplyToolStore::new(f.store.clone(), f.authority.clone())
        .await
        .unwrap();
    assert!(!reopened.deliver_one("worker", &port).await.unwrap());
    assert_eq!(port.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    let current = retained(&reopened, &item.id).await;
    assert!(matches!(current.state, DeliveryState::Sent { .. }));
    assert_eq!(reopened.audits.count_documents(doc! {}).await.unwrap(), 3);
    let mut session = f.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let invocation = records::Invocation::read(
        &reopened
            .invocations
            .find_one(doc! {"_id":&item.tool_invocation_id})
            .session(&mut session)
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let receipt = reopened.receipt(&invocation, &mut session).await.unwrap();
    assert_eq!(receipt.status, ReplyToolStatus::Sent);
    assert_eq!(
        invocation.require_request(&"5".repeat(64), &"9".repeat(64)),
        Err(OutboxError::RequestConflict)
    );
    session.abort_transaction().await.unwrap();
    f.client.database(&f.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn durable_scan_cursor_reaches_beyond_first_page_and_delays_blocked_items() {
    let f = enabled_fixture().await;
    let store = store(&f).await;
    let mut items = Vec::new();
    for index in 0..103 {
        items.push(retained_item(&f, &format!("page-{index}")));
    }
    items.sort_by(|a, b| a.id.cmp(&b.id));
    store
        .items
        .insert_many(
            items
                .iter()
                .map(records::item_document)
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
        )
        .await
        .unwrap();
    let first = store.scan("worker").await.unwrap();
    assert_eq!(first.len(), 100);
    store
        .advance_scan("worker", first.last().unwrap().get_str("_id").unwrap())
        .await
        .unwrap();
    let reopened = MongoBridgeReplyToolStore::new(f.store.clone(), f.authority.clone())
        .await
        .unwrap();
    assert_eq!(reopened.scan("worker").await.unwrap().len(), 3);
    reopened
        .advance_scan("worker", &items[102].id)
        .await
        .unwrap();
    reopened.delay_candidate(&items[0], 30_000).await.unwrap();
    let wrapped = reopened.scan("worker").await.unwrap();
    assert_eq!(wrapped.len(), 100);
    assert!(wrapped
        .iter()
        .all(|doc| doc.get_str("_id").unwrap() != items[0].id));
    f.client.database(&f.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn authority_outage_is_durably_deferred_without_sending() {
    let f = enabled_fixture().await;
    let store = store(&f).await;
    let item = retained_item(&f, "outage");
    seed(&store, &item).await;
    *f.authority.snapshot.lock().unwrap() = None;
    let port = Delivered(std::sync::atomic::AtomicUsize::new(0));
    assert!(store.deliver_one("worker", &port).await.unwrap());
    let current = retained(&store, &item.id).await;
    assert!(matches!(
        current.state,
        DeliveryState::RetryWait {
            reason: DeliveryFailure::ConnectUnavailable,
            ..
        }
    ));
    assert_eq!(port.0.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(store.scan("worker").await.unwrap().is_empty());
    f.client.database(&f.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn corrupt_earlier_item_cannot_starve_unrelated_valid_response() {
    let f = enabled_fixture().await;
    let store = store(&f).await;
    let mut items = [
        retained_item(&f, "corrupt-row-a"),
        retained_item(&f, "corrupt-row-b"),
    ];
    items.sort_by(|a, b| a.id.cmp(&b.id));
    for item in &items {
        seed(&store, item).await;
    }
    store
        .items
        .update_one(
            doc! {"_id":&items[0].id},
            doc! {"$set":{"record.text":"tampered text"}},
        )
        .await
        .unwrap();
    let port = Delivered(std::sync::atomic::AtomicUsize::new(0));
    assert_eq!(
        store.deliver_one("worker", &port).await,
        Err(OutboxError::Corrupt)
    );
    assert!(store.deliver_one("worker", &port).await.unwrap());
    assert_eq!(port.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(matches!(
        retained(&store, &items[1].id).await.state,
        DeliveryState::Sent { .. }
    ));
    f.client.database(&f.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn manual_trigger_and_legacy_agents_bound_to_bridge_never_receive_reply_capability() {
    use base64::Engine;
    use layer2_core::sandbox::{AgentCreationOrigin, AgentCreationProvenance, AgentOrigin};
    use layer3_adapter::hosted::HostedChannelToolSource;
    let f = enabled_fixture().await;
    let key = crate::store::AuthEncryptionKey::from_base64(
        &base64::engine::general_purpose::STANDARD.encode([11_u8; 32]),
    )
    .unwrap();
    let tools = crate::store::MongoChannelToolStore::open(
        f.resources.clone(),
        "fixture-deployment",
        &key,
        "https://cma.example",
    )
    .await
    .unwrap();
    let target = layer2_core::resource_access::ResourceTarget::Agent {
        agent_id: f.target.agent_id.clone(),
    };
    let snapshot = f.resources.resource_access_snapshot(&target).await.unwrap();
    let sandbox = &snapshot.lineage.sandbox;
    let mut agent = snapshot.lineage.agent.as_ref().unwrap().clone();
    for origin in [
        AgentCreationOrigin::Manual,
        AgentCreationOrigin::Trigger {
            trigger_id: "trigger".into(),
            launch_id: "launch".into(),
            launching_actor: None,
        },
        AgentCreationOrigin::LegacyUnknown,
    ] {
        agent.origin = match &origin {
            AgentCreationOrigin::Trigger {
                trigger_id,
                launch_id,
                ..
            } => AgentOrigin::Trigger {
                trigger_id: trigger_id.clone(),
                launch_id: launch_id.clone(),
            },
            _ => AgentOrigin::Manual,
        };
        f.resources
            .agents
            .replace_one(
                doc! {"_id":agent.id.as_str()},
                crate::store::agents::agent_document(&agent).unwrap(),
            )
            .await
            .unwrap();
        let provenance = AgentCreationProvenance {
            schema_version: 1,
            agent_id: agent.id.clone(),
            agent_created_at: agent.created_at.clone(),
            sandbox_id: sandbox.id.clone(),
            sandbox_created_at: sandbox.created_at.clone(),
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
            owner: f.bridge.owner.clone(),
            creation_origin: origin,
            created_by: None,
            provisioning_origin: agent.origin.clone(),
            creation_fingerprint: agent.creation_fingerprint.clone(),
        };
        provenance.validate_against(&agent, sandbox).unwrap();
        f.resources
            .agent_creation_provenance
            .replace_one(
                doc! {"_id":agent.id.as_str()},
                crate::store::agent_provenance::document(&provenance).unwrap(),
            )
            .upsert(true)
            .await
            .unwrap();
        let request = layer3_adapter::AgentOpenRequest::new(
            format!("agent:{}:open:v1", agent.id),
            crate::sandbox::runtime_options(
                &agent,
                sandbox,
                snapshot.lineage.worktree.as_ref().unwrap(),
            ),
            None,
        );
        assert!(tools
            .resolve(
                &layer3_adapter::RuntimeHandle("fixture-runtime".into()),
                &request,
                1
            )
            .await
            .unwrap()
            .is_none());
    }
    assert_eq!(
        f.client
            .database(&f.database)
            .collection::<Document>("bridge_channel_capabilities")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    let store = store(&f).await;
    assert_eq!(store.items.count_documents(doc! {}).await.unwrap(), 0);
    f.client.database(&f.database).drop().await.unwrap();
}
