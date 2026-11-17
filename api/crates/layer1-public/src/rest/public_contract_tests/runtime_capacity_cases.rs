//! Capacity fixtures inject only the runtime/live-source boundary, using real routes.
use layer2_core::{sandbox::*, AgentId};

pub(super) fn capacity_failure() -> layer3_adapter::agent_sandbox::RuntimeCapacityFailure {
    use layer3_adapter::agent_sandbox::*;
    RuntimeCapacityFailure {
        kind: RuntimePermitKind::Request,
        class: RuntimeWorkloadClass::Ordinary,
        reason: RuntimeCapacityReason::Exhausted,
    }
}
pub(super) fn request_failure() -> layer3_adapter::AdapterError {
    capacity_failure().into()
}

pub(super) struct FullLiveSource;
impl AgentLiveSource for FullLiveSource {
    fn subscribe(&self, _: &AgentId) -> Option<AgentLiveSubscription> {
        panic!("public streams must use typed live admission")
    }
    fn try_subscribe(&self, _: &AgentId) -> Result<Option<AgentLiveSubscription>, AgentLiveError> {
        Err(AgentLiveError::Capacity)
    }
}

struct PendingLiveSource {
    deadline: std::time::Duration,
    refusal: bool,
    pending: std::sync::Mutex<Vec<tokio::sync::oneshot::Sender<Result<(), AgentLiveError>>>>,
}

impl AgentLiveSource for PendingLiveSource {
    fn subscribe(&self, _: &AgentId) -> Option<AgentLiveSubscription> {
        let (_, subscription) = AgentLiveSubscription::channel();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        if self.refusal {
            sender.send(Err(AgentLiveError::Capacity)).unwrap();
        } else {
            self.pending.lock().unwrap().push(sender);
        }
        Some(subscription.with_admission_deadline(receiver, self.deadline))
    }
}

#[tokio::test(start_paused = true)]
async fn owner_admission_is_bounded_and_preserves_durable_replay_or_named_refusal() {
    use crate::rest::agents_tests::{conversation_fixture, insert_agent, request, seeded_agent};
    use axum::{body::to_bytes, http::Method};
    use std::{sync::Arc, time::Duration};
    use tokio::time::Instant;
    use tower::ServiceExt;

    let deadline = Duration::from_secs(3);
    for mode in ["local", "unanswered", "capacity"] {
        let (state, store) = conversation_fixture().await;
        insert_agent(
            &store,
            seeded_agent("agt_admission", AgentStatus::Ready, 6, None),
            vec![
                AgentLogKind::TurnStarted {
                    model: None,
                    turn_sequence: 3,
                    origin_key: None,
                },
                AgentLogKind::OwnerMessage {
                    content: "Question".into(),
                },
                AgentLogKind::AgentMessage {
                    content: "Durable answer".into(),
                },
                AgentLogKind::TurnCompleted { turn_sequence: 3 },
            ],
        )
        .await;
        let updates = DurableAgentUpdateRecorder::new(store);
        if mode != "local" {
            updates.install_live_source(Arc::new(PendingLiveSource {
                deadline,
                refusal: mode == "capacity",
                pending: Default::default(),
            }));
        }
        let router = crate::agui::router(state.with_updates(updates));
        let started = Instant::now();
        let response = router
            .oneshot(request(
                Method::GET,
                "/api/v1/agui/events?thread_id=agt_admission&follow=false",
                "owner-a",
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(
            started.elapsed(),
            if mode == "unanswered" {
                deadline
            } else {
                Duration::ZERO
            }
        );
        if mode == "capacity" {
            assert_eq!(response.status(), 429);
            assert_eq!(response.headers()["retry-after"], "1");
            let body: serde_json::Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap())
                    .unwrap();
            assert_eq!(body["code"], "relay_observer_capacity_exhausted");
        } else {
            assert_eq!(response.status(), 200);
            let body = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
            let wire = std::str::from_utf8(&body).unwrap();
            assert!(wire.contains("Durable answer"));
            assert!(wire.contains("RUN_FINISHED"));
            assert!(wire.contains("id: 6"));
        }
    }
}

#[tokio::test]
async fn approval_observation_uses_durable_replay_when_ordinary_observers_are_full() {
    use std::sync::Arc;
    let (state, store) = crate::rest::agents_tests::conversation_fixture().await;
    let updates = DurableAgentUpdateRecorder::new(store);
    updates.install_live_source(Arc::new(FullLiveSource));
    let state = state.with_updates(updates);
    let id = AgentId("agt_approval_capacity".into());
    assert_eq!(state.watch(&id).await.err().unwrap().status(), 429);
    assert!(state.watch_control(&id).await.unwrap().is_none());
}

#[tokio::test]
async fn all_capacity_problems_are_retryable_and_in_the_shared_inventory() {
    use crate::rest::HttpProblem;
    use axum::{body::to_bytes, response::IntoResponse};
    let inventory: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/public-contract-inventory.json"
    ))
    .unwrap();
    let mut actual = std::collections::BTreeSet::new();
    for problem in [
        HttpProblem::RuntimeRequestCapacityExhausted,
        HttpProblem::RuntimeStreamCapacityExhausted,
        HttpProblem::RelayObserverCapacityExhausted,
        HttpProblem::RelayCapacityExhausted,
    ] {
        let response = problem.into_response();
        assert_eq!(response.status(), 429);
        assert_eq!(response.headers()["retry-after"], "1");
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        actual.insert(body["code"].as_str().unwrap().to_owned());
    }
    let expected = inventory["shared_capacity_problems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|code| code.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(actual, expected);
}
