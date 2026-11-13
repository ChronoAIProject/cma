use super::*;

fn with_origin_channel(fixture: &Fixture, mut record: Connection) -> Connection {
    use cmaeg_core::{channel::ChannelDefinition, seal::ChannelPointer};
    let mut raw: Value = serde_json::from_str(include_str!(
        "../../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    raw["record_id"] = uuid::Uuid::new_v4().to_string().into();
    raw["sources"] = json!([{"type":"nyxid_relay","issuer":"https://identity.example","jwks_url":"https://identity.example/jwks",
        "key_id":"fixture-key","route_ids":["fixture-route"],"platform":"telegram","admission":{"type":"all"}}]);
    raw["partition"]["key"] = "conversation_and_sender".into();
    let mut definition: ChannelDefinition = serde_json::from_value(raw).unwrap();
    definition.profile_digest = definition.profile.digest();
    let pointer = ChannelPointer {
        v: 1,
        record_id: definition.record_id.clone(),
        record_url: fixture
            .records()
            .record_url(definition.record_id.as_str())
            .unwrap()
            .to_string(),
    };
    record.origin.event.channel_id = pointer.seal_id(&fixture.keys).unwrap();
    record.origin.event.record_id = definition.record_id.clone();
    record.origin.binding_id = definition.provider.binding_id.to_string();
    record.origin.agent_key_hash = definition.secrets.agent_key_hash.clone();
    record.origin.event.partition_key = format!(
        "{}\u{1f}{}",
        record.origin.conversation.id, record.origin.actor
    );
    record.origin.source = SourceKind::NyxidRelay {
        platform: Platform::Telegram,
        route_id: "fixture-route".into(),
    };
    fixture.peer.channels.lock().unwrap().insert(
        definition.record_id.to_string(),
        Retained {
            owner: definition.owner.subject.clone(),
            version: 1,
            blob: STANDARD.encode(
                seal_blob(
                    &fixture.keys,
                    definition.to_blob_json().to_string().as_bytes(),
                )
                .unwrap(),
            ),
        },
    );
    record
}

#[derive(Default)]
pub(super) struct RecordingLauncher {
    pub(super) requests: Mutex<Vec<crate::ingress::launch::LaunchRequest>>,
    observed: tokio::sync::Notify,
}
impl crate::ingress::launch::TurnLauncher for RecordingLauncher {
    fn launch(
        &self,
        request: crate::ingress::launch::LaunchRequest,
    ) -> futures_util::future::BoxFuture<'_, crate::ingress::launch::LaunchOutcome> {
        Box::pin(async move {
            use crate::ingress::launch::LaunchOutcome;
            let mut requests = self.requests.lock().unwrap();
            requests.push(request);
            let result = match requests.len() {
                1 => LaunchOutcome::Deferred,
                2 => LaunchOutcome::Refused(cmaeg_core::problem::Problem::new(
                    cmaeg_core::problem::ProblemCode::ProviderUnavailable,
                    503,
                    "fixture provider refusal",
                )),
                _ => LaunchOutcome::Accepted {
                    response_id: "fixture-accepted-turn".into(),
                },
            };
            self.observed.notify_one();
            result
        })
    }
}

#[tokio::test]
async fn completion_outbox_replays_stable_system_event_after_restart_and_only_acks_acceptance() {
    use cmaeg_core::activity::{ActivityKind, ActorKind};
    let fixture = Fixture::new().await;
    let mut record = with_origin_channel(&fixture, connection("alice", false));
    record.status = Status::Ready;
    fixture
        .store()
        .save_connection(&record, None)
        .await
        .unwrap();
    let launcher = Arc::new(RecordingLauncher::default());
    let mut reference = None;
    for attempt in 1..=3 {
        let worker = fixture.service().start_completion_worker(launcher.clone());
        tokio::time::timeout(Duration::from_secs(3), launcher.observed.notified())
            .await
            .unwrap();
        if attempt == 3 {
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    if fixture
                        .store()
                        .connection(&record.id)
                        .await
                        .unwrap()
                        .0
                        .notified
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
        }
        drop(worker);
        let (stored, _) = fixture.store().connection(&record.id).await.unwrap();
        assert_eq!(stored.notified, attempt == 3);
        let durable = stored.completion_ref.unwrap();
        match &reference {
            Some(previous) => assert_eq!(&durable, previous),
            None => reference = Some(durable),
        }
    }
    let requests = launcher.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    let first = &requests[0];
    assert_eq!(first.activity.actor.kind, ActorKind::System);
    assert_eq!(first.activity.actor.id, record.origin.actor);
    assert_eq!(first.activity.conversation, record.origin.conversation);
    assert_eq!(first.conversation_id, record.origin.event.conversation_id);
    let ActivityKind::Event { ce, .. } = &first.activity.kind else {
        panic!("completion must be a system event");
    };
    assert_eq!(ce.r#type, "gateway.connection.ready");
    assert_eq!(
        ce.data,
        Some(json!({"connection_ref":record.id,"status":"ready"}))
    );
    let serialized = serde_json::to_string(&first.activity).unwrap();
    for secret in [
        CHILD_TOKEN,
        WEBHOOK_SECRET,
        REPLY_TOKEN,
        &record.setup_nonce,
    ] {
        assert!(!serialized.contains(secret));
    }
    for request in &requests[1..] {
        assert_eq!(request.activity, first.activity);
        assert_eq!(request.event_ref, first.event_ref);
        assert_eq!(request.idempotency_key, first.idempotency_key);
        assert_eq!(request.conversation_id, first.conversation_id);
    }
    assert_eq!(Some(first.event_ref.as_str()), reference.as_deref());
}

#[tokio::test]
async fn completion_cannot_cross_origin_binding_agent_key_partition_or_connection() {
    let fixture = Fixture::new().await;
    let service = fixture.service();
    let mut record = with_origin_channel(&fixture, connection("alice", false));
    record.status = Status::Ready;
    let request = service.completion_request(&record).await.unwrap();
    record.completion_ref = Some(request.event_ref.as_str().into());
    let launcher = RecordingLauncher::default();
    for field in ["binding", "key", "partition", "connection"] {
        let mut other = record.clone();
        match field {
            "binding" => other.origin.binding_id = "another-provider-binding".into(),
            "key" => other.origin.agent_key_hash = "another-provider-key".into(),
            "partition" => other.origin.event.partition_key = "another-user-partition".into(),
            _ => other.id = uuid::Uuid::new_v4().to_string(),
        }
        assert!(
            service.notify_completion(&other, &launcher).await.is_err(),
            "changed {field} was accepted"
        );
    }
    assert!(launcher.requests.lock().unwrap().is_empty());
}
