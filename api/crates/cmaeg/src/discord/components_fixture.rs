use super::{
    dispatch_fixture::Setup, rest_fixture::Fixture, Checkpoint, Dispatch, InboxLaunch, InboxReader,
    NativeDiscordJournal,
};
use crate::{
    channels::resolver::ResolvedChannel,
    interactions::{Destination, InteractionStore, PreparedCard},
    providers::EventContextSnapshot,
    turns::routed::adapter_test_server::HttpServer,
};
use cmaeg_core::{message::OutboundMessage, seal::EventRefPayload};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

pub(crate) struct FixtureState {
    pub setup: Setup,
    pub rest: Fixture,
    pub store: Arc<InteractionStore>,
    pub channel: Arc<ResolvedChannel>,
    pub original: EventRefPayload,
    pub context: EventContextSnapshot,
    pub provider: HttpServer,
    pub activity: Arc<crate::activity::ActivityStore>,
    original_inbox: String,
}
impl FixtureState {
    pub fn keys(&self) -> Arc<cmaeg_core::seal::MasterKeyRing> {
        self.setup.fixture.store.keys.clone()
    }
    pub async fn named() -> Self {
        let mut f = Self::new().await;
        let mut definition = f.setup.channel.to_blob_json();
        let routed = crate::channels::routing_fixture::routed();
        definition["routing"]["consumers"] = json!([routed["routing"]["consumers"][0]]);
        definition["secrets"]["consumer_keys"] = json!([routed["secrets"]["consumer_keys"][0]]);
        f.setup.channel = serde_json::from_value(definition).unwrap();
        f.setup.fixture.replace(&f.setup.channel);
        f.channel = f
            .setup
            .state
            .resolver
            .resolve_current(f.channel.id.as_str())
            .await
            .unwrap();
        let consumer =
            cmaeg_core::ids::ConsumerId::parse(crate::channels::routing_fixture::REVIEWER).unwrap();
        f.original.conversation_id = cmaeg_core::ids::ConversationId::derive_for_consumer(
            &f.setup.fixture.store.keys,
            &f.channel.id,
            f.setup.channel.partition.version,
            &consumer,
            &f.original.partition_key,
        );
        f.original.consumer = Some(consumer);
        f.context.conversation_id = f.original.conversation_id.clone();
        f.context.event_ref =
            cmaeg_core::seal::EventRef::seal(&f.original, &f.setup.fixture.store.keys).unwrap();
        f.provider.memory.lock().unwrap().config.event_context =
            Some(serde_json::to_value(&f.context).unwrap());
        f
    }
    pub async fn new() -> Self {
        Self::configured(false).await
    }
    pub async fn group() -> Self {
        Self::configured(true).await
    }
    async fn configured(group: bool) -> Self {
        let mut setup = Setup::discovered().await;
        let activity = setup.observe();
        let provider = HttpServer::start().await;
        let rest = Fixture::new().await;
        rest.respond(axum::http::StatusCode::NO_CONTENT, Value::Null);
        let store = Arc::new(InteractionStore::new(
            setup.fixture.store.channels.clone(),
            setup.fixture.store.keys.clone(),
            provider.client(true, Duration::from_secs(5)),
        ));
        let id = setup
            .message(|raw| {
                if group {
                    raw["guild_id"] = "88".into();
                }
            })
            .await;
        let channel = setup
            .state
            .resolver
            .resolve_current(setup.channel.creation.as_ref().unwrap().channel_id.as_str())
            .await
            .unwrap();
        let InboxLaunch::Launch(verified) = InboxReader::new(
            setup.fixture.store.channels.clone(),
            setup.fixture.store.keys.clone(),
        )
        .load(&id, channel.clone(), setup.now())
        .await
        .unwrap() else {
            panic!()
        };
        let original = verified
            .request
            .event_ref
            .open(&setup.fixture.store.keys, setup.now())
            .unwrap()
            .payload;
        let context = EventContextSnapshot {
            schema_version: 1,
            channel_id: channel.id.clone(),
            conversation_id: original.conversation_id.clone(),
            activity: verified.request.activity,
            event_ref: verified.request.event_ref,
        };
        provider.memory.lock().unwrap().config.event_context =
            Some(serde_json::to_value(&context).unwrap());
        setup.state.interactions = Some(store.clone());
        setup.dispatcher = super::inbox_dispatch::InboxDispatcher::with_selection(
            setup.fixture.store.channels.clone(),
            setup.fixture.store.keys.clone(),
            setup.state.clone(),
            setup.selection.clone(),
        );
        setup.journal.authorize_connection(None).await.unwrap();
        let ready = Dispatch {
            name: "READY".into(),
            sequence: 1,
            data: json!({"session_id":"component-fixture","resume_gateway_url":"wss://gateway.discord.gg/","user":{"id":"77","bot":true}}),
        };
        let checkpoint = Checkpoint {
            session_id: "component-fixture".into(),
            resume_url: "wss://gateway.discord.gg/?v=10&encoding=json".into(),
            sequence: 1,
        };
        setup.journal.admit(&checkpoint, &ready).await.unwrap();
        let api = super::DiscordApi::fixture(&format!("{}/api/v10/", rest.base))
            .with_components(setup.state.clone());
        Arc::get_mut(&mut setup.journal).unwrap().api = api;
        setup.fixture.advance(1);
        Self {
            setup,
            rest,
            store,
            channel,
            original,
            context,
            provider,
            activity,
            original_inbox: id,
        }
    }
    /// The synthetic message establishes sealed event context only. Settle it
    /// before installing a real executor so this fixture launches just the click.
    pub async fn settle_context_message(&self) {
        let inbox = super::inbox_scan::InboxStore::new(
            self.setup.fixture.store.channels.clone(),
            self.setup.fixture.store.keys.clone(),
        );
        let mut cursor = None;
        loop {
            let page = inbox
                .page(cursor.as_deref(), self.setup.now())
                .await
                .unwrap();
            if let Some(entry) = page
                .entries
                .iter()
                .find(|entry| entry.id == self.original_inbox)
            {
                inbox
                    .acknowledge(
                        entry,
                        super::inbox_scan::Disposition::Refused {
                            reason: "fixture context only".into(),
                        },
                    )
                    .await
                    .unwrap();
                return;
            }
            cursor = page.next_cursor;
            assert!(cursor.is_some(), "original fixture context must exist");
        }
    }
    pub async fn card(&self, message: &OutboundMessage) -> PreparedCard {
        let mut card = self
            .store
            .prepare_with_context(&self.channel, &self.original, message, self.context.clone())
            .await
            .unwrap();
        self.store
            .bind(
                &mut card,
                Destination {
                    outbound_id: "800".into(),
                    platform_id: "800".into(),
                },
            )
            .await
            .unwrap();
        card
    }
    pub fn callback(&self, custom: &str) -> Value {
        json!({"id":self.setup.fixture.message_id(),"application_id":"77","type":3,"token":"private-interaction-token","channel_id":"99","channel":{"id":"99","type":1},"user":{"id":"42","bot":false},"message":{"id":"800","channel_id":"99","author":{"id":"77","bot":true}},"data":{"component_type":2,"custom_id":custom}})
    }
    pub async fn receive(&self, data: Value, sequence: u64) -> Result<(), super::GatewayError> {
        self.setup
            .journal
            .admit(
                &Checkpoint {
                    session_id: "component-fixture".into(),
                    resume_url: "wss://gateway.discord.gg/?v=10&encoding=json".into(),
                    sequence,
                },
                &Dispatch {
                    name: "INTERACTION_CREATE".into(),
                    sequence,
                    data,
                },
            )
            .await
    }
    pub fn id(&self, data: &Value) -> String {
        crate::connections::store::index_id(
            "discord-native-message-v1",
            &[
                &self.setup.fixture.record,
                "77",
                data["id"].as_str().unwrap(),
            ],
        )
    }
}
pub(crate) fn message(select: bool) -> OutboundMessage {
    serde_json::from_value(json!({"text":"Choose","blocks": if select {
        json!([{"type":"choice","id":"scope","prompt":"Choose a scope","multi":false,"options":[{"id":"private-read","label":"Read"},{"id":"private-write","label":"Write"}]}])
    } else { json!([{"type":"buttons","id":"approval","options":[{"id":"private-yes","label":"Yes","style":"primary"},{"id":"private-no","label":"No","style":"danger"}]}]) }})).unwrap()
}
