use super::*;
use crate::turns::routed::{
    delivery_tests::{accepted, lane},
    test_support::{fixture, supported, Setup},
};
use serde_json::json;
use std::sync::{atomic::Ordering, Arc};

fn request(setup: &Setup) -> EventAttestationRequest {
    let event = delivery_event(&setup.plan, setup.target());
    EventAttestationRequest {
        event_ref: EventRef::seal(&event, &fixture::keys()).unwrap(),
        provider_binding_id: setup.target().launch.provider.binding_id.clone(),
        channel_id: event.channel_id,
        conversation_id: event.conversation_id,
        event_id: event.event_id,
        activity_sha256: event.activity_sha256.unwrap(),
        event_expires_at: event.expires_at,
        binding_generation: None,
        artifact: None,
    }
}

async fn check(setup: &Setup, request: &EventAttestationRequest) -> Result<bool, ExecutionError> {
    attest(
        &setup.server.store(),
        setup.journal(),
        setup.current.as_ref(),
        request,
    )
    .await
}

#[tokio::test]
async fn attestation_replay_is_read_only_and_keeps_the_active_response_lease() {
    let setup = Setup::new(supported()).await;
    let lease = accepted(&setup).await;
    let before = setup.server.memory.writes.load(Ordering::SeqCst);
    let original = request(&setup);
    let (delivery, mut effects) = lane(&setup);
    for _ in 0..3 {
        assert!(delivery
            .tool_replies()
            .attest_event(&original)
            .await
            .unwrap());
        setup.journal().guard(&lease).await.unwrap();
    }
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), before);
    assert!(effects.try_recv().is_err());
    assert_eq!(setup.port.starts.lock().unwrap().len(), 1);
    let response = serde_json::to_value(EventAttestationResponse { current: true }).unwrap();
    assert_eq!(response, json!({"current":true}));
}

#[tokio::test]
async fn attestation_requires_the_original_capability_digest_and_target_comparisons() {
    let setup = Setup::new(supported()).await;
    let _lease = accepted(&setup).await;
    let original = request(&setup);
    for defect in 0..9 {
        let mut changed = original.clone();
        match defect {
            0 => changed.event_ref = EventRef::from_sealed("cmaeg1.ev.invalid".into()),
            1 => changed.provider_binding_id = "other-binding".parse().unwrap(),
            2 => {
                changed.conversation_id = setup.plan.targets[1].launch.event.conversation_id.clone()
            }
            3 => changed.event_id = "other-event".parse().unwrap(),
            4 => changed.activity_sha256 = "0".repeat(64),
            5 => changed.binding_generation = Some(1),
            6 => changed.event_ref = EventRef::from_sealed("x".repeat(MAX_TOKEN_LENGTH + 1)),
            7 => changed.channel_id = "cmaeg1.ch.other-channel".parse().unwrap(),
            8 => changed.event_expires_at += 1,
            _ => unreachable!(),
        }
        assert!(!check(&setup, &changed).await.unwrap(), "defect {defect}");
    }
    // An unissued target cannot be selected by changing the consumer of an
    // otherwise valid sealed reference, even when the key ring is available.
    let mut other = delivery_event(&setup.plan, setup.target());
    other.consumer = Some(fixture::id(fixture::NYX));
    let mut changed = original;
    changed.event_ref = EventRef::seal(&other, &fixture::keys()).unwrap();
    assert!(!matches!(check(&setup, &changed).await, Ok(true)));
}

#[tokio::test]
async fn attestation_rechecks_removed_source_consumer_provider_and_routing() {
    for defect in 0..6 {
        let setup = Setup::new(supported()).await;
        let _lease = accepted(&setup).await;
        let original = request(&setup);
        assert!(check(&setup, &original).await.unwrap());
        {
            let mut current = setup.current.channel.lock().unwrap();
            let mut changed = (**current).clone();
            match defect {
                0 => changed.definition.sources.clear(),
                1 => changed
                    .definition
                    .routing
                    .as_mut()
                    .unwrap()
                    .consumers
                    .clear(),
                2 => changed.definition.owner.subject = "other-owner".into(),
                3 => changed.definition.routing = None,
                4 => {
                    changed.definition.routing.as_mut().unwrap().consumers[0]
                        .provider
                        .binding_id = "other-binding".parse().unwrap()
                }
                5 => changed.definition.secrets.consumer_keys.clear(),
                _ => unreachable!(),
            }
            *current = Arc::new(changed);
        }
        assert_eq!(
            check(&setup, &original).await,
            Err(ExecutionError::AuthorityChanged),
            "defect {defect}"
        );
    }
}

#[tokio::test]
async fn attestation_http_is_body_only_bounded_and_discloses_no_retained_content() {
    use crate::turns::routed::custom_http_fixture::App;
    let mut app = App::new(true).await;
    let received = app
        .request(app.event("attestation-http").to_string(), true)
        .send()
        .await
        .unwrap();
    assert_eq!(received.status(), 200);
    received.text().await.unwrap();
    app.drain().await;
    let plan = app.plans().await.remove(0);
    let context = app
        .provider
        .seen()
        .iter()
        .find(|seen| seen.method == "POST")
        .map(|seen| serde_json::from_slice::<serde_json::Value>(&seen.body).unwrap())
        .unwrap()["event_context"]
        .clone();
    let request = EventAttestationRequest {
        event_ref: serde_json::from_value(context["event_ref"].clone()).unwrap(),
        provider_binding_id: plan.targets[0].launch.provider.binding_id.clone(),
        channel_id: plan.identity.channel_id.clone(),
        conversation_id: serde_json::from_value(context["conversation_id"].clone()).unwrap(),
        event_id: plan.identity.event_id.clone(),
        activity_sha256: cmaeg_core::seal::event_ref::activity_digest(&plan.activity),
        event_expires_at: delivery_event(&plan, &plan.targets[0]).expires_at,
        binding_generation: None,
        artifact: None,
    };
    let origin = reqwest::Url::parse(&app.url)
        .unwrap()
        .origin()
        .ascii_serialization();
    let url = format!("{origin}/v1/event-attestations");
    let client = reqwest::Client::new();
    let response = client.post(&url).json(&request).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.text().await.unwrap(), r#"{"current":true}"#);
    assert_eq!(client.get(&url).send().await.unwrap().status(), 405);
    let oversized = json!({"event_ref":"x".repeat(17 * 1024)});
    assert_eq!(
        client
            .post(&url)
            .json(&oversized)
            .send()
            .await
            .unwrap()
            .status(),
        413
    );
    let mut invalid = serde_json::to_value(&request).unwrap();
    invalid["target_path"] = "/home/arbitrary".into();
    assert_eq!(
        client
            .post(&url)
            .json(&invalid)
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let mut expired = request.clone();
    let mut payload = expired
        .event_ref
        .open(
            &fixture::keys(),
            time::OffsetDateTime::now_utc().unix_timestamp(),
        )
        .unwrap()
        .payload;
    payload.expires_at = 1;
    expired.event_ref = EventRef::seal(&payload, &fixture::keys()).unwrap();
    let response = client.post(&url).json(&expired).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.text().await.unwrap(), r#"{"current":false}"#);
}

#[tokio::test]
async fn attestation_refuses_unsubmitted_refused_and_expired_originals() {
    let setup = Setup::new(supported()).await;
    let mut lease = setup.prepared().await;
    let original = request(&setup);
    assert_eq!(check(&setup, &original).await, Err(ExecutionError::Invalid));
    setup
        .journal()
        .transition(&mut lease, Phase::Unknown)
        .await
        .unwrap();
    assert!(check(&setup, &original).await.unwrap());
    setup
        .journal()
        .transition(
            &mut lease,
            Phase::Refused {
                code: "revoked".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(check(&setup, &original).await, Err(ExecutionError::Invalid));
    setup.server.memory.clock.fetch_add(901, Ordering::SeqCst);
    assert!(!check(&setup, &original).await.unwrap());
}

#[tokio::test]
async fn attestation_supports_custom_existing_agent_events_without_inventing_a_generation() {
    let setup = Setup::with_request(
        supported(),
        fixture::request(
            |channel| {
                channel["sources"] = json!([crate::channels::routing_fixture::custom()]);
                channel["provider"]["target"] =
                    json!({"type":"existing_agent","agent_id":"agt_existing"});
            },
            fixture::custom(None),
        ),
    )
    .await;
    let _lease = accepted(&setup).await;
    let original = request(&setup);
    assert!(check(&setup, &original).await.unwrap());
    let mut invented = original;
    invented.binding_generation = Some(1);
    assert!(!check(&setup, &invented).await.unwrap());
}
