use super::*;
use crate::{
    channels::{resolver::ChannelResolver, routing_fixture},
    ingress::{
        launch::{LaunchOutcome, TurnLauncher},
        routed::RoutedIngress,
        routing_store::{
            test_server::{Row, Server},
            EventIdentity,
        },
    },
    turns::routed::scheduler::{Disposition, ScheduleConfig, Scheduler, TargetExecutor, WorkItem},
};
use futures_util::future::BoxFuture;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(super) struct Legacy(pub AtomicUsize);
impl TurnLauncher for Legacy {
    fn launch(&self, _: LaunchRequest) -> BoxFuture<'_, LaunchOutcome> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            LaunchOutcome::Accepted {
                response_id: "legacy".into(),
            }
        })
    }
}
struct Executor;
impl TargetExecutor for Executor {
    fn execute<'a>(&'a self, _: &'a WorkItem) -> BoxFuture<'a, Disposition> {
        Box::pin(async { Disposition::Settled })
    }
}
pub(super) struct Fixture {
    pub server: Server,
    pub state: IngressState,
    pub channel: Arc<ResolvedChannel>,
    pub raw: Value,
    pub now: i64,
    pub legacy: Arc<Legacy>,
    _scheduler: Scheduler,
}
impl Fixture {
    pub async fn new(group: bool) -> Self {
        let server = Server::start_realtime().await;
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let keys = Arc::new(crate::ingress::routing_plan::fixture::keys());
        let mut value = routing_fixture::routed();
        value["record_id"] = "11111111-1111-4111-8111-111111111111".into();
        value["sources"] = json!([routing_fixture::telegram()]);
        value["sources"][0]["message_events"] = true.into();
        value["routing"]["default_targets"] = json!([routing_fixture::REVIEWER, "default"]);
        value["partition"]["key"] = "conversation_and_sender".into();
        if group {
            value["sources"][0]["admission"] = json!({"type":"scoped","senders":{"type":"open"},"chats":{"type":"open"},"groups":"mention"});
        }
        let definition = routing_fixture::definition(value);
        definition.validate().unwrap();
        let pointer = cmaeg_core::seal::ChannelPointer {
            v: 1,
            record_id: definition.record_id.clone(),
            record_url: server
                .records
                .record_url(definition.record_id.as_str())
                .unwrap()
                .to_string(),
        };
        let channel = Arc::new(ResolvedChannel {
            id: pointer.seal_id(&keys).unwrap(),
            pointer,
            definition,
            agent_key_hash: [7; 32],
            consumer_keys: vec![],
            loaded_at: Instant::now(),
            version: 1,
        });
        let (scheduler, handle) = Scheduler::new(
            Arc::new(server.store()),
            Arc::new(Executor),
            ScheduleConfig::default(),
        )
        .unwrap();
        let legacy = Arc::new(Legacy(AtomicUsize::new(0)));
        let state = IngressState::new(
            ChannelResolver::new(
                Arc::new(server.records.clone()),
                keys.clone(),
                Default::default(),
            ),
            keys.clone(),
            Default::default(),
            legacy.clone(),
        )
        .unwrap()
        .with_routed(Some(Arc::new(RoutedIngress::new(
            server.store(),
            handle,
            keys,
            BTreeSet::new(),
            BTreeSet::from(["cma_codex".into()]),
            BTreeSet::from(["cma_codex".into(), "nyxbot".into()]),
        ))));
        let mut raw = json!({"update_id":101,"message":{"message_id":5,"date":now-5,
            "chat":{"id":42,"type":"private"},"from":{"id":42,"is_bot":false,"first_name":"Alice"},"text":"original"}});
        if group {
            raw["message"]["chat"] = json!({"id":-100,"type":"supergroup"});
            raw["message"]["is_topic_message"] = true.into();
            raw["message"]["message_thread_id"] = 123.into();
            raw["message"]["text"] = "@golden_bot original".into();
            raw["message"]["entities"] = json!([{"type":"mention","offset":0,"length":11}]);
        }
        let f = Self {
            server,
            state,
            channel,
            raw,
            now,
            legacy,
            _scheduler: scheduler,
        };
        f.publish_channel();
        f
    }
    pub fn publish_channel(&self) {
        self.server.memory.channels.lock().unwrap().insert(
            self.channel.definition.record_id.to_string(),
            Row {
                owner: self.channel.definition.owner.subject.clone(),
                version: 1,
                expiry: self.now + 1000,
                blob: cmaeg_core::seal::seal_blob(
                    &self.state.master_keys,
                    &serde_json::to_vec(&self.channel.definition.to_blob_json()).unwrap(),
                )
                .unwrap(),
            },
        );
    }
    pub fn request(&self) -> LaunchRequest {
        let Outcome::Admitted(observed) = crate::ingress::telegram::observe(
            self.raw.clone(),
            "77",
            Some("golden_bot"),
            false,
            false,
        )
        .unwrap() else {
            panic!()
        };
        let crate::ingress::media_guard::Built::Launch(request) = crate::ingress::telegram::launch(
            self.channel.clone(),
            "77",
            *observed,
            &self.state.master_keys,
            Instant::now(),
            self.now,
            false,
        )
        .unwrap() else {
            panic!()
        };
        *request
    }
    pub async fn admit_original(&self) -> AdmittedPlan {
        let request = self.request();
        let context = Context::capture(&self.state, &request, self.now).unwrap();
        let body = serde_json::to_vec(&self.raw).unwrap();
        let source = self.state.capture_routed(&request, &body, true);
        let hold = self.state.inflight.clone().try_acquire_owned().unwrap();
        assert!(matches!(
            self.state.launch_verified(request, source, hold).await,
            LaunchOutcome::Admitted { .. }
        ));
        context.remember(&self.state, &self.channel).await.unwrap();
        self.plan("telegram:77:101").await.unwrap()
    }
    pub async fn plan(&self, event: &str) -> Option<AdmittedPlan> {
        self.server
            .store()
            .load(&EventIdentity {
                owner: self.channel.definition.owner.subject.clone(),
                channel_id: self.channel.id.clone(),
                record_id: self.channel.definition.record_id.clone(),
                event_id: EventId::new(event).unwrap(),
            })
            .await
            .unwrap()
    }
    pub fn edit(&self) -> Value {
        let mut message = self.raw["message"].clone();
        message["edit_date"] = self.now.into();
        message["text"] = "/to nyx edited".into();
        message.as_object_mut().unwrap().remove("entities");
        json!({"update_id":102,"edited_message":message})
    }
    pub fn reaction(&self) -> Value {
        json!({"update_id":103,"message_reaction":{"chat":self.raw["message"]["chat"],"message_id":5,
            "user":{"id":43,"is_bot":false,"first_name":"Reactor"},"date":self.now,
            "old_reaction":[{"type":"emoji","emoji":"👍"}],"new_reaction":[]}})
    }
    pub async fn verified(&self, raw: &Value) -> Result<VerifiedMessageEvent, Error> {
        prepare(
            &self.state,
            self.channel.clone(),
            "77",
            raw,
            &serde_json::to_vec(raw).unwrap(),
            Instant::now(),
            self.now,
        )
        .await
    }
    pub async fn launch(&self, raw: &Value) -> LaunchOutcome {
        let verified = self.verified(raw).await.unwrap();
        self.state
            .launch_message_event_verified(
                verified,
                self.state.inflight.clone().try_acquire_owned().unwrap(),
            )
            .await
    }
}
