//! Durable retained-route contracts through the native admission seam.
use super::{
    launch::{LaunchOutcome, UnavailableLauncher},
    routed::RoutedIngress,
    routing_plan::fixture,
    routing_store::{test_server::Server, EventIdentity},
    IngressState,
};
use crate::{
    channels::resolver::{ChannelResolver, ResolvedChannel},
    native_lark::{
        message,
        receipts::{Receipt, Store, RETENTION},
        test_support::{credentials, reaction, recall, source, text},
    },
    turns::routed::scheduler::{Disposition, ScheduleConfig, Scheduler, TargetExecutor, WorkItem},
};
use cmaeg_core::{
    activity::{ActivityKind, ActorKind},
    channel::{NativeLarkSource, Secret, Source},
    ids::ConsumerId,
    seal::ReplyAddress,
};
use futures_util::future::BoxFuture;
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    sync::{atomic::Ordering, Arc},
    time::Instant,
};
struct Executor;
impl TargetExecutor for Executor {
    fn execute<'a>(&'a self, _: &'a WorkItem) -> BoxFuture<'a, Disposition> {
        Box::pin(async { Disposition::Settled })
    }
}
struct Setup {
    server: Server,
    state: IngressState,
    channel: Arc<ResolvedChannel>,
    _scheduler: Scheduler,
}
impl Setup {
    async fn new() -> Self {
        let request = fixture::request(
            |v| {
                fixture::targets(v);
                v["sources"] = json!([source()]);
                let c = credentials();
                v["secrets"]["native_lark"] = json!({"app_secret":c.app_secret.as_str(),"encrypt_key":c.encrypt_key.as_str(),"verification_token":c.verification_token.as_str()});
            },
            fixture::message("unused"),
        );
        let server = Server::start().await;
        let keys = Arc::new(fixture::keys());
        let (scheduler, handle) = Scheduler::new(
            Arc::new(server.store()),
            Arc::new(Executor),
            ScheduleConfig::default(),
        )
        .unwrap();
        let routing = RoutedIngress::new(
            server.store(),
            handle,
            keys.clone(),
            BTreeSet::new(),
            BTreeSet::from(["cma_codex".into(), "nyxbot".into()]),
            BTreeSet::from(["cma_codex".into(), "nyxbot".into()]),
        )
        .with_clock(Arc::new(|| fixture::NOW));
        let state = IngressState::new(
            ChannelResolver::new(
                Arc::new(server.records.clone()),
                keys.clone(),
                Default::default(),
            ),
            keys,
            Default::default(),
            Arc::new(UnavailableLauncher),
        )
        .unwrap()
        .with_routed(Some(Arc::new(routing)))
        .with_native_lark_intake(true);
        Self {
            server,
            state,
            channel: request.channel,
            _scheduler: scheduler,
        }
    }
    fn source() -> NativeLarkSource {
        let Source::NativeLark(s) = source() else {
            panic!()
        };
        s
    }
    fn identity(&self, raw: &Value) -> EventIdentity {
        EventIdentity {
            owner: self.channel.definition.owner.subject.clone(),
            channel_id: self.channel.id.clone(),
            record_id: self.channel.definition.record_id.clone(),
            event_id: message::extract(&Self::source(), raw, fixture::NOW)
                .unwrap()
                .unwrap()
                .event_id(&Self::source().identity)
                .unwrap(),
        }
    }
    async fn launch(&self, raw: Value) -> LaunchOutcome {
        self.launch_in(raw, self.channel.clone()).await
    }
    async fn launch_in(&self, raw: Value, channel: Arc<ResolvedChannel>) -> LaunchOutcome {
        let msg = message::extract(&Self::source(), &raw, fixture::NOW)
            .unwrap()
            .unwrap();
        self.state
            .launch_native_lark(
                channel,
                Self::source(),
                msg,
                Instant::now(),
                fixture::NOW,
                self.state.inflight.clone().try_acquire_owned().unwrap(),
            )
            .await
    }
    async fn plan(&self, raw: &Value) -> super::routing_store::AdmittedPlan {
        self.server
            .store()
            .load(&self.identity(raw))
            .await
            .unwrap()
            .unwrap()
    }
}
fn admitted(result: LaunchOutcome) {
    assert!(
        matches!(result, LaunchOutcome::Admitted { .. }),
        "{result:?}"
    )
}
#[tokio::test]
async fn native_lark_duplicate_concurrent_receive_is_one_plan_and_changed_text_conflicts() {
    let setup = Setup::new().await;
    let raw = text("om_original", "hello", fixture::NOW);
    let (a, b) = tokio::join!(setup.launch(raw.clone()), setup.launch(raw.clone()));
    admitted(a);
    admitted(b);
    let before = setup.server.memory.writes.load(Ordering::SeqCst);
    let mut retry = raw.clone();
    retry["header"]["event_id"] = "other_delivery".into();
    admitted(setup.launch(retry).await);
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), before);
    assert_eq!(
        setup
            .server
            .store()
            .recover_all_page(None, 32)
            .await
            .unwrap()
            .plans
            .len(),
        1
    );
    let plan = setup.plan(&raw).await;
    assert_eq!(plan.targets.len(), 3);
    assert_eq!(plan.activity.actor.kind, ActorKind::Human);
    assert!(!String::from_utf8(plan.source_body)
        .unwrap()
        .contains(credentials().verification_token.as_str()));
    assert!(
        matches!(setup.launch(text("om_original","changed",fixture::NOW)).await,LaunchOutcome::Refused(p) if p.status==409)
    );
}
#[tokio::test]
async fn native_lark_recall_and_reaction_keep_original_consumers_partition_and_conversation() {
    let setup = Setup::new().await;
    let raw = text("om_original", "hello", fixture::NOW);
    admitted(setup.launch(raw.clone()).await);
    let original = setup.plan(&raw).await;
    for event in [
        recall("recall_delivery", "om_original", fixture::NOW),
        reaction("reaction_add", "om_original", true, fixture::NOW),
        reaction("reaction_remove", "om_original", false, fixture::NOW),
    ] {
        let mut channel = setup.channel.clone();
        Arc::make_mut(&mut channel)
            .definition
            .routing
            .as_mut()
            .unwrap()
            .default_targets = vec![ConsumerId::default_consumer()];
        admitted(setup.launch_in(event.clone(), channel).await);
        let plan = setup.plan(&event).await;
        assert_eq!(plan.targets.len(), 3);
        for (a, b) in original.targets.iter().zip(&plan.targets) {
            assert_eq!(a.launch.consumer, b.launch.consumer);
            assert_eq!(a.launch.provider, b.launch.provider);
            assert_eq!(a.launch.event.partition_key, b.launch.event.partition_key);
            assert_eq!(
                a.launch.event.conversation_id,
                b.launch.event.conversation_id
            );
        }
        let ActivityKind::Event { ce, .. } = &plan.activity.kind else {
            panic!()
        };
        let data = ce.data.as_ref().unwrap();
        assert_eq!(
            data["original_event_id"],
            original.identity.event_id.to_string()
        );
        assert!(data.get("old_reactions").is_none());
        assert!(data.get("new_reactions").is_none());
        if ce.r#type == "cma.message.deleted.v1" {
            assert_eq!(plan.activity.actor.kind, ActorKind::Unknown);
            assert_eq!(plan.activity.actor.id, "unknown");
        } else {
            assert_eq!(plan.activity.actor.kind, ActorKind::Human);
            assert_eq!(data["reaction"]["emoji_type"], "SMILE");
            assert!(matches!(data["operation"].as_str(), Some("add" | "remove")));
        }
        let writes = setup.server.memory.writes.load(Ordering::SeqCst);
        admitted(setup.launch(event).await);
        assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), writes);
    }
}
#[tokio::test]
async fn native_lark_revoked_credentials_consumers_and_cross_chat_recall_never_admit() {
    let setup = Setup::new().await;
    let original = text("om_original", "hello", fixture::NOW);
    admitted(setup.launch(original).await);
    for mutation in ["secret", "source", "consumer", "provider"] {
        let mut channel = setup.channel.clone();
        let d = &mut Arc::make_mut(&mut channel).definition;
        match mutation {
            "secret" => {
                d.secrets.native_lark.as_mut().unwrap().encrypt_key =
                    Secret::new("changed-fixture-encrypt-key")
            }
            "source" => d.sources.clear(),
            "consumer" => d.routing.as_mut().unwrap().consumers.clear(),
            "provider" => {
                d.provider.binding_id = cmaeg_core::ids::BindingId::new("bnd_other").unwrap()
            }
            _ => {}
        }
        assert!(matches!(
            setup
                .launch_in(
                    reaction("revoked_event", "om_original", true, fixture::NOW),
                    channel
                )
                .await,
            LaunchOutcome::Refused(_)
        ));
    }
    let mut cross = recall("cross_chat", "om_original", fixture::NOW);
    cross["event"]["chat_id"] = "oc_other".into();
    assert!(matches!(setup.launch(cross).await,LaunchOutcome::Refused(p) if p.status==403));
}
#[tokio::test]
async fn native_lark_confirmed_receipt_selects_only_sending_consumer_and_retains_pending_retry() {
    let setup = Setup::new().await;
    let raw = text("om_original", "hello", fixture::NOW);
    admitted(setup.launch(raw.clone()).await);
    let original = setup.plan(&raw).await;
    let owner = &original.targets[0].launch;
    let ReplyAddress::NativeLark(mut reply) = owner.event.reply.clone() else {
        panic!()
    };
    reply.message_id = "om_sent".into();
    let receipt = Receipt {
        kind: "native_lark_receipt_v1".into(),
        original: original.identity.clone(),
        source: original.activity.source.clone(),
        recipient: reply.chat_id.clone(),
        reply,
        message_id: "om_sent".into(),
        consumer: owner.consumer.clone(),
        provider: owner.provider.clone(),
        partition_key: owner.event.partition_key.clone(),
        conversation_id: owner.event.conversation_id.clone(),
        confirmed_at: fixture::NOW,
        retained_until: fixture::NOW + RETENTION,
    };
    let event = reaction("outbound_event", "om_sent", true, fixture::NOW);
    assert!(matches!(setup.launch(event.clone()).await,LaunchOutcome::Refused(p) if p.status==503));
    Store::new(setup.server.records.clone(), Arc::new(fixture::keys()))
        .put(&receipt)
        .await
        .unwrap();
    *setup.server.memory.unavailable_read_id.lock().unwrap() =
        Some(original.identity.admission_id());
    assert!(matches!(setup.launch(event.clone()).await,LaunchOutcome::Refused(p) if p.status==503));
    *setup.server.memory.unavailable_read_id.lock().unwrap() = None;
    admitted(setup.launch(event.clone()).await);
    let admitted = setup.plan(&event).await;
    assert_eq!(admitted.targets.len(), 1);
    assert_eq!(admitted.targets[0].launch.consumer, owner.consumer);
    let ActivityKind::Event { ce, .. } = admitted.activity.kind else {
        panic!()
    };
    assert_eq!(ce.data.unwrap()["original_author"]["kind"], "bot");
    let stale = reaction("missing_event", "om_unseen", true, fixture::NOW - 121);
    assert!(matches!(setup.launch(stale).await,LaunchOutcome::Refused(p) if p.status==422));
}
