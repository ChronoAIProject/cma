use super::{
    inbox_dispatch::{DispatchPage, InboxDispatcher},
    journal_fixture::{self, Fixture},
    normalization_tests::raw,
    Dispatch, DurableJournal,
};
use crate::{
    channels::resolver::ChannelResolver,
    ingress::{
        launch::{LaunchOutcome, LaunchRequest, TurnLauncher},
        routed::RoutedIngress,
        routing_store::PlanStore,
        IngressState,
    },
    turns::routed::scheduler::{Disposition, ScheduleConfig, Scheduler, TargetExecutor, WorkItem},
};
use cmaeg_core::{channel::ChannelCreation, seal::ChannelPointer};
use futures_util::future::BoxFuture;
use std::{
    collections::BTreeSet,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

pub struct Trap(pub AtomicUsize);
impl TurnLauncher for Trap {
    fn launch(&self, _: LaunchRequest) -> BoxFuture<'_, LaunchOutcome> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            LaunchOutcome::Accepted {
                response_id: "must-never-launch".into(),
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
pub struct Setup {
    pub fixture: Fixture,
    pub journal: Arc<DurableJournal>,
    pub state: IngressState,
    pub dispatcher: InboxDispatcher,
    pub plans: PlanStore,
    pub selection: super::selection::Selection,
    pub trap: Arc<Trap>,
    pub channel: cmaeg_core::channel::ChannelDefinition,
    _scheduler: Scheduler,
}
impl Setup {
    pub async fn new(routed: bool) -> Self {
        Self::configured(routed, false).await
    }
    pub async fn discovered() -> Self {
        Self::configured(true, true).await
    }
    async fn configured(routed: bool, discovery: bool) -> Self {
        let fixture = Fixture::new().await;
        let mut channel = journal_fixture::definition();
        if routed {
            channel.routing = Some(serde_json::from_value(serde_json::json!({"version":1,"default_consumer":{"name":"lead"},"consumers":[],"default_targets":["default"]})).unwrap());
        }
        let pointer = ChannelPointer {
            v: 1,
            record_id: channel.record_id.clone(),
            record_url: fixture
                .store
                .channels
                .record_url(&fixture.record)
                .unwrap()
                .to_string(),
        };
        channel.creation = Some(ChannelCreation {
            channel_id: pointer.seal_id(&fixture.store.keys).unwrap(),
            fingerprint: "native-runtime-fixture".into(),
            ready: true,
        });
        fixture.replace(&channel);
        let journal = fixture.acquire().await.unwrap();
        let keys = fixture.store.keys.clone();
        let source_clock = fixture.store.clock.clone();
        let clock: Arc<dyn Fn() -> i64 + Send + Sync> = Arc::new(move || source_clock() / 1000);
        let plans =
            PlanStore::new(fixture.store.channels.clone(), keys.clone()).with_clock(clock.clone());
        let (scheduler, handle) = Scheduler::new(
            Arc::new(plans.clone()),
            Arc::new(Executor),
            ScheduleConfig::default(),
        )
        .unwrap();
        let selected = BTreeSet::from([fixture.record.clone()]);
        let selection = super::selection::Selection::new(&crate::NativeDiscordConfig {
            enabled: true,
            discovery_enabled: discovery,
            rich_content_enabled: false,
            records: selected.clone(),
        });
        let routing = RoutedIngress::new(
            plans.clone(),
            handle,
            keys.clone(),
            if discovery {
                BTreeSet::new()
            } else {
                selected.clone()
            },
            BTreeSet::new(),
            BTreeSet::from([channel.provider.slug.to_string()]),
        )
        .with_native_discovery(selection.clone())
        .with_clock(clock);
        let trap = Arc::new(Trap(AtomicUsize::new(0)));
        let state = IngressState::new(
            ChannelResolver::new(
                Arc::new(fixture.store.channels.clone()),
                keys.clone(),
                Default::default(),
            ),
            keys.clone(),
            Default::default(),
            trap.clone(),
        )
        .unwrap()
        .with_routed(Some(Arc::new(routing)));
        let dispatcher = InboxDispatcher::with_selection(
            fixture.store.channels.clone(),
            keys,
            state.clone(),
            selection.clone(),
        );
        Self {
            fixture,
            journal,
            state,
            dispatcher,
            plans,
            selection,
            trap,
            channel,
            _scheduler: scheduler,
        }
    }
    pub fn now(&self) -> i64 {
        (self.fixture.store.clock)() / 1000
    }
    pub fn observe(&mut self) -> Arc<crate::activity::ActivityStore> {
        let activity = Arc::new(crate::activity::ActivityStore::new(
            self.fixture.store.channels.clone(),
            self.fixture.store.keys.clone(),
        ));
        let observer = Arc::new(crate::activity::ObservedActivity::new(
            activity.clone(),
            Arc::new(crate::turns::observer::NoopObserver),
        ));
        Arc::get_mut(&mut self.journal).unwrap().activity = observer.clone();
        self.state = self.state.clone().with_activity_observer(observer);
        self.dispatcher = InboxDispatcher::with_selection(
            self.fixture.store.channels.clone(),
            self.fixture.store.keys.clone(),
            self.state.clone(),
            self.selection.clone(),
        );
        activity
    }
    pub async fn message(&self, edit: impl FnOnce(&mut serde_json::Value)) -> String {
        let mut data = raw();
        data["id"] = self.fixture.message_id().into();
        edit(&mut data);
        let dispatch = Dispatch {
            name: "MESSAGE_CREATE".into(),
            sequence: 1,
            data,
        };
        self.journal
            .persist_message(&self.journal.authority, &dispatch)
            .await
            .unwrap();
        crate::connections::store::index_id(
            "discord-native-message-v1",
            &[&self.fixture.record, "77", dispatch.message_id().unwrap()],
        )
    }
    pub async fn sweep(&self, now: i64) -> DispatchPage {
        let mut report = DispatchPage {
            next_cursor: None,
            acknowledged: 0,
            pending: 0,
            invalid: 0,
        };
        let mut cursor = None;
        for _ in 0..128 {
            let page = self.dispatcher.page(cursor.as_deref(), now).await.unwrap();
            report.acknowledged += page.acknowledged;
            report.pending += page.pending;
            report.invalid += page.invalid;
            cursor = page.next_cursor;
            if cursor.is_none() {
                return report;
            }
        }
        panic!("unbounded native inbox scan")
    }
    pub fn direct_calls(&self) -> usize {
        self.trap.0.load(Ordering::SeqCst)
    }
}
