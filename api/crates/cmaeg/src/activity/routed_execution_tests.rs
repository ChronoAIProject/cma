use super::*;
use crate::{
    delivery::{durable::DeliveryCoordinator, router::ShardedScheduler},
    providers::{parse_event, RecoveryEvent, SseFrame},
    turns::{
        execution_receipt::{
            ExecutionReceipt, ExecutionReceiptVerifier, ExecutionScope, ExecutionVerifiers,
        },
        routed::{
            test_support::{recovery_frame, supported, Setup},
            DispatchOutcome, FrameDelivery, ReplayEffect, RoutedDelivery,
        },
    },
};
use serde_json::{json, Value};

struct SyntheticProvider(ExecutionScope);

impl ExecutionReceiptVerifier for SyntheticProvider {
    fn verify(&self, scope: &ExecutionScope, terminal: &Value) -> Option<ExecutionReceipt> {
        (scope == &self.0 && terminal["receipt"] == "fixture-terminal-handoff").then(|| {
            ExecutionReceipt {
                scope: self.0.clone(),
                reference: "executor-after-handoff".into(),
            }
        })
    }
}

fn terminal_frame(receipt: &str) -> RecoveryEvent {
    let mut frame = recovery_frame("resp_1", 0, true);
    frame.raw["response"]["receipt"] = json!(receipt);
    frame.event = parse_event(&SseFrame {
        event: None,
        id: Some("0".into()),
        data: frame.raw.to_string(),
        retry: None,
    })
    .unwrap();
    frame
}

#[tokio::test]
async fn routed_execution_receipt_repairs_exact_replay_and_survives_restart_in_get_and_sse() {
    routed_execution_receipt(false).await;
}

#[tokio::test]
async fn routed_execution_receipt_persists_on_initial_terminal_delivery() {
    routed_execution_receipt(true).await;
}

async fn routed_execution_receipt(initially_enabled: bool) {
    let (server, mut state, mut request, mut creator) = fixture_app().await;
    let channel_id = request.channel.id.clone();
    fixture::rewrite_ref(&mut request, |event| event.channel_id = channel_id);
    let _authorization = live_auth(&mut state, &mut creator).await;
    let setup = Setup::with_request(supported(), request.clone()).await;
    let plan = &setup.plan;
    let identity = &plan.identity;
    let target = setup.target();
    let provider = &target.launch.provider.slug;
    let consumer = target.launch.consumer.as_str();
    let cold_store = || {
        Arc::new(
            ActivityStore::new(server.records.clone(), Arc::new(fixture::keys()))
                .with_execution_attribution(true),
        )
    };
    let activity = cold_store();
    activity.received(&request).await.unwrap();
    let mut accepted = Change::state(consumer, ConsumerState::Accepted, Some("resp_1".into()));
    accepted.provider = Some(provider.to_string());
    accepted.agent_id = Some("native-provider-agent".into());
    activity
        .change(
            identity.record_id.as_str(),
            identity.event_id.as_str(),
            &accepted,
        )
        .await
        .unwrap();
    let DispatchOutcome::Replay { mut lease, .. } = setup.dispatch().await.unwrap() else {
        panic!("routed response must be accepted");
    };
    let (delivery, mut sends) = tokio::sync::mpsc::channel(1);
    let lane = |verifiers| {
        RoutedDelivery::new(
            setup.server.store(),
            setup
                .journal()
                .clone()
                .with_activity(cold_store())
                .with_execution_verifiers(Arc::new(verifiers)),
            setup.current.clone(),
            DeliveryCoordinator::new(Arc::new(ShardedScheduler::new(delivery.clone()))),
        )
    };
    let receipt = terminal_frame("fixture-terminal-handoff");
    let verifier = Arc::new(SyntheticProvider(ExecutionScope {
        provider: provider.to_string(),
        channel_record_id: identity.record_id.to_string(),
        event_id: identity.event_id.to_string(),
        consumer_id: consumer.into(),
        response_id: "resp_1".into(),
    }));
    let initial = lane(
        ExecutionVerifiers::new(initially_enabled)
            .with_provider(provider.clone(), verifier.clone()),
    );
    assert!(matches!(
        initial
            .deliver_frame(
                identity,
                &target.id,
                &mut lease,
                &receipt,
                ReplayEffect::NoEffect
            )
            .await
            .unwrap(),
        FrameDelivery::Settled { cursor: 0, .. }
    ));

    let current = || activity.latest(&identity.owner, identity.record_id.as_str());
    let before = crate::activity::public_page(&current().await.unwrap());
    let previous = &before.items[0].consumers[0];
    assert_eq!(previous.state, ConsumerState::Completed);
    assert_eq!(
        previous.execution_reference.as_deref(),
        initially_enabled.then_some("executor-after-handoff"),
    );
    let on = lane(ExecutionVerifiers::new(true).with_provider(provider.clone(), verifier));
    assert!(on
        .deliver_frame(
            identity,
            &target.id,
            &mut lease,
            &terminal_frame("other-receipt"),
            ReplayEffect::NoEffect
        )
        .await
        .is_err());
    assert_eq!(current().await.unwrap().cursor, before.cursor);
    assert!(matches!(
        on.deliver_frame(
            identity,
            &target.id,
            &mut lease,
            &receipt,
            ReplayEffect::NoEffect
        )
        .await
        .unwrap(),
        FrameDelivery::Settled { cursor: 0, .. }
    ));
    assert!(sends.try_recv().is_err());

    state.activity = cold_store();
    let raw = request.channel.id.to_string();
    let response =
        crate::http::activity::latest(State(state.clone()), creator.clone(), Path(raw.clone()))
            .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 1_000_000)
        .await
        .unwrap();
    let page: Value = serde_json::from_slice(&body).unwrap();
    let consumer = &page["items"][0]["consumers"][0];
    assert_eq!(consumer["execution_reference"], "executor-after-handoff");
    assert_eq!(consumer["agent_id"], "native-provider-agent");
    assert_eq!(consumer["response_id"], "resp_1");
    let response = crate::http::activity::stream(
        State(state),
        creator,
        Path(raw),
        Query(crate::http::activity::ActivityQuery {
            cursor: Some(page["cursor"].as_str().unwrap().into()),
        }),
        auth_headers(),
    )
    .await;
    let mut stream = response.into_body().into_data_stream();
    let frame = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let data = std::str::from_utf8(&frame)
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .unwrap();
    assert_eq!(serde_json::from_str::<Value>(data).unwrap(), page);
}
