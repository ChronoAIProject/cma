use super::{
    compile,
    management::{Manager, Proof},
    service::Forms,
};
use crate::{
    interactions::whatsapp_native_fixture,
    whatsapp::{Client, Error, Transport},
};
use cmaeg_core::{
    channel::{Source, WhatsappCredentials},
    message::OutboundMessage,
};
use futures_util::future::BoxFuture;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

pub(super) fn message() -> OutboundMessage {
    serde_json::from_value(json!({"text":"Please answer", "blocks":[{"type":"form","id":"survey","fields":[
        {"id":"business/name","label":"Name","required":true,"kind":{"type":"text"}},
        {"id":"amount","label":"Amount","required":true,"kind":{"type":"number"}},
        {"id":"when","label":"Date","required":true,"kind":{"type":"date"}},
        {"id":"choice","label":"Choose","required":false,"kind":{"type":"choice","options":[{"id":"yes","label":"Yes","selected":true},{"id":"no","label":"No"}]}}
    ]}]})).unwrap()
}
pub(super) fn values() -> super::values::Submitted {
    serde_json::from_value(json!({"f0":"Ada","f1":"-2.5e2","f2":"2028-02-29","f3":"0"})).unwrap()
}
#[derive(Default)]
pub(super) struct Graph {
    pub calls: AtomicUsize,
    pub unknown: AtomicBool,
}
impl Transport for Graph {
    fn exchange(
        &self,
        method: reqwest::Method,
        url: reqwest::Url,
        _: reqwest::header::HeaderMap,
        body: Vec<u8>,
        _: std::time::Duration,
    ) -> BoxFuture<'_, Result<crate::net::exchange::HttpReply, crate::net::exchange::TransportError>>
    {
        assert_eq!(method, reqwest::Method::POST);
        assert!(url.path().ends_with("/messages"));
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["interactive"]["type"], "flow");
        assert_eq!(
            body["interactive"]["action"]["parameters"]["mode"],
            "published"
        );
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if self.unknown.load(Ordering::SeqCst) {
                return Err(crate::net::exchange::TransportError::Unavailable);
            }
            Ok(crate::net::exchange::HttpReply { status:reqwest::StatusCode::OK,headers:Default::default(),body:Ok(serde_json::to_vec(&json!({"messaging_product":"whatsapp","contacts":[{"input":"447700900123","wa_id":"447700900123"}],"messages":[{"id":"wamid.flow"}]})).unwrap()) })
        })
    }
}
pub(super) struct Management {
    pub creates: AtomicUsize,
    pub publishes: AtomicUsize,
    pub published: AtomicBool,
    pub unknown_create: AtomicBool,
    pub unknown_publish: AtomicBool,
    pub ambiguous: AtomicBool,
    pub verify_delay: AtomicBool,
    pub digest: String,
}
impl Manager for Management {
    fn verify<'a>(
        &'a self,
        _: &'a Source,
        _: &'a WhatsappCredentials,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async {
            if self.verify_delay.load(Ordering::SeqCst) {
                tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
            }
            Ok(())
        })
    }
    fn create<'a>(
        &'a self,
        _: &'a Source,
        _: &'a WhatsappCredentials,
        _: &'a str,
        _: &'a Value,
    ) -> BoxFuture<'a, Result<String, Error>> {
        Box::pin(async move {
            self.creates.fetch_add(1, Ordering::SeqCst);
            if self.unknown_create.load(Ordering::SeqCst) {
                Err(Error::Unconfirmed)
            } else {
                Ok("99999".into())
            }
        })
    }
    fn inspect<'a>(
        &'a self,
        _: &'a Source,
        _: &'a WhatsappCredentials,
        id: &'a str,
        _: &'a str,
    ) -> BoxFuture<'a, Result<Proof, Error>> {
        Box::pin(async move {
            Ok(Proof {
                id: id.into(),
                published: self.published.load(Ordering::SeqCst),
                digest: self.digest.clone(),
            })
        })
    }
    fn publish<'a>(
        &'a self,
        _: &'a WhatsappCredentials,
        _: &'a str,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            self.publishes.fetch_add(1, Ordering::SeqCst);
            self.published.store(true, Ordering::SeqCst);
            if self.unknown_publish.load(Ordering::SeqCst) {
                Err(Error::Unconfirmed)
            } else {
                Ok(())
            }
        })
    }
    fn candidates<'a>(
        &'a self,
        _: &'a Source,
        _: &'a WhatsappCredentials,
        _: &'a str,
    ) -> BoxFuture<'a, Result<Vec<String>, Error>> {
        Box::pin(async move {
            Ok(if self.ambiguous.load(Ordering::SeqCst) {
                vec!["99999".into(), "88888".into()]
            } else {
                vec!["99999".into()]
            })
        })
    }
}
pub(super) struct Fixture {
    pub base: whatsapp_native_fixture::Fixture,
    pub forms: Forms,
    pub management: Arc<Management>,
    pub graph: Arc<Graph>,
}
impl Fixture {
    pub async fn new(named: bool) -> Self {
        Self::with_message(named, &message()).await
    }
    pub async fn with_message(named: bool, form: &OutboundMessage) -> Self {
        let mut base = whatsapp_native_fixture::Fixture::new(named).await;
        if let Source::Whatsapp { forms_enabled, .. } =
            &mut Arc::make_mut(&mut base.channel).definition.sources[0]
        {
            *forms_enabled = true;
        } else {
            panic!("whatsapp source");
        }
        base.context.activity.occurred_at = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        base.event.activity_sha256 = Some(cmaeg_core::seal::event_ref::activity_digest(
            &base.context.activity,
        ));
        base.context.event_ref = cmaeg_core::seal::EventRef::seal(
            &base.event,
            &crate::ingress::routing_plan::fixture::keys(),
        )
        .unwrap();
        let definition = Arc::make_mut(&mut base.channel)
            .definition
            .routing
            .as_mut()
            .unwrap();
        definition.default_consumer.non_human = Some(true);
        for consumer in &mut definition.consumers {
            consumer.non_human = Some(true);
        }
        base.save_channel();
        let graph = Arc::new(Graph::default());
        let client = Client::production().unwrap().with_transport(graph.clone());
        let resolver = crate::channels::resolver::ChannelResolver::new(
            Arc::new(base.server.records.clone()),
            Arc::new(crate::ingress::routing_plan::fixture::keys()),
            Default::default(),
        );
        base.channel = resolver
            .resolve_current(base.channel.id.as_str())
            .await
            .unwrap();
        let mut forms = Forms::new(
            base.server.records.clone(),
            Arc::new(crate::ingress::routing_plan::fixture::keys()),
            resolver,
            base.store.as_ref().clone(),
            client,
            true,
            true,
        );
        let management = Arc::new(Management {
            creates: 0.into(),
            publishes: 0.into(),
            published: false.into(),
            unknown_create: false.into(),
            unknown_publish: false.into(),
            ambiguous: false.into(),
            verify_delay: false.into(),
            digest: compile::compile(form).unwrap().digest,
        });
        forms.provisioner.manager = management.clone();
        Self {
            base,
            forms,
            management,
            graph,
        }
    }
    pub async fn prepare(&self) -> crate::interactions::Card {
        self.base
            .store
            .prepare_whatsapp_flow(
                &self.base.channel,
                &self.base.event,
                &message(),
                self.base.context.activity.clone(),
                "prepare-key",
                &self.forms.provisioner.store,
            )
            .await
            .unwrap()
    }
    pub async fn ready(&self) -> crate::interactions::Card {
        let card = self.prepare().await;
        let id = &card.native_whatsapp_flow.as_ref().unwrap().asset_registry;
        for _ in 0..5 {
            self.forms.provisioner.advance(id).await.unwrap();
        }
        assert!(matches!(
            self.forms
                .status(&self.base.channel, &self.base.event)
                .await
                .unwrap()
                .state,
            super::service::FormPreparationState::Ready { .. }
        ));
        card
    }
    pub async fn commit(
        &self,
        card: &crate::interactions::Card,
        key: &str,
    ) -> Result<crate::delivery::intents::IntentReceipt, super::Error> {
        self.forms
            .commit(
                &self.base.channel,
                &self.base.event,
                &message(),
                &card.native_whatsapp_flow.as_ref().unwrap().preparation_ref,
                key,
                tokio::time::Instant::now() + std::time::Duration::from_secs(8),
            )
            .await
    }
    pub fn callback(&self, card: &crate::interactions::Card) -> super::callback::Callback {
        super::callback::Callback {
            kind: "native_whatsapp_flow_completion_v1".into(),
            card: card.id.clone(),
            revision: 1,
            source: self.base.context.activity.source.clone(),
            id: "wamid.completion".into(),
            sender: self.base.context.activity.actor.id.clone(),
            context: "wamid.flow".into(),
            timestamp: whatsapp_native_fixture::now(),
            fields: values(),
        }
    }
}
