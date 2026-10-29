//! Real HMAC Custom HTTP admission and encrypted records; dispatch is controlled by each test.
use super::{preparation_dispatch_tests::PreparingPort, scheduler::*, *};
use crate::{
    auth::webhook,
    channels::{resolver::ChannelResolver, routing_fixture},
    delivery::sync::SyncRegistry,
    http::events::CustomIngressState,
    ingress::{
        launch::{LaunchOutcome, LaunchRequest, TurnLauncher},
        routed::RoutedIngress,
        routing_plan::fixture,
        routing_store::{
            test_server::{Row, Server},
            AdmittedPlan, PlanStore,
        },
        IngressState,
    },
};
use cmaeg_core::{
    channel::Secret,
    seal::{seal_blob, ChannelPointer},
};
use futures_util::future::BoxFuture;
use serde_json::{json, Value};
use std::{collections::BTreeSet, sync::Arc, time::Duration};

struct NoLegacy;
impl TurnLauncher for NoLegacy {
    fn launch(&self, _: LaunchRequest) -> BoxFuture<'_, LaunchOutcome> {
        panic!("selected Custom ingress must remain durable")
    }
}
struct ControlledExecution;
impl TargetExecutor for ControlledExecution {
    fn execute<'a>(&'a self, _: &'a WorkItem) -> BoxFuture<'a, Disposition> {
        panic!("test dispatches only after inspecting durable HTTP admission")
    }
}

pub(super) struct App {
    pub records: Server,
    pub port: Arc<PreparingPort>,
    pub dispatcher: Dispatcher,
    definition: Value,
    record: String,
    resolver: ChannelResolver,
    channel: String,
    servers: Vec<tokio::task::JoinHandle<()>>,
    schedulers: Vec<Scheduler>,
}
impl Drop for App {
    fn drop(&mut self) {
        for server in &self.servers {
            server.abort();
        }
    }
}
impl App {
    pub async fn new(lose_reply: bool) -> Self {
        let records = Server::start_realtime().await;
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let record = uuid::Uuid::new_v4().to_string();
        let mut value = routing_fixture::routed();
        value["record_id"] = record.clone().into();
        value["sources"] = json!([routing_fixture::custom()]);
        value["partition"]["key"] = "per_event".into();
        value["reply"]["sinks"] = json!([{"type":"sync"}]);
        value["routing"]["consumers"] = json!([]);
        value["secrets"]["consumer_keys"] = json!([]);
        let definition = routing_fixture::definition(value.clone());
        definition.validate().unwrap();
        let keys = Arc::new(fixture::keys());
        let pointer = ChannelPointer {
            v: 1,
            record_id: definition.record_id.clone(),
            record_url: records.records.record_url(&record).unwrap().to_string(),
        };
        let channel = pointer.seal_id(&keys).unwrap().to_string();
        records.memory.channels.lock().unwrap().insert(
            record.clone(),
            Row {
                owner: definition.owner.subject.clone(),
                blob: seal_blob(
                    &keys,
                    &serde_json::to_vec(&definition.to_blob_json()).unwrap(),
                )
                .unwrap(),
                expiry: now + 3600,
                version: 1,
            },
        );
        let resolver = ChannelResolver::new(
            Arc::new(records.records.clone()),
            keys.clone(),
            Default::default(),
        );
        let port = PreparingPort::new(false, lose_reply, false);
        let dispatcher = Dispatcher::new(
            PlanStore::new(records.records.clone(), keys.clone()),
            Journal::new(records.records.clone(), keys),
            Arc::new(resolver.clone()),
            port.clone(),
        );
        Self {
            records,
            port,
            dispatcher,
            definition: value,
            record,
            resolver,
            channel,
            servers: vec![],
            schedulers: vec![],
        }
    }
    pub async fn listen(&mut self, enabled: bool, enrolled: bool) -> String {
        let keys = Arc::new(fixture::keys());
        let store = PlanStore::new(self.records.records.clone(), keys.clone());
        let (scheduler, schedule) = Scheduler::new(
            Arc::new(store.clone()),
            Arc::new(ControlledExecution),
            Default::default(),
        )
        .unwrap();
        self.schedulers.push(scheduler);
        let routing = RoutedIngress::new(
            store,
            schedule,
            keys.clone(),
            BTreeSet::from([self.record.clone()]),
            BTreeSet::from(["cma_codex".into()]),
            BTreeSet::from(["cma_codex".into()]),
        )
        .with_binding_preparation(
            enabled,
            if enrolled {
                BTreeSet::from([self.definition["provider"]["binding_id"]
                    .as_str()
                    .unwrap()
                    .to_string()])
            } else {
                BTreeSet::new()
            },
        )
        .with_sync_delivery(true);
        let ingress = IngressState::new(
            self.resolver.clone(),
            keys,
            Default::default(),
            Arc::new(NoLegacy),
        )
        .unwrap()
        .with_routed(Some(Arc::new(routing)));
        let router = crate::http::events::router(CustomIngressState {
            ingress,
            waiters: Arc::new(SyncRegistry::default()),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/v1/channels/{}/events",
            listener.local_addr().unwrap(),
            self.channel
        );
        self.servers.push(tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        }));
        url
    }
    pub fn event(id: &str) -> Value {
        json!({"specversion":"1.0","id":id,"source":"https://producer.example/build","type":"build.completed","subject":"fresh-event-test","data":{"request":"reply verification"}})
    }
    pub fn request(url: &str, body: String) -> reqwest::RequestBuilder {
        let timestamp = time::OffsetDateTime::now_utc().unix_timestamp().to_string();
        let signature = webhook::signature(
            &Secret::new("Zml4dHVyZQ=="),
            "custom-preparation",
            &timestamp,
            body.as_bytes(),
        );
        reqwest::Client::new()
            .post(url)
            .header("content-type", "application/cloudevents+json")
            .header("webhook-id", "custom-preparation")
            .header("webhook-timestamp", timestamp)
            .header("webhook-signature", signature)
            .body(body)
            .timeout(Duration::from_secs(5))
    }
    pub async fn plan(&self, id: &str) -> AdmittedPlan {
        PlanStore::new(self.records.records.clone(), Arc::new(fixture::keys()))
            .recover_all_page(None, 32)
            .await
            .unwrap()
            .plans
            .into_iter()
            .find(|plan| plan.identity.event_id.as_str() == id)
            .unwrap()
    }
    pub async fn dispatch(&self, plan: &AdmittedPlan) -> Result<DispatchOutcome, ExecutionError> {
        self.dispatcher
            .dispatch(&plan.identity, &plan.targets[0].id, "custom-http-worker")
            .await
    }
    pub fn revoke_on_inspection(&self, producer: bool) {
        let memory = self.records.memory.clone();
        let record = self.record.clone();
        let mut value = self.definition.clone();
        if producer {
            value["secrets"]["hmac_secrets"][0][1] = "rotated-under-same-key-id".into();
        } else {
            value["provider"]["binding_id"] = "rebound-consumer".into();
        }
        let definition = routing_fixture::definition(value);
        definition.validate().unwrap();
        let blob = seal_blob(
            &fixture::keys(),
            &serde_json::to_vec(&definition.to_blob_json()).unwrap(),
        )
        .unwrap();
        *self.port.on_inspect.lock().unwrap() = Some(Arc::new(move || {
            let mut channels = memory.channels.lock().unwrap();
            let row = channels.get_mut(&record).unwrap();
            row.blob = blob.clone();
            row.version += 1;
        }));
    }
}
