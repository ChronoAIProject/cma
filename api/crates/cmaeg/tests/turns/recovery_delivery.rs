use super::{
    fake_provider::FakeProviderConfig,
    support::{Fixture, TestRecoveryChannel},
};
use cmaeg::{
    delivery::{
        custom::CustomDelivery,
        engine::DeliveryEngine,
        intents::DeliveryCommand,
        relay_client::RelayClient,
        webhook_client::{TransportError, WebhookClient, WebhookTransport},
    },
    ingress::launch::{LaunchOutcome, TurnLauncher},
    turns::outcomes::TurnOutcome,
};
use cmaeg_core::channel::{Secret, Sink};
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, Url};
use serde_json::json;
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, Semaphore};

#[tokio::test]
async fn recovered_webhook_rechecks_current_sink_after_dns_before_post() {
    for remove_sink in [false, true] {
        let mut fixture = Fixture::new(
            FakeProviderConfig {
                disconnect_after: Some(2),
                recovery_status: Some("completed"),
                ..Default::default()
            },
            Default::default(),
            4,
        )
        .await;
        let mut channel = (*fixture.channel).clone();
        channel.definition.reply.sinks = vec![Sink::ResultWebhook {
            url: "https://result.example/hook".into(),
            hmac_key_id: "key-1".into(),
        }];
        channel
            .definition
            .secrets
            .hmac_secrets
            .push(("key-1".into(), Secret::new("test-signing-value")));
        fixture.channel = Arc::new(channel);
        *fixture.recovery_channel.0.lock().unwrap() = Some(fixture.channel.clone());
        assert!(matches!(
            fixture
                .runner
                .launch(fixture.custom_recovery_request("recovered-delivery"))
                .await,
            LaunchOutcome::Accepted { .. }
        ));
        let command = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let command = fixture.outcomes.recv().await.unwrap();
                if matches!(
                    &command,
                    DeliveryCommand::StreamOutcome {
                        outcome: TurnOutcome::Completed { .. },
                        recovery: Some(_),
                        ..
                    }
                ) {
                    break command;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(
            fixture
                .fake
                .log()
                .iter()
                .filter(|call| call.method == "GET")
                .count(),
            1
        );
        let transport = Arc::new(Transport {
            authority: fixture.recovery_channel.clone(),
            remove_sink,
            resolutions: AtomicUsize::new(0),
            posts: AtomicUsize::new(0),
        });
        let mut custom = CustomDelivery::new("https://gateway.example".into(), Arc::default());
        custom.webhooks = WebhookClient::with_transport(transport.clone());
        let (commands, receiver) = mpsc::channel(4);
        let engine = DeliveryEngine::start(
            receiver,
            fixture.keys.clone(),
            RelayClient::new("https://relay.example", Duration::from_secs(1)).unwrap(),
            Default::default(),
            fixture.tracker.clone(),
            custom,
        );
        commands.send(command).await.unwrap();
        drop(commands);
        assert!(engine.drain(Instant::now() + Duration::from_secs(5)).await);
        assert_eq!(transport.resolutions.load(Ordering::SeqCst), 1);
        assert_eq!(
            transport.posts.load(Ordering::SeqCst),
            usize::from(!remove_sink)
        );
        assert_eq!(fixture.fake.requests().len(), 1);
        fixture.fake.close().await;
    }
}

#[tokio::test]
async fn recovery_refuses_a_removed_credential_before_get() {
    let terminal_gate = Arc::new(Semaphore::new(0));
    let fixture = Fixture::new(
        FakeProviderConfig {
            disconnect_after: Some(2),
            recovery_status: Some("completed"),
            terminal_gate: Some(terminal_gate.clone()),
            ..Default::default()
        },
        Default::default(),
        4,
    )
    .await;
    assert!(matches!(
        fixture
            .runner
            .launch(fixture.custom_recovery_request("removed-credential"))
            .await,
        LaunchOutcome::Accepted { .. }
    ));
    let mut current = (*fixture.channel).clone();
    current.definition.secrets.provider_credential = None;
    *fixture.recovery_channel.0.lock().unwrap() = Some(Arc::new(current));
    terminal_gate.add_permits(1);
    assert!(
        fixture
            .tracker
            .drain(Instant::now() + Duration::from_secs(5))
            .await
    );
    assert_eq!(fixture.fake.log().len(), 1);
    fixture.fake.close().await;
}

#[tokio::test]
async fn recovery_preserves_terminal_reasons_and_pins_the_in_progress_agent() {
    for (snapshot, replay, wrong_agent) in [
        ("failed", None, false),
        ("incomplete", None, false),
        ("in_progress", Some("failed"), false),
        ("in_progress", Some("incomplete"), false),
        ("completed", None, true),
    ] {
        let agent = if wrong_agent { "agt_other" } else { "agt_test" };
        let mut fixture = Fixture::new(
            FakeProviderConfig {
                frames: Some(vec![json!({
                    "type":"response.in_progress",
                    "response":{"id":"resp_fake", "conversation":{"id":agent}}
                })]),
                recovery_status: Some(snapshot),
                recovery_replay_status: replay,
                ..Default::default()
            },
            Default::default(),
            4,
        )
        .await;
        assert!(matches!(
            fixture
                .runner
                .launch(fixture.custom_recovery_request("terminal-reason"))
                .await,
            LaunchOutcome::Accepted { .. }
        ));
        let outcomes = fixture.terminal().await;
        match (replay.unwrap_or(snapshot), outcomes.last()) {
            ("failed", Some(TurnOutcome::Failed { code, .. })) => {
                assert_eq!(code, "provider_test_failed")
            }
            ("incomplete", Some(TurnOutcome::Incomplete { reason })) => {
                assert_eq!(reason, "test_incomplete")
            }
            ("completed", Some(TurnOutcome::Failed { code, .. })) if wrong_agent => {
                assert_eq!(code, "stream_lost")
            }
            _ => panic!("unexpected terminal outcome: {outcomes:?}"),
        }
        assert_eq!(fixture.fake.requests().len(), 1);
        assert_eq!(fixture.fake.log().len(), 2 + usize::from(replay.is_some()));
        fixture.fake.close().await;
    }
}

struct Transport {
    authority: Arc<TestRecoveryChannel>,
    remove_sink: bool,
    resolutions: AtomicUsize,
    posts: AtomicUsize,
}

impl WebhookTransport for Transport {
    fn resolve(&self, _url: Url) -> BoxFuture<'_, Result<Vec<SocketAddr>, TransportError>> {
        Box::pin(async {
            self.resolutions.fetch_add(1, Ordering::SeqCst);
            let mut authority = self.authority.0.lock().unwrap();
            let mut channel = (**authority.as_ref().unwrap()).clone();
            channel.version += 1;
            channel.definition.name = "Renamed channel".into();
            if self.remove_sink {
                channel.definition.reply.sinks.clear();
            }
            *authority = Some(Arc::new(channel));
            Ok(vec!["8.8.8.8:443".parse().unwrap()])
        })
    }

    fn post(
        &self,
        _url: Url,
        _addresses: Vec<SocketAddr>,
        _headers: HeaderMap,
        _body: Arc<Vec<u8>>,
    ) -> BoxFuture<'_, Result<u16, TransportError>> {
        Box::pin(async {
            self.posts.fetch_add(1, Ordering::SeqCst);
            Ok(200)
        })
    }
}
