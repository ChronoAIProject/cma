use super::*;

#[tokio::test]
async fn first_failure_log_is_safe_correlated_and_emitted_before_revocation() {
    use std::io::Write;
    use std::sync::Mutex;
    #[derive(Clone)]
    struct Output(Arc<Mutex<Vec<u8>>>);
    impl Write for Output {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let output = Output(Arc::default());
    let sink = output.clone();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_writer(move || sink.clone())
        .finish();
    let _subscriber = tracing::subscriber::set_default(subscriber);
    let caller = tracing::info_span!("private_caller", identity = "private-ancestor-marker");
    let _entered = caller.enter();
    let authority = Authority::with_store(
        OwnershipToken {
            agent_id: AgentId("private-agent-marker".into()),
            instance_id: "private-instance-marker".into(),
            epoch: 7,
            protocol: PROTOCOL_VERSION,
        },
        tokio::time::Instant::now() + Duration::from_secs(15),
        Arc::new(super::super::test_support::MemoryOwnershipStore::default()),
    );
    authority
        .record_failure(OwnershipFailure::http(Phase::Challenge, 404))
        .await;
    assert!(authority.is_current());
    let bytes = output.0.lock().unwrap().clone();
    let event = String::from_utf8(bytes.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .find(|event| event["fields"]["message"] == "Receiver ownership negotiation failed")
        .unwrap();
    assert_eq!(event["fields"]["phase"], "challenge");
    assert_eq!(event["fields"]["reason"], "missing_route");
    assert_eq!(event["fields"]["http_status"], 404);
    assert_eq!(event["fields"]["epoch"], 7);
    for key in ["agent_sha256", "instance_sha256"] {
        assert_eq!(event["fields"][key].as_str().unwrap().len(), 64);
    }
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(!text.contains("private-ancestor-marker"), "{text}");
    assert!(!text.contains("private-agent-marker"));
    assert!(!text.contains("private-instance-marker"));
    TransportAuthority::revoke(authority.as_ref());
    authority
        .record_failure(OwnershipFailure::http(Phase::Revoke, 403))
        .await;
    assert_eq!(*output.0.lock().unwrap(), bytes);
    assert_eq!(
        authority.receiver_failure(),
        Some(OwnershipFailure::http(Phase::Challenge, 404))
    );
}

#[tokio::test]
async fn safe_diagnostic_events_exclude_ancestors_in_text_json_and_selective_filters() {
    use std::io::Write;
    use std::sync::Mutex;
    use tracing_subscriber::{layer::SubscriberExt, Layer};
    #[derive(Clone, Default)]
    struct Output(Arc<Mutex<Vec<u8>>>);
    impl Write for Output {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    for json in [false, true] {
        for suppress_root in [false, true] {
            for global in [false, true] {
                let output = Output::default();
                let sink = output.clone();
                let filter = tracing_subscriber::filter::filter_fn(move |metadata| {
                    !suppress_root || metadata.name() != "receiver_ownership"
                });
                let layer = tracing_subscriber::fmt::layer()
                    .without_time()
                    .with_ansi(false)
                    .with_writer(move || sink.clone());
                let subscriber: Box<dyn tracing::Subscriber + Send + Sync> = match (json, global) {
                    (true, true) => Box::new(
                        tracing_subscriber::registry()
                            .with(layer.json())
                            .with(filter),
                    ),
                    (false, true) => {
                        Box::new(tracing_subscriber::registry().with(layer).with(filter))
                    }
                    (true, false) => Box::new(
                        tracing_subscriber::registry().with(layer.json().with_filter(filter)),
                    ),
                    (false, false) => {
                        Box::new(tracing_subscriber::registry().with(layer.with_filter(filter)))
                    }
                };
                let _subscriber = tracing::subscriber::set_default(subscriber);
                let caller = tracing::info_span!("caller", secret = "private-ancestor-marker");
                let _entered = caller.enter();
                let fixture = Fixture::new(false).await;
                fixture.store.inner.partition(true);
                assert!(fixture
                    .cancelled
                    .check_for_phase(Phase::Challenge)
                    .await
                    .is_err());
                let success = Fixture::new(false).await;
                success
                    .cancelled
                    .record_failure(OwnershipFailure::http(Phase::Challenge, 404))
                    .await;
                let text = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
                assert!(!text.contains("private-ancestor-marker"), "{text}");
                if suppress_root {
                    if global {
                        assert!(text.is_empty(), "{text}");
                    }
                } else {
                    assert!(
                        text.contains("Receiver ownership operation failed"),
                        "{text}"
                    );
                    assert!(
                        text.contains("Receiver ownership negotiation failed"),
                        "{text}"
                    );
                    assert!(
                        text.contains("Receiver ownership failure durably recorded"),
                        "{text}"
                    );
                    assert!(
                        text.contains("Receiver ownership failure persistence unconfirmed"),
                        "{text}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn diagnostic_lineage_observes_only_the_exact_agent_and_survives_cancellation() {
    use super::super::receiver_diagnostics::{observe_agent_sandbox, Correlation};
    use crate::SandboxId;

    let fixture = Fixture::new(false).await;
    let authority = &fixture.cancelled;
    let agent = &authority.token.agent_id;
    let sandbox = SandboxId("first-sandbox".into());
    let other = SandboxId("other-sandbox".into());

    observe_agent_sandbox(agent, &other);
    assert!(authority.receiver_correlation().sandbox_sha256.is_none());
    authority
        .run(async {
            observe_agent_sandbox(&AgentId("other-agent".into()), &other);
            assert!(authority.receiver_correlation().sandbox_sha256.is_none());
            observe_agent_sandbox(agent, &sandbox);
            observe_agent_sandbox(agent, &other);
        })
        .await
        .unwrap();
    TransportAuthority::revoke(authority.as_ref());

    let expected = Correlation::new(agent, Some(sandbox.as_str()), &authority.token.instance_id);
    let retained = authority.receiver_correlation();
    assert_eq!(retained.sandbox_sha256, expected.sandbox_sha256);
    assert_eq!(retained.epoch, Some(authority.token.epoch));
}
