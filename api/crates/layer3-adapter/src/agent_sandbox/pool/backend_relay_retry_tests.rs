use super::*;
use crate::{
    agent_sandbox::{pool::fixture_exec::RelayOutcome, with_initial_relay_readiness_retry},
    hosted::{NativeRuntimeTransport, ProviderHostedControlTransport},
};

async fn status(
    outcomes: Vec<RelayOutcome>,
    enabled: bool,
) -> (Result<ResponseFrame>, usize, usize) {
    let bound = Box::pin(bound(false)).await;
    let home = Box::pin(adopted(&bound.fixture, &bound.claim)).await;
    let client = client(&bound.fixture, false);
    let transport = ProviderHostedControlTransport::for_agent_sandbox_with_transport(
        client.clone(),
        NativeRuntimeTransport::KubernetesExec,
    )
    .unwrap();
    let backend = AgentSandboxBackend::new(client, Arc::new(transport));
    let handle = backend
        .provision_with_home(bound.request, &home)
        .await
        .unwrap();
    bound.fixture.state.lock().unwrap().relay_outcomes = outcomes.into();
    let request = RequestFrame::RuntimeStatus(RuntimeStatusRequest {
        sandbox_id: home.request.sandbox_id.clone(),
        generation: 1,
        home_identity: home.identity().unwrap(),
    });
    let exchange = backend.exchange(&handle, request);
    let result = tokio::time::timeout(std::time::Duration::from_secs(7), async {
        if enabled {
            with_initial_relay_readiness_retry(home, || Ok(None), exchange).await
        } else {
            exchange.await
        }
    })
    .await
    .expect("status fixture timed out");
    let state = bound.fixture.state.lock().unwrap();
    (result, state.relay_opens, state.relay_requests)
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
