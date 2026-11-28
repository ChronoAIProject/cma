use super::*;
use crate::{
    agent_sandbox::{pool::fixture_exec::RelayOutcome, with_initial_relay_readiness_retry},
    hosted::{NativeRuntimeTransport, ProviderHostedControlTransport},
};

struct RelayFixture {
    bound: Bound,
    backend: AgentSandboxBackend,
    home: HomeVolumeAttachment,
}

impl RelayFixture {
    async fn start() -> Self {
        let bound = Box::pin(bound(false)).await;
        let home = Box::pin(adopted(&bound.fixture, &bound.claim)).await;
        let client = client(&bound.fixture, false);
        let transport = ProviderHostedControlTransport::for_agent_sandbox_with_transport(
            client.clone(),
            NativeRuntimeTransport::KubernetesExec,
        )
        .unwrap();
        let backend = AgentSandboxBackend::new(client, Arc::new(transport));
        Self {
            bound,
            backend,
            home,
        }
    }

    async fn provision(&self) -> RuntimeHandle {
        self.backend
            .provision_with_home(self.bound.request.clone(), &self.home)
            .await
            .unwrap()
    }

    async fn status(&self, handle: &RuntimeHandle, enabled: bool) -> Result<ResponseFrame> {
        let request = RequestFrame::RuntimeStatus(RuntimeStatusRequest {
            sandbox_id: self.home.request.sandbox_id.clone(),
            generation: 1,
            home_identity: self.home.identity().unwrap(),
        });
        let exchange = self.backend.exchange(handle, request);
        tokio::time::timeout(std::time::Duration::from_secs(7), async {
            if enabled {
                with_initial_relay_readiness_retry(self.home.clone(), || Ok(None), exchange).await
            } else {
                exchange.await
            }
        })
        .await
        .expect("status fixture timed out")
    }

    fn outcomes(&self, outcomes: impl Into<std::collections::VecDeque<RelayOutcome>>) {
        self.bound.fixture.state.lock().unwrap().relay_outcomes = outcomes.into();
    }

    fn counts(&self) -> (usize, usize) {
        let state = self.bound.fixture.state.lock().unwrap();
        (state.relay_opens, state.relay_requests)
    }
}

async fn status(
    outcomes: Vec<RelayOutcome>,
    enabled: bool,
) -> (Result<ResponseFrame>, usize, usize) {
    let fixture = Box::pin(RelayFixture::start()).await;
    let handle = fixture.provision().await;
    fixture.outcomes(outcomes);
    let result = fixture.status(&handle, enabled).await;
    let (opens, requests) = fixture.counts();
    (result, opens, requests)
}

#[tokio::test]
async fn relay_readiness_retry_recovers_same_binding_with_one_status_request() {
    let (result, opens, requests) =
        status(vec![RelayOutcome::EarlyExit, RelayOutcome::Status], true).await;
    assert!(
        matches!(result, Ok(ResponseFrame::RuntimeStatus(_))),
        "{result:?}"
    );
    assert_eq!((opens, requests), (2, 1));
}

#[tokio::test]
async fn relay_readiness_retry_refuses_changed_binding_and_invalid_marker() {
    for outcome in [RelayOutcome::ReplacedPod, RelayOutcome::InvalidMarker] {
        let (result, opens, requests) = status(vec![outcome], true).await;
        assert!(
            matches!(result, Err(AdapterError::Rejected(_))),
            "{result:?}"
        );
        assert_eq!((opens, requests), (1, 0));
    }
}

#[tokio::test]
async fn relay_readiness_retry_stops_at_the_budget_or_when_disabled() {
    let (result, opens, requests) = status(
        vec![
            RelayOutcome::EarlyExit,
            RelayOutcome::EarlyExit,
            RelayOutcome::EarlyExit,
        ],
        true,
    )
    .await;
    assert!(
        matches!(result, Err(AdapterError::Unavailable(_))),
        "{result:?}"
    );
    assert_eq!((opens, requests), (3, 0));
    let (result, opens, requests) = status(vec![RelayOutcome::EarlyExit], false).await;
    assert!(
        matches!(result, Err(AdapterError::Unavailable(_))),
        "{result:?}"
    );
    assert_eq!((opens, requests), (1, 0));
}

#[tokio::test]
async fn relay_readiness_retry_does_not_replay_a_written_status_request() {
    let (result, opens, requests) = status(vec![RelayOutcome::DisconnectAfterWrite], true).await;
    assert!(
        matches!(result, Err(AdapterError::Unavailable(_))),
        "{result:?}"
    );
    assert_eq!((opens, requests), (1, 1));
}

#[tokio::test]
async fn relay_readiness_retry_leaves_a_sustained_listener_gap_to_the_durable_retry() {
    use std::time::Duration;
    use tokio::time::Instant;

    let fixture = Box::pin(RelayFixture::start()).await;
    let handle = fixture.provision().await;
    let assigned_at = Instant::now();
    let listener_ready_at = assigned_at + Duration::from_millis(24_350);
    fixture.outcomes([RelayOutcome::ListenerReadyAt(listener_ready_at); 4]);
    let assignments = fixture
        .bound
        .fixture
        .state
        .lock()
        .unwrap()
        .assignments
        .len();

    let result = fixture.status(&handle, true).await;
    assert!(
        matches!(result, Err(AdapterError::Unavailable(_))),
        "{result:?}"
    );
    assert_eq!(fixture.counts(), (3, 0));
    assert!(fixture
        .bound
        .fixture
        .state
        .lock()
        .unwrap()
        .relay_first_marker
        .is_none());

    // Model the ordinary 30-second retry becoming due without sleeping the test.
    let ordinary_retry_at = assigned_at + Duration::from_secs(30);
    tokio::time::pause();
    tokio::time::advance(ordinary_retry_at.saturating_duration_since(Instant::now())).await;
    tokio::time::resume();

    assert_eq!(fixture.provision().await, handle);
    let result = fixture.status(&handle, true).await;
    assert!(
        matches!(result, Ok(ResponseFrame::RuntimeStatus(_))),
        "{result:?}"
    );
    assert_eq!(fixture.counts(), (4, 1));
    let state = fixture.bound.fixture.state.lock().unwrap();
    assert_eq!(state.assignments.len(), assignments);
    let marker = state.relay_first_marker.unwrap();
    assert!(marker >= ordinary_retry_at);
    eprintln!(
        "Simulated assignment to first valid relay marker was {} ms",
        marker.duration_since(assigned_at).as_millis()
    );
}
