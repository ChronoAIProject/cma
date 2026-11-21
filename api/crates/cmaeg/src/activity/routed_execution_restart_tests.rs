use super::*;
use crate::{
    ingress::routing_store::test_server::Memory,
    turns::routed::{
        adapter_test_server::HttpServer,
        delivery_tests::stored,
        executor_tests::executor,
        scheduler::{Disposition, TargetExecutor},
        Dispatcher, Phase,
    },
};

struct OutageAtVerification {
    provider: SyntheticProvider,
    activity: Arc<Memory>,
    calls: AtomicUsize,
}
impl ExecutionReceiptVerifier for OutageAtVerification {
    fn verify(&self, scope: &ExecutionScope, terminal: &Value) -> Option<ExecutionReceipt> {
        let verified = self.provider.verify(scope, terminal)?;
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.activity.unavailable_reads.store(100, Ordering::SeqCst);
        Some(verified)
    }
}

#[tokio::test]
async fn routed_execution_receipt_recovers_from_activity_outage_without_replaying_the_terminal_frame(
) {
    let (records, mut state, mut request, mut creator) = fixture_app().await;
    let channel = request.channel.id.clone();
    fixture::rewrite_ref(&mut request, |event| event.channel_id = channel);
    let _authorization = live_auth(&mut state, &mut creator).await;
    let mut setup = Setup::with_request(supported(), request.clone()).await;
    let identity = setup.plan.identity.clone();
    let slug = setup.target().launch.provider.slug.clone();
    let consumer = setup.target().launch.consumer.to_string();
    let cold_store = || {
        Arc::new(
            ActivityStore::new(records.records.clone(), Arc::new(fixture::keys()))
                .with_execution_attribution(true),
        )
    };
    let activity = cold_store();
    activity.received(&request).await.unwrap();
    let mut accepted = Change::state(
        &consumer,
        ConsumerState::Accepted,
        Some("resp_accepted".into()),
    );
    accepted.provider = Some(slug.to_string());
    activity
        .change(
            identity.record_id.as_str(),
            identity.event_id.as_str(),
            &accepted,
        )
        .await
        .unwrap();
    let verifier = Arc::new(OutageAtVerification {
        provider: SyntheticProvider(ExecutionScope {
            provider: slug.to_string(),
            channel_record_id: identity.record_id.to_string(),
            event_id: identity.event_id.to_string(),
            consumer_id: consumer,
            response_id: "resp_accepted".into(),
        }),
        activity: records.memory.clone(),
        calls: AtomicUsize::new(0),
    });
    let verifiers = Arc::new(ExecutionVerifiers::new(true).with_provider(slug, verifier.clone()));
    let bind = |setup: &Setup, activity| {
        Dispatcher::new(
            setup.server.store(),
            setup
                .journal()
                .clone()
                .with_activity(activity)
                .with_execution_verifiers(verifiers.clone()),
            setup.current.clone(),
            setup.port.clone(),
        )
    };
    setup.dispatcher = bind(&setup, activity);
    let http = HttpServer::start().await;
    http.memory.lock().unwrap().config.replay_frames = Some(vec![
        json!({"type":"response.created","sequence_number":0,"response":{"id":"resp_accepted","object":"response","status":"queued","conversation":{"id":"agt_native"},"metadata":{},"output":[]}}),
        json!({"type":"response.completed","sequence_number":1,"response":{"id":"resp_accepted","object":"response","status":"completed","conversation":{"id":"agt_native"},"metadata":{},"output":[],"receipt":"fixture-terminal-handoff"}}),
    ]);
    let (runner, mut sends, work) = executor(&setup, &http);
    assert!(matches!(runner.execute(&work).await, Disposition::Retry(_)));
    assert!(matches!(
        stored(&setup).await.phase,
        Phase::Accepted {
            durable_cursor: Some(1),
            ..
        }
    ));
    assert!(sends.try_recv().is_err());
    assert_eq!(verifier.calls.load(Ordering::SeqCst), 1);
    drop(runner);

    records.memory.unavailable_reads.store(0, Ordering::SeqCst);
    setup.dispatcher = bind(&setup, cold_store());
    let request_count = http.seen().len();
    let (resumed, mut sends, work) = executor(&setup, &http);
    assert!(matches!(resumed.execute(&work).await, Disposition::Settled));
    assert!(matches!(
        stored(&setup).await.phase,
        Phase::Completed {
            durable_cursor: Some(1),
            ..
        }
    ));
    assert!(sends.try_recv().is_err());
    assert_eq!(http.seen().len(), request_count);
    assert_eq!(verifier.calls.load(Ordering::SeqCst), 1);

    state.activity = cold_store();
    let page = read_visible_activity(state, creator, request.channel.id.to_string()).await;
    let actor = &page["items"][0]["consumers"][0];
    assert_eq!(actor["execution_reference"], "executor-after-handoff");
    assert_eq!(actor["agent_id"], "agt_native");
    assert_eq!(actor["response_id"], "resp_accepted");
}
