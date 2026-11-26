//! Real public SSE and private live relays, with independent clients/local state.
//! Authentication is a fixed test principal; no runtime receiver is simulated.
use super::super::super::*;
use axum::{Extension, Router};
use layer1_public::agent_v2::{router, AgentV2State};
use layer2_core::{
    agent_ownership::{candidates, OwnershipError, HEARTBEAT_INTERVAL_MS, PROTOCOL_VERSION},
    resource_access::*,
    sandbox::{agent_io::*, store::*, *},
    Authorization, Caller, Capability, SubjectId, WorkspaceProfileId,
};
use mongodb::Client;
use serde_json::Value;
use tokio::task::JoinHandle;

use crate::store::{
    sandbox_maintenance::mongo_integration_tests::fixture as retained_fixture, MongoSandboxStore,
};

#[path = "shared_stream_tests/containment.rs"]
mod containment;
#[path = "shared_stream_tests/fixture.rs"]
mod fixture;
#[path = "sw04_cancel_tests.rs"]
mod sw04_cancel;
use fixture::{seed, Replica};

async fn acquire(replicas: &[Replica], id: &AgentId) -> (usize, Arc<Authority>) {
    let membership = replicas
        .last()
        .unwrap()
        .coordinator
        .store
        .membership()
        .await
        .unwrap();
    let preferred = &candidates(id, &membership, None).unwrap()[0].instance_id;
    let index = replicas
        .iter()
        .position(|r| &r.coordinator.instance_id == preferred)
        .unwrap();
    let authority = replicas[index]
        .coordinator
        .acquire(id)
        .await
        .unwrap()
        .unwrap();
    (index, authority)
}

async fn commit(replica: &Replica, authority: &Arc<Authority>, logs: Vec<AgentLogKind>) -> Agent {
    let mut agent = replica
        .store
        .get_agent(&authority.token.agent_id)
        .await
        .unwrap()
        .unwrap();
    let revision = agent.revision;
    let sequence = agent.conversation.last_seq;
    for kind in &logs {
        match kind {
            AgentLogKind::TurnStarted { turn_sequence, .. } => {
                agent.conversation.active_turn = Some(*turn_sequence);
                agent.status = AgentStatus::Working;
            }
            AgentLogKind::TurnCompleted { .. } => {
                agent.conversation.active_turn = None;
                agent.status = AgentStatus::Ready;
            }
            _ => (),
        }
    }
    agent.revision += 1;
    agent.conversation.last_seq += logs.len() as u64;
    agent.updated_at = layer2_core::now_rfc3339();
    let admission_guard = if logs.iter().any(|kind| {
        matches!(
            kind,
            AgentLogKind::TurnStarted { .. } | AgentLogKind::OwnerMessage { .. }
        )
    }) {
        Some(SandboxMutationGuard::capture(
            &replica
                .store
                .get_sandbox(&agent.sandbox_id)
                .await
                .unwrap()
                .unwrap(),
        ))
    } else {
        None
    };
    authority
        .run(
            replica
                .store
                .commit_agent_conversation(AgentConversationMutation {
                    settings_fence: None,
                    command_claim: None,
                    execution_fence: None,
                    trigger_execution_fence: None,
                    admission_guard,
                    expected_agent_revision: revision,
                    expected_last_seq: sequence,
                    runtime_generation: 1,
                    appended_at: agent.updated_at.clone(),
                    agent: agent.clone(),
                    agent_logs: logs,
                }),
        )
        .await
        .unwrap()
        .unwrap_or_else(|error| panic!("fixture commit after sequence {sequence}: {error:?}"));
    agent
}

async fn subscribed(replica: &Replica, id: &AgentId, count: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while replica.updates.live_subscriber_count(id) != count {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

async fn publish(replica: &Replica, authority: &Arc<Authority>, after: u64, delta: &str) {
    // Public command admission can append records before the turn starts.
    let turn_sequence = replica
        .store
        .get_agent(&authority.token.agent_id)
        .await
        .unwrap()
        .unwrap()
        .conversation
        .active_turn
        .unwrap();
    authority
        .run(async {
            replica.updates.publish_live_fixture(AgentLiveFragment {
                agent_id: authority.token.agent_id.clone(),
                runtime_generation: 1,
                turn_sequence,
                after_seq: after,
                message_id: "stream-message".into(),
                kind: AgentLiveFragmentKind::Message,
                delta: delta.into(),
            });
        })
        .await
        .unwrap();
}

struct SseReader {
    response: reqwest::Response,
    buffered: String,
}
#[derive(Debug)]
struct Frame {
    event: String,
    id: Option<String>,
    body: Value,
}
impl SseReader {
    async fn next(&mut self) -> Option<Frame> {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(end) = self.buffered.find("\n\n") {
                    let wire = self.buffered[..end].to_owned();
                    self.buffered.drain(..end + 2);
                    let field = |name: &str| wire.lines().find_map(|line| line.strip_prefix(name));
                    let Some(event) = field("event: ") else {
                        continue;
                    };
                    return Some(Frame {
                        event: event.into(),
                        id: field("id: ").map(str::to_owned),
                        body: serde_json::from_str(field("data: ").unwrap()).unwrap(),
                    });
                }
                let chunk = self.response.chunk().await.unwrap()?;
                self.buffered.push_str(std::str::from_utf8(&chunk).unwrap());
                assert!(self.buffered.len() < 1024 * 1024);
            }
        })
        .await
        .expect("SSE failed to make progress before the fallback poll")
    }
    async fn event(&mut self, expected: &str) -> Frame {
        for _ in 0..8 {
            let frame = self.next().await.expect("stream ended unexpectedly");
            if frame.event == expected {
                return frame;
            }
            assert_eq!(
                frame.event, "cma.agent.live_gap",
                "unexpected frame: {frame:?}"
            );
            assert!(
                frame.id.is_none(),
                "ephemeral gap must not advance the cursor"
            );
        }
        panic!("expected stream event did not arrive")
    }
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn independent_clients_follow_owner_and_reconnect_from_durable_cursor() {
    for count in [2, 3] {
        let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        let name = format!("cma_shared_stream_{}", uuid::Uuid::new_v4().simple());
        let off = MongoSandboxStore::open(&client, &name).await.unwrap();
        let agent = seed(&off, &client.database(&name)).await;
        drop(off);
        let mut replicas = Vec::new();
        for _ in 0..count {
            replicas.push(Replica::open(&name, &agent.owner).await);
        }
        let (owner, authority) = acquire(&replicas, &agent.id).await;
        commit(
            &replicas[owner],
            &authority,
            vec![
                AgentLogKind::TurnStarted {
                    model: None,
                    turn_sequence: 3,
                    origin_key: None,
                },
                AgentLogKind::OwnerMessage {
                    content: "fixture input".into(),
                },
            ],
        )
        .await;
        let mut streams = Vec::new();
        for replica in &replicas {
            let mut stream = replica.stream(&agent.id, None, true).await;
            for sequence in 1..=4 {
                let frame = stream.event("cma.agent.event").await;
                assert_eq!(frame.body["sequence"], sequence);
            }
            streams.push(Some(stream));
        }
        subscribed(&replicas[owner], &agent.id, count).await;
        publish(&replicas[owner], &authority, 4, "first live fragment").await;
        for stream in streams.iter_mut().flatten() {
            let frame = stream.event("cma.agent.live").await;
            assert_eq!(frame.body["delta"], "first live fragment");
            assert!(frame.id.is_none());
        }
        let mut heads: Vec<_> = replicas
            .iter()
            .map(|replica| replica.updates.subscribe_head(&agent.id).unwrap())
            .collect();
        commit(
            &replicas[owner],
            &authority,
            vec![AgentLogKind::AgentMessage {
                content: "first durable message".into(),
            }],
        )
        .await;
        assert!(replicas[owner]
            .updates
            .notify_owned_head(&agent.id, &authority));
        for head in &mut heads {
            tokio::time::timeout(Duration::from_secs(5), head.recv())
                .await
                .unwrap()
                .unwrap();
        }
        drop(heads);
        let mut cursor = None;
        for stream in streams.iter_mut().flatten() {
            let frame = stream.event("cma.agent.event").await;
            assert_eq!(frame.body["sequence"], 5);
            let next = frame.id.unwrap();
            if let Some(previous) = &cursor {
                assert_eq!(previous, &next);
            }
            cursor = Some(next);
        }
        replicas[owner].coordinator.begin_drain().await;
        replicas[owner]
            .coordinator
            .handoff(&authority, async { Ok(()) })
            .await
            .unwrap();
        assert!(matches!(
            replicas[owner]
                .coordinator
                .store
                .check(&authority.token)
                .await,
            Err(OwnershipError::Stale)
        ));
        assert!(!replicas[owner]
            .updates
            .notify_owned_head(&agent.id, &authority));
        assert!(authority
            .run(async { panic!("retired owner executed a publication") })
            .await
            .is_err());
        streams[owner] = None;
        let (successor, current) = acquire(&replicas, &agent.id).await;
        assert_ne!(owner, successor);
        assert!(current.token.epoch > authority.token.epoch);
        for stream in streams.iter_mut().flatten() {
            let gap = stream.event("cma.agent.live_gap").await;
            assert!(gap.id.is_none());
            assert_eq!(gap.body["reason"], "lagged");
        }
        subscribed(&replicas[successor], &agent.id, count - 1).await;
        publish(&replicas[successor], &current, 5, "successor live fragment").await;
        for stream in streams.iter_mut().flatten() {
            let frame = stream.event("cma.agent.live").await;
            assert_eq!(frame.body["delta"], "successor live fragment");
            assert!(frame.id.is_none());
        }
        drop(streams);
        subscribed(&replicas[successor], &agent.id, 0).await;
        // Recreate ingress/local caches and Mongo pool. The retained owner does
        // not change; the cursor contains no routing affinity.
        let fresh = Replica::open(&name, &agent.owner).await;
        let mut reconnected = fresh.stream(&agent.id, cursor.as_deref(), true).await;
        let initial_gap = reconnected.event("cma.agent.live_gap").await;
        assert_eq!(initial_gap.body["reason"], "reconnected");
        subscribed(&replicas[successor], &agent.id, 1).await;
        publish(
            &replicas[successor],
            &current,
            5,
            "reconnected live fragment",
        )
        .await;
        assert_eq!(
            reconnected.event("cma.agent.live").await.body["delta"],
            "reconnected live fragment"
        );
        drop(reconnected);
        commit(
            &replicas[successor],
            &current,
            vec![
                AgentLogKind::AgentMessage {
                    content: "committed while disconnected".into(),
                },
                AgentLogKind::TurnCompleted { turn_sequence: 3 },
            ],
        )
        .await;
        // No head notification: durable catch-up must work independently.
        for replica in [&fresh, &replicas[successor]] {
            let mut replay = replica.stream(&agent.id, cursor.as_deref(), false).await;
            for sequence in [6, 7] {
                let frame = replay.event("cma.agent.event").await;
                assert_eq!(frame.body["agent_id"], agent.id.as_str());
                assert_eq!(frame.body["sequence"], sequence);
                assert_eq!(frame.id.as_deref(), frame.body["cursor"].as_str());
                assert_eq!(
                    frame.body["kind"]["type"],
                    if sequence == 7 {
                        "turn_completed"
                    } else {
                        "message"
                    }
                );
            }
            assert!(replay.next().await.is_none());
        }
        fresh.close().await;
        for replica in replicas {
            replica.close().await;
        }
        client.database(&name).drop().await.unwrap();
        client.shutdown().await;
    }
}
