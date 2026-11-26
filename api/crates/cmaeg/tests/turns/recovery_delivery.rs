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
    ingress::launch::{LaunchOutcome, LaunchRequest, TurnLauncher},
    turns::outcomes::TurnOutcome,
    TurnConfig,
};
use cmaeg_core::{
    channel::{Secret, Sink},
    seal::{open, EventRefPayload, MasterKeyRing, TokenKind},
};
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, Url};
use serde_json::json;
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot, Semaphore};

#[tokio::test]
async fn recovery_denied_before_get_retires_the_accepted_event() {
    assert_recovery_retires_accepted_event(true, false).await;
}

#[tokio::test]
async fn recovery_denied_before_delivery_retires_the_accepted_event() {
    assert_recovery_retires_accepted_event(false, false).await;
}

#[tokio::test]
async fn recovery_denied_with_a_full_queue_releases_the_turn_and_expires_its_event() {
    assert_recovery_retires_accepted_event(true, true).await;
}

async fn assert_recovery_retires_accepted_event(before_get: bool, full_queue: bool) {
    let turn_timeout = Duration::from_secs(10);
    let terminal_gate = Arc::new(Semaphore::new(0));
    let mut fixture = Fixture::new(
        FakeProviderConfig {
            frames: Some(vec![
                json!({"type":"response.created","response":{"id":"resp_fake"}}),
                json!({"type":"response.in_progress","response":{"id":"resp_fake"}}),
            ]),
            recovery_status: Some("completed"),
            terminal_gate: Some(terminal_gate.clone()),
            ..Default::default()
        },
        TurnConfig {
            turn_timeout,
            ..Default::default()
        },
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
    let turn = Arc::new(fixture.custom_recovery_request("retired-recovery"));

    let transport = Arc::new(Transport {
        authority: fixture.recovery_channel.clone(),
        remove_sink: false,
        resolutions: AtomicUsize::new(0),
        posts: Mutex::default(),
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
    let mut revoked = (*fixture.channel).clone();
    revoked.definition.secrets.provider_credential = None;
    let revoked = Arc::new(revoked);
    assert!(matches!(
        fixture.runner.launch((*turn).clone()).await,
        LaunchOutcome::Accepted { .. }
    ));
    let accepted = fixture.outcomes.recv().await.unwrap();
    assert!(matches!(
        &accepted,
        DeliveryCommand::StreamOutcome {
            outcome: TurnOutcome::Accepted { .. },
            ..
        }
    ));
    commands.send(accepted).await.unwrap();
    worker_barrier(&commands, &fixture.keys, &turn).await;
    if before_get {
        *fixture.recovery_channel.0.lock().unwrap() = Some(revoked.clone());
    }
    if full_queue {
        for _ in 0..fixture.delivery_commands.capacity() {
            fixture
                .delivery_commands
                .try_send(completion(turn.clone()))
                .unwrap();
        }
        assert_eq!(fixture.delivery_commands.capacity(), 0);
    }
    terminal_gate.add_permits(1);
    if full_queue {
        let deadline = turn.received_at + turn_timeout;
        assert!(fixture.tracker.drain(deadline).await);
        assert!(fixture.tracker.is_empty());
        assert!(Instant::now() < deadline);
        assert_eq!(fixture.delivery_commands.capacity(), 0);
    } else {
        let denied = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let command = fixture.outcomes.recv().await.unwrap();
                if matches!(
                    &command,
                    DeliveryCommand::RetireStream { .. }
                        | DeliveryCommand::StreamOutcome {
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
            matches!(&denied, DeliveryCommand::RetireStream { .. }),
            before_get
        );
        *fixture.recovery_channel.0.lock().unwrap() = Some(revoked);
        commands.send(denied).await.unwrap();
        worker_barrier(&commands, &fixture.keys, &turn).await;
        assert_eq!(transport.posts.lock().unwrap().len(), 0);

        *fixture.recovery_channel.0.lock().unwrap() = Some(fixture.channel.clone());
        commands.send(completion(turn.clone())).await.unwrap();
        worker_barrier(&commands, &fixture.keys, &turn).await;
        assert_eq!(transport.posts.lock().unwrap().len(), 0);
    }
    assert_eq!(
        fixture
            .fake
            .log()
            .iter()
            .filter(|call| call.method == "GET")
            .count(),
        usize::from(!before_get)
    );
    worker_barrier(&commands, &fixture.keys, &turn).await;
    assert_eq!(transport.posts.lock().unwrap().len(), 0);

    let mut fresh = (*turn).clone();
    Arc::make_mut(&mut fresh.channel).definition.reply.sinks = vec![Sink::ResultWebhook {
        url: "https://result.example/current".into(),
        hmac_key_id: "key-1".into(),
    }];
    let other = Arc::new(fixture.custom_recovery_request("another-event"));
    fixture.fake.close().await;
    tokio::time::pause();
    let retention = Duration::from_secs(31);
    if full_queue {
        tokio::time::sleep_until((turn.received_at + turn_timeout + retention).into()).await;
    } else {
        tokio::time::sleep(retention).await;
    }
    let fresh = Arc::new(fresh);
    *fixture.recovery_channel.0.lock().unwrap() = Some(fresh.channel.clone());
    commands.send(completion(fresh)).await.unwrap();
    worker_barrier(&commands, &fixture.keys, &turn).await;
    assert_eq!(&*transport.posts.lock().unwrap(), &["/current"]);
    commands.send(completion(other.clone())).await.unwrap();
    worker_barrier(&commands, &fixture.keys, &other).await;
    assert_eq!(transport.posts.lock().unwrap().len(), 2);
    drop(commands);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    assert!(engine.drain(deadline.into_std()).await);
}

fn completion(turn: Arc<LaunchRequest>) -> DeliveryCommand {
    DeliveryCommand::StreamOutcome {
        turn,
        outcome: TurnOutcome::Completed {
            final_text: Some("test completion".into()),
        },
        answer_response_id: None,
        accepted_deadline: None,
        recovery: None,
    }
}

async fn worker_barrier(
    commands: &mpsc::Sender<DeliveryCommand>,
    keys: &MasterKeyRing,
    turn: &Arc<LaunchRequest>,
) {
    let event = open::<EventRefPayload>(keys, TokenKind::EventRef, turn.event_ref.as_str())
        .unwrap()
        .payload;
    let (receipt, completed) = oneshot::channel();
    commands
        .send(DeliveryCommand::Typing {
            trace: tracing::Span::none(),
            channel: turn.channel.clone(),
            event,
            receipt,
        })
        .await
        .unwrap();
    completed.await.unwrap();
}

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
            posts: Mutex::default(),
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
            transport.posts.lock().unwrap().len(),
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
    posts: Mutex<Vec<String>>,
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
        url: Url,
        _addresses: Vec<SocketAddr>,
        _headers: HeaderMap,
        _body: Arc<Vec<u8>>,
    ) -> BoxFuture<'_, Result<u16, TransportError>> {
        Box::pin(async move {
            self.posts.lock().unwrap().push(url.path().into());
            Ok(200)
        })
    }
}
