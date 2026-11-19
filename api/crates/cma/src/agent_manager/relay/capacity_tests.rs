use super::*;
use layer2_core::sandbox::{AgentLiveError, AgentUpdateRecorder};

#[test]
fn only_lifecycle_and_control_routes_spend_reserve() {
    use axum::http::Method;
    for (path, method, expected) in [
        (
            "/internal/agent-manager/v1/live/agt_one",
            Method::GET,
            capacity::Class::Observer,
        ),
        (
            "/internal/agent-manager/v1/control/agt_one/reopen",
            Method::POST,
            capacity::Class::Control,
        ),
        (
            "/api/v1/agents/agt_one/recover-worktree",
            Method::POST,
            capacity::Class::Control,
        ),
        (
            "/api/v1/agents/agt_one/skills",
            Method::POST,
            capacity::Class::Control,
        ),
        (
            "/api/v1/agents/agt_one/skills",
            Method::GET,
            capacity::Class::Ordinary,
        ),
        (
            "/api/v1/agents/agt_one/files",
            Method::POST,
            capacity::Class::Ordinary,
        ),
        (
            "/api/v1/agents/agt_one/browser/control",
            Method::POST,
            capacity::Class::Ordinary,
        ),
    ] {
        assert_eq!(capacity::class(path, &method), expected);
    }
}

#[tokio::test]
async fn owner_call_capacity_stays_typed_and_recoverable_across_http() {
    use axum::{routing::get, Router};
    use layer3_adapter::{agent_sandbox::*, AdapterError};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new().route(
        "/capacity",
        get(|| async {
            call_failure::response(
                RuntimeCapacityFailure {
                    kind: RuntimePermitKind::Stream,
                    class: RuntimeWorkloadClass::Continuity,
                    reason: RuntimeCapacityReason::Exhausted,
                }
                .into(),
            )
        }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let response = reqwest::Client::new()
        .get(format!("http://{address}/capacity"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 429);
    assert_eq!(response.headers()["retry-after"], "1");
    let error = call_failure::CallFailure::remote(response, RuntimeWorkloadClass::Continuity)
        .await
        .into_adapter("lost");
    assert!(matches!(
        error,
        AdapterError::RuntimeCapacity(RuntimeCapacityFailure {
            kind: RuntimePermitKind::Stream,
            class: RuntimeWorkloadClass::Continuity,
            ..
        })
    ));
    for class in [
        RuntimeWorkloadClass::Continuity,
        RuntimeWorkloadClass::Control,
    ] {
        let error = call_failure::CallFailure::problem("relay_capacity_exhausted", class)
            .into_adapter("lost");
        assert!(
            matches!(error, AdapterError::Unavailable(ref detail) if detail == "relay_capacity_exhausted: slots_exhausted")
        );
        let unknown =
            call_failure::CallFailure::problem("unknown", class).into_adapter("owner unavailable");
        assert!(
            matches!(unknown, AdapterError::Unavailable(ref detail) if detail == "owner unavailable")
        );
    }
    server.abort();
}

#[tokio::test]
async fn activation_supports_300_observers_and_reserves_physical_slots_for_controls() {
    let (store, instances) = cluster_with_capacity(
        1,
        crate::config::RelayCapacityConfig {
            limit: 384,
            control_reserve: 64,
        },
    )
    .await;
    let id = AgentId("agt-quota-observers".into());
    let (owner, authority) = acquire(&store, &instances, &id).await;
    let instance = &instances[owner];
    let mut observers = Vec::new();
    for _ in 0..300 {
        observers.push(instance.updates.admit_live(&id).await.unwrap().unwrap());
    }
    assert_eq!(instance.relay.slots.available_permits(), 84);
    authority
        .run(async {
            instance
                .updates
                .publish_live_fixture(fragment(&id, "shared"));
        })
        .await
        .unwrap();
    for observer in &mut observers {
        assert_eq!(observer.recv().await.unwrap().delta, "shared");
    }
    for _ in 300..320 {
        observers.push(instance.updates.admit_live(&id).await.unwrap().unwrap());
    }
    assert!(matches!(
        instance.updates.admit_live(&id).await,
        Err(AgentLiveError::Capacity)
    ));
    let controls: Vec<_> = (0..64)
        .map(|_| instance.relay.acquire(capacity::Class::Control).unwrap())
        .collect();
    assert_eq!(instance.relay.slots.available_permits(), 0);
    assert!(instance.relay.acquire(capacity::Class::Control).is_err());
    authority
        .run(async {
            instance
                .updates
                .publish_live_fixture(fragment(&id, "accepted-output-at-capacity"));
        })
        .await
        .unwrap();
    for observer in &mut observers {
        assert_eq!(
            observer.recv().await.unwrap().delta,
            "accepted-output-at-capacity"
        );
    }
    drop((controls, observers));
    tokio::time::timeout(Duration::from_secs(2), async {
        while instance.relay.slots.available_permits() != 384 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn remote_saturation_is_typed_before_subscription_admission_and_recovers() {
    let (store, instances) = cluster(2).await;
    let id = AgentId("agt-remote-quota".into());
    let (owner, _authority) = acquire(&store, &instances, &id).await;
    let peer = &instances[1 - owner];
    let full: Vec<_> = (0..MAX_STREAMS)
        .map(|_| {
            instances[owner]
                .relay
                .acquire(capacity::Class::Control)
                .unwrap()
        })
        .collect();
    assert!(matches!(
        peer.updates.admit_live(&id).await,
        Err(AgentLiveError::Capacity)
    ));
    drop(full);
    let admitted = peer.updates.admit_live(&id).await.unwrap().unwrap();
    assert_eq!(
        instances[owner].relay.slots.available_permits(),
        MAX_STREAMS - 1,
        "owner body has exactly one slot"
    );
    drop(admitted);
    tokio::time::timeout(Duration::from_secs(2), async {
        while peer.relay.slots.available_permits() != MAX_STREAMS {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn relay_capacity_responses_are_named_retryable_and_body_free() {
    use axum::body::to_bytes;
    for (observer, code) in [
        (true, "relay_observer_capacity_exhausted"),
        (false, "relay_capacity_exhausted"),
    ] {
        let response = capacity::refused(observer);
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()["retry-after"], "1");
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(body["code"], code);
        assert!(body.get("occupancy").is_none());
    }
}

#[tokio::test]
async fn observer_survives_initial_owner_election_and_resumes_live_delivery() {
    let (store, instances) = cluster(1).await;
    let id = AgentId("agt-owner-not-yet-elected".into());
    let mut subscription = instances[0].updates.admit_live(&id).await.unwrap().unwrap();
    let (owner, authority) = acquire(&store, &instances, &id).await;
    subscribed(&instances[owner], &id, 1).await;
    authority
        .run(async {
            instances[owner]
                .updates
                .publish_live_fixture(fragment(&id, "after-election"));
        })
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), subscription.recv())
            .await
            .unwrap()
            .unwrap()
            .delta,
        "after-election"
    );
}

#[tokio::test]
async fn expired_read_custody_cannot_spend_a_second_slot_or_fall_back_to_native_ownership() {
    use layer2_core::sandbox::agent_observation::*;
    let (_, instances) = cluster_with_capacity(
        1,
        crate::config::RelayCapacityConfig {
            limit: 2,
            control_reserve: 1,
        },
    )
    .await;
    let instance = &instances[0];
    let id = AgentId("agt-read-custody-expired".into());
    let _permit = instance.relay.acquire(capacity::Class::Observer).unwrap();
    let authority = AgentObservationAuthority::new(
        AgentObservationToken {
            agent_id: id.clone(),
            created_at: "fixture".into(),
            instance_id: instance.coordinator.instance_id.clone(),
            epoch: 1,
        },
        tokio::time::Instant::now(),
    );
    let result = admitted_observation(
        id.clone(),
        observation_scope(authority, instance.updates.admit_live(&id)),
    )
    .await;
    assert!(matches!(result, Err(AgentLiveError::Unavailable)));
    assert_eq!(instance.relay.slots.available_permits(), 1);
    assert!(instance
        .coordinator
        .store
        .owner(&id)
        .await
        .unwrap()
        .is_none());
}
