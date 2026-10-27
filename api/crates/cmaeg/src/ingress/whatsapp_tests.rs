use super::{
    launch::{LaunchOutcome, UnavailableLauncher},
    routed::RoutedIngress,
    routing_plan::fixture,
    routing_store::{test_server::Server, EventIdentity},
    whatsapp::VerifiedWhatsappMessage,
    IngressState,
};
#[path = "whatsapp_http_tests.rs"]
mod http_tests;
#[path = "whatsapp_media_tests.rs"]
mod media_tests;
use crate::{
    channels::resolver::ChannelResolver,
    turns::routed::scheduler::{Disposition, ScheduleConfig, Scheduler, TargetExecutor, WorkItem},
    whatsapp::{
        receipts::{Receipt, Store, RETENTION},
        tests::{credentials, envelope, reaction, source, text},
        webhook,
    },
};
use cmaeg_core::{
    activity::{ActivityKind, ActorKind},
    ids::ConsumerId,
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
    channel: Arc<crate::channels::resolver::ResolvedChannel>,
    _scheduler: Scheduler,
}
impl Setup {
    async fn new() -> Self {
        let request = fixture::request(
            |value| {
                fixture::targets(value);
                value["sources"] = json!([source()]);
                let secrets = credentials();
                value["secrets"]["whatsapp"] = json!({"app_secret":secrets.app_secret.as_str(),"access_token":secrets.access_token.as_str(),"verify_token":secrets.verify_token.as_str()});
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
        let routed = RoutedIngress::new(
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
            keys.clone(),
            Default::default(),
            Arc::new(UnavailableLauncher),
        )
        .unwrap()
        .with_routed(Some(Arc::new(routed)))
        .with_whatsapp_receipts(Arc::new(Store::new(server.records.clone(), keys)));
        Self {
            server,
            state,
            channel: request.channel,
            _scheduler: scheduler,
        }
    }
    fn verified(&self, message: Value, extras: Vec<Value>) -> VerifiedWhatsappMessage {
        let mut messages = vec![message];
        messages.extend(extras);
        let body = envelope(messages);
        VerifiedWhatsappMessage {
            channel: self.channel.clone(),
            source: source(),
            message: webhook::extract(&source(), &body).unwrap().remove(0),
            body,
            received: Instant::now(),
            received_at: fixture::NOW,
        }
    }
    async fn launch(&self, verified: VerifiedWhatsappMessage) -> LaunchOutcome {
        self.state
            .launch_whatsapp_verified(
                verified,
                self.state.inflight.clone().try_acquire_owned().unwrap(),
            )
            .await
    }
    fn identity(&self, message: &str) -> EventIdentity {
        EventIdentity {
            owner: self.channel.definition.owner.subject.clone(),
            channel_id: self.channel.id.clone(),
            record_id: self.channel.definition.record_id.clone(),
            event_id: webhook::event_id(&source(), message).unwrap(),
        }
    }
}
fn admitted(outcome: LaunchOutcome) {
    assert!(
        matches!(outcome, LaunchOutcome::Admitted { .. }),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn native_whatsapp_rebatched_replays_preserve_first_signed_body_and_conflict_on_changed_message(
) {
    let setup = Setup::new().await;
    let first = setup.verified(text("wamid.original", "hello"), Vec::new());
    let raw = first.body.clone();
    admitted(setup.launch(first).await);
    let before = setup.server.memory.writes.load(Ordering::SeqCst);
    let mut replay = setup.verified(
        text("wamid.original", "hello"),
        vec![text("wamid.unrelated", "elsewhere")],
    );
    Arc::make_mut(&mut replay.channel)
        .definition
        .routing
        .as_mut()
        .unwrap()
        .default_targets = vec![ConsumerId::parse("default").unwrap()];
    admitted(setup.launch(replay).await);
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), before);
    let plan = setup
        .server
        .store()
        .load(&setup.identity("wamid.original"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(plan.source_body, raw);
    assert_eq!(plan.targets.len(), 3);
    assert_eq!(plan.activity.actor.kind, ActorKind::Unknown);
    let changed = setup.verified(text("wamid.original", "different content"), Vec::new());
    assert!(
        matches!(setup.launch(changed).await, LaunchOutcome::Refused(problem) if problem.status == 409)
    );
}

#[tokio::test]
async fn native_whatsapp_reactions_keep_original_routing_and_do_not_invent_prior_state() {
    let setup = Setup::new().await;
    admitted(
        setup
            .launch(setup.verified(text("wamid.original", "hello"), Vec::new()))
            .await,
    );
    let original = setup
        .server
        .store()
        .load(&setup.identity("wamid.original"))
        .await
        .unwrap()
        .unwrap();
    for (id, emoji) in [
        ("wamid.add", "👍"),
        ("wamid.replace", "❤️"),
        ("wamid.remove", ""),
    ] {
        let mut verified = setup.verified(reaction(id, "wamid.original", emoji), Vec::new());
        Arc::make_mut(&mut verified.channel)
            .definition
            .routing
            .as_mut()
            .unwrap()
            .default_targets = vec![ConsumerId::parse("default").unwrap()];
        admitted(setup.launch(verified).await);
        let plan = setup
            .server
            .store()
            .load(&setup.identity(id))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(plan.targets.len(), 3);
        assert_eq!(plan.activity.actor.kind, ActorKind::Unknown);
        for (before, after) in original.targets.iter().zip(&plan.targets) {
            assert_eq!(before.launch.consumer, after.launch.consumer);
            assert_eq!(before.launch.provider, after.launch.provider);
            assert_eq!(
                before.launch.event.partition_key,
                after.launch.event.partition_key
            );
            assert_eq!(
                before.launch.event.conversation_id,
                after.launch.event.conversation_id
            );
        }
        let ActivityKind::Event { ce, .. } = &plan.activity.kind else {
            panic!()
        };
        let data = ce.data.as_ref().unwrap();
        assert!(data.get("old_reactions").is_none());
        assert_eq!(
            data["new_reactions"].as_array().unwrap().len(),
            usize::from(!emoji.is_empty())
        );
        assert_eq!(ce.subject.as_deref(), Some("wamid.original"));
        let writes = setup.server.memory.writes.load(Ordering::SeqCst);
        admitted(
            setup
                .launch(setup.verified(
                    reaction(id, "wamid.original", emoji),
                    vec![text("wamid.other", "batch changed")],
                ))
                .await,
        );
        assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), writes);
    }
}

#[tokio::test]
async fn native_whatsapp_confirmed_outbound_reaction_is_limited_to_receipt_consumer() {
    let setup = Setup::new().await;
    admitted(
        setup
            .launch(setup.verified(text("wamid.original", "hello"), Vec::new()))
            .await,
    );
    let original = setup
        .server
        .store()
        .load(&setup.identity("wamid.original"))
        .await
        .unwrap()
        .unwrap();
    let owner = &original.targets[0].launch;
    let receipt = Receipt {
        kind: "native_whatsapp_receipt_v1".into(),
        original: original.identity.clone(),
        source: original.activity.source.clone(),
        recipient: "447700900123".into(),
        message_id: "wamid.cma-reply".into(),
        consumer: owner.consumer.clone(),
        provider: owner.provider.clone(),
        partition_key: owner.event.partition_key.clone(),
        conversation_id: owner.event.conversation_id.clone(),
        confirmed_at: fixture::NOW,
        retained_until: fixture::NOW + RETENTION,
    };
    let raced = reaction("wamid.outbound-reaction", "wamid.cma-reply", "👍");
    assert!(
        matches!(setup.launch(setup.verified(raced.clone(), Vec::new())).await,
        LaunchOutcome::Refused(problem) if problem.status == 503 && problem.detail.as_deref() == Some("native_whatsapp_target_pending"))
    );
    assert!(setup
        .server
        .store()
        .load(&setup.identity("wamid.outbound-reaction"))
        .await
        .unwrap()
        .is_none());
    setup
        .state
        .whatsapp_receipts
        .as_ref()
        .unwrap()
        .put(&receipt)
        .await
        .unwrap();
    // Receipt evidence exists, but a temporary read failure for its exact
    // originating plan must preserve Meta's retry opportunity.
    *setup.server.memory.unavailable_read_id.lock().unwrap() =
        Some(original.identity.admission_id());
    assert!(matches!(
        setup.launch(setup.verified(raced, Vec::new())).await,
        LaunchOutcome::Refused(problem) if problem.status == 503
            && problem.detail.as_deref() == Some("native_whatsapp_original_unavailable")
    ));
    *setup.server.memory.unavailable_read_id.lock().unwrap() = None;
    admitted(
        setup
            .launch(setup.verified(
                reaction("wamid.outbound-reaction", "wamid.cma-reply", "👍"),
                Vec::new(),
            ))
            .await,
    );
    let event = setup
        .server
        .store()
        .load(&setup.identity("wamid.outbound-reaction"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event.targets.len(), 1);
    assert_eq!(event.targets[0].launch.consumer, owner.consumer);
    assert_eq!(event.activity.routing.as_ref().unwrap().targets.len(), 1);
    let ActivityKind::Event { ce, .. } = &event.activity.kind else {
        panic!()
    };
    assert_eq!(ce.data.as_ref().unwrap()["original_author"]["kind"], "bot");
    let mut other_chat = reaction("wamid.wrong-chat", "wamid.cma-reply", "👍");
    other_chat["from"] = "447700900999".into();
    other_chat["timestamp"] = (fixture::NOW - 121).to_string().into();
    assert!(
        matches!(setup.launch(setup.verified(other_chat, Vec::new())).await, LaunchOutcome::Refused(problem) if problem.status == 422)
    );
}

#[tokio::test]
async fn native_whatsapp_missing_or_revoked_targets_never_start_new_routes() {
    let setup = Setup::new().await;
    let mut unknown = reaction("wamid.unknown", "wamid.absent", "👍");
    unknown["timestamp"] = (fixture::NOW - 121).to_string().into();
    assert!(
        matches!(setup.launch(setup.verified(unknown, Vec::new())).await, LaunchOutcome::Refused(problem) if problem.status == 422)
    );
    admitted(
        setup
            .launch(setup.verified(text("wamid.original", "hello"), Vec::new()))
            .await,
    );
    let mut verified = setup.verified(
        reaction("wamid.revoked", "wamid.original", "👍"),
        Vec::new(),
    );
    Arc::make_mut(&mut verified.channel)
        .definition
        .sources
        .clear();
    assert!(matches!(
        setup.launch(verified).await,
        LaunchOutcome::Refused(_)
    ));
    assert!(setup
        .server
        .store()
        .load(&setup.identity("wamid.unknown"))
        .await
        .unwrap()
        .is_none());
    assert!(setup
        .server
        .store()
        .load(&setup.identity("wamid.revoked"))
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn native_whatsapp_concurrent_rebatched_delivery_keeps_one_admitted_identity() {
    let setup = Setup::new().await;
    let left = setup.verified(text("wamid.racing", "same"), vec![]);
    let right = setup.verified(
        text("wamid.racing", "same"),
        vec![text("wamid.extra", "batch")],
    );
    let (a, b) = tokio::join!(setup.launch(left), setup.launch(right));
    admitted(a);
    admitted(b);
    let plans = setup
        .server
        .store()
        .recover_all_page(None, 32)
        .await
        .unwrap();
    assert_eq!(plans.plans.len(), 1);
    assert_eq!(plans.plans[0].identity, setup.identity("wamid.racing"));
}

#[tokio::test]
async fn native_whatsapp_incoming_target_requires_same_raw_private_sender() {
    let setup = Setup::new().await;
    admitted(
        setup
            .launch(setup.verified(text("wamid.original", "hello"), vec![]))
            .await,
    );
    let mut value = reaction("wamid.wrong-sender", "wamid.original", "👍");
    value["from"] = "447700900999".into();
    assert!(
        matches!(setup.launch(setup.verified(value, vec![])).await, LaunchOutcome::Refused(problem) if problem.status == 403)
    );
    assert!(setup
        .server
        .store()
        .load(&setup.identity("wamid.wrong-sender"))
        .await
        .unwrap()
        .is_none());
}
