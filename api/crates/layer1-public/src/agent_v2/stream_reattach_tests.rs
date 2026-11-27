use super::*;
use crate::agent_v2::fixtures::{append, context, fixture, Fixture};
use futures_util::StreamExt;
use layer2_core::{
    agent_ownership::{
        test_support::MemoryOwnershipStore, Authority, Member, OwnershipCoordinator,
        OwnershipStore, PROTOCOL_VERSION,
    },
    sandbox::{
        store::AgentStore, AgentLiveFragment, AgentLiveFragmentKind, AgentLivePublisher,
        AgentLogKind,
    },
};
use std::sync::Arc;
use tokio::time::{timeout, Instant};

struct RelayFixture {
    publisher: AgentLivePublisher,
    store: Arc<MemoryOwnershipStore>,
    coordinator: Arc<OwnershipCoordinator>,
    authority: Arc<Authority>,
}

impl RelayFixture {
    async fn bind(open: &mut Following) -> Self {
        let store = Arc::new(MemoryOwnershipStore::default());
        let coordinator = OwnershipCoordinator::register(
            store.clone(),
            Member {
                instance_id: "relay-fixture".into(),
                protocol: PROTOCOL_VERSION,
                relay_address: "http://fixture.invalid".into(),
                expires_at_ms: 0,
                draining: false,
                owned_agents: 0,
                max_owned_agents: 1,
            },
        )
        .await
        .unwrap();
        let lease = store
            .acquire(&open.header.id, &coordinator.instance_id)
            .await
            .unwrap()
            .unwrap();
        let authority = Authority::new(lease.token, Instant::now() + Duration::from_secs(10));
        let (publisher, subscription) = AgentLiveSubscription::channel();
        publisher.attached(authority.clone(), coordinator.clone());
        open.watching = Some(subscription);
        Self {
            publisher,
            store,
            coordinator,
            authority,
        }
    }

    async fn replace_owner(&self) {
        self.authority.revoke();
        self.publisher.interrupted(
            self.authority.token.clone(),
            Instant::now() + Duration::from_secs(3),
        );
        self.store.release(&self.authority.token).await.unwrap();
        let lease = self
            .store
            .acquire(
                &self.authority.token.agent_id,
                &self.coordinator.instance_id,
            )
            .await
            .unwrap()
            .unwrap();
        self.publisher.attached(
            Authority::new(lease.token, Instant::now() + Duration::from_secs(10)),
            self.coordinator.clone(),
        );
    }
}

async fn open_stream(
    subject: &str,
) -> (
    Following,
    Fixture,
    RelayFixture,
    Option<layer2_core::resource_access::ResourceShare>,
) {
    let fixture = fixture(false, false).await;
    let id = AgentId("agt_fixture".into());
    let mut agent = fixture.store.get_agent(&id).await.unwrap().unwrap();
    let revision = agent.revision;
    agent.conversation.active_turn = None;
    agent.status = layer2_core::sandbox::AgentStatus::Ready;
    agent.revision += 1;
    fixture.store.save_agent(revision, agent).await.unwrap();
    let share = if subject == "reader" {
        Some(super::tests::share(&fixture).await)
    } else {
        None
    };
    let auth = AuthenticatedResourceCaller {
        context: context(subject),
        credential: (subject == "reader")
            .then(|| crate::DelegatedCredential::new("fixture-credential".into())),
    };
    let mut open = super::open(fixture.state.clone(), auth, id, None, HeaderMap::new())
        .await
        .unwrap();
    let relay = RelayFixture::bind(&mut open).await;
    (open, fixture, relay, share)
}

async fn start_turn(f: &Fixture, id: &AgentId) -> u64 {
    let mut agent = f.store.get_agent(id).await.unwrap().unwrap();
    let sequence = agent.conversation.last_seq + 1;
    append(
        &f.store,
        AgentLogKind::TurnStarted {
            turn_sequence: sequence,
            model: None,
            origin_key: None,
        },
    )
    .await;
    agent = f.store.get_agent(id).await.unwrap().unwrap();
    let revision = agent.revision;
    agent.conversation.active_turn = Some(sequence);
    agent.status = layer2_core::sandbox::AgentStatus::Working;
    agent.revision += 1;
    f.store.save_agent(revision, agent).await.unwrap();
    sequence
}

async fn finish_turn(f: &Fixture, id: &AgentId, sequence: u64) {
    append(
        &f.store,
        AgentLogKind::TurnCompleted {
            turn_sequence: sequence,
        },
    )
    .await;
    let mut agent = f.store.get_agent(id).await.unwrap().unwrap();
    let revision = agent.revision;
    agent.conversation.active_turn = None;
    agent.status = layer2_core::sandbox::AgentStatus::Ready;
    agent.revision += 1;
    f.store.save_agent(revision, agent).await.unwrap();
}

async fn next(body: &mut axum::body::BodyDataStream) -> String {
    let bytes = timeout(Duration::from_secs(3), body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn catch_up(body: &mut axum::body::BodyDataStream) -> usize {
    let mut count = 0;
    loop {
        let wire = next(body).await;
        if wire.contains("\"reason\":\"reconnected\"") {
            return count;
        }
        assert!(wire.contains("event: cma.agent.event\n"), "{wire}");
        count += 1;
    }
}

#[tokio::test]
async fn idle_reattach_keeps_queued_durable_prefix_and_initial_marker_once() {
    let (mut open, fixture, relay, _) = open_stream("owner-a").await;
    let queued = open.pending.len();
    assert!(queued > 0);
    let mut body = {
        // Bind the drained baseline while keeping the original durable queue
        // under backpressure. Its cursor already points past the queued body.
        open.queue_live(false);
        let mut body = response(open).into_body().into_data_stream();
        let first = next(&mut body).await;
        assert!(first.contains("event: cma.agent.event\n"));
        relay.replace_owner().await;
        assert_eq!(catch_up(&mut body).await + 1, queued);
        body
    };
    assert!(timeout(Duration::from_millis(250), body.next())
        .await
        .is_err());
    append(
        &fixture.store,
        AgentLogKind::OwnerMessage {
            content: "after recovery".into(),
        },
    )
    .await;
    assert!(next(&mut body).await.contains("after recovery"));
}

#[tokio::test]
async fn a_turn_start_between_binding_and_the_authorized_head_read_emits_a_gap() {
    let (mut open, fixture, relay, _) = open_stream("owner-a").await;
    open.state.event_wake = true;
    open.pending.clear();
    open.initial_gap = false;
    open.queue_live(false);
    let id = open.header.id.clone();
    assert!(open.drained_attachment.is_some());
    relay.replace_owner().await;
    open.wait_for_change().await.unwrap();
    let interruption = open
        .interruption
        .take()
        .expect("owner retirement woke the reader");
    let turn = start_turn(&fixture, &id).await;
    let current = open
        .state
        .reads
        .event_stream_header(&open.authorization)
        .await
        .unwrap();
    assert_eq!(current.active_turn, Some(turn));
    open.reattach(interruption).await.unwrap();
    assert!(!open.ended);
    assert_eq!(open.discarded_fragments, 1);

    let mut body = response(open).into_body().into_data_stream();
    assert!(next(&mut body).await.contains("turn_started"));
    assert!(next(&mut body).await.contains("\"reason\":\"lagged\""));
}

#[tokio::test]
async fn a_completed_turn_cannot_overtake_and_hide_a_pending_interruption() {
    let (open, fixture, relay, _) = open_stream("owner-a").await;
    let id = open.header.id.clone();
    let mut body = response(open).into_body().into_data_stream();
    catch_up(&mut body).await;
    relay.replace_owner().await;
    let sequence = start_turn(&fixture, &id).await;
    finish_turn(&fixture, &id, sequence).await;
    assert!(next(&mut body).await.contains("turn_started"));
    assert!(next(&mut body).await.contains("turn_completed"));
    assert!(next(&mut body).await.contains("\"reason\":\"lagged\""));
    assert!(timeout(Duration::from_millis(200), body.next())
        .await
        .is_err());
}

#[tokio::test]
async fn delayed_peer_retirement_cannot_rebase_onto_a_later_idle_head() {
    let (open, fixture, relay, _) = open_stream("owner-a").await;
    let id = open.header.id.clone();
    let mut body = response(open).into_body().into_data_stream();
    catch_up(&mut body).await;
    // The peer can still see the old token while the retirement is in transit.
    let sequence = start_turn(&fixture, &id).await;
    finish_turn(&fixture, &id, sequence).await;
    assert!(next(&mut body).await.contains("turn_started"));
    assert!(next(&mut body).await.contains("turn_completed"));
    assert!(timeout(Duration::from_millis(200), body.next())
        .await
        .is_err());
    relay.replace_owner().await;
    assert!(next(&mut body).await.contains("\"reason\":\"lagged\""));
}

#[tokio::test]
async fn retired_fragments_and_real_lag_preserve_order_and_counts() {
    let (open, _, relay, _) = open_stream("owner-a").await;
    let fragment = AgentLiveFragment {
        agent_id: open.header.id.clone(),
        runtime_generation: open.header.runtime_generation.unwrap(),
        turn_sequence: open.header.head,
        after_seq: open.header.head,
        message_id: "retired-message".into(),
        kind: AgentLiveFragmentKind::Message,
        delta: "retired payload".into(),
    };
    let mut body = response(open).into_body().into_data_stream();
    catch_up(&mut body).await;
    relay.publisher.publish(fragment, relay.authority.clone());
    relay.publisher.lagged(7);
    relay.replace_owner().await;
    for count in [1, 7, 1] {
        let wire = next(&mut body).await;
        assert!(wire.contains(&format!("\"frames\":{count}")), "{wire}");
        assert!(!wire.contains("retired payload"));
    }
}

#[tokio::test]
async fn access_withdrawal_ends_the_stream_during_reattach() {
    use layer2_core::resource_access::{ShareChange, ShareCommand};
    let (open, fixture, relay, share) = open_stream("reader").await;
    let mut body = response(open).into_body().into_data_stream();
    catch_up(&mut body).await;
    let share = share.unwrap();
    relay.replace_owner().await;
    fixture
        .access
        .change_share(
            &context("owner-a"),
            None,
            ShareCommand {
                selector: share.selector,
                change: ShareChange::Revoke { share_id: share.id },
                expected_share_revision: Some(share.revision),
                idempotency_key: "reattach-revocation".into(),
            },
        )
        .await
        .unwrap();
    assert!(timeout(Duration::from_secs(3), body.next())
        .await
        .unwrap()
        .is_none());
}
