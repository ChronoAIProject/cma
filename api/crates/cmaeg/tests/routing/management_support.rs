//! Local owner walkthrough: real management, auth and ingress with fake backends.
use super::{
    fake_nyxid::FakeNyxid, fake_provider::FakeProvider, routing_fixture as fixture, support::App,
};
use cmaeg::{
    auth::CreatorAuthenticator,
    channels::management::ManagementState,
    providers::{ProviderClient, ProviderRegistry},
    records::RecordClient,
};
use cmaeg_core::{
    channel::{ChannelDefinition, Source},
    seal::{open_blob, seal_blob},
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

pub struct OwnerApp {
    pub ingress: App,
    pub management: ManagementState,
    pub base: String,
    pub providers: [FakeProvider; 2],
    nyxid: FakeNyxid,
    task: tokio::task::JoinHandle<()>,
}
impl OwnerApp {
    pub async fn new(enabled: bool) -> Self {
        Self::with_sources(enabled, false).await
    }
    pub async fn with_sources(enabled: bool, dynamic_sources: bool) -> Self {
        let mut cache = cmaeg::ChannelCacheConfig::default();
        if dynamic_sources {
            // This walkthrough deliberately makes several invalidating writes in a burst.
            cache.restores_per_minute = std::num::NonZeroU32::new(60_000).unwrap();
        }
        let ingress = App::with_cache(enabled, cache, |raw| {
            raw["owner"]["subject"] = "alice".into();
            raw["profile"]["metadata"]["owner"]["subject"] = "alice".into();
            raw["profile_digest"] = fixture::digest_of(&raw["profile"]).into();
            for consumer in raw["routing"]["consumers"].as_array_mut().unwrap() {
                consumer["profile"]["metadata"]["owner"]["subject"] = "alice".into();
                consumer["profile_digest"] = fixture::digest_of(&consumer["profile"]).into();
            }
        })
        .await;
        let providers = [
            FakeProvider::start(Default::default()).await,
            FakeProvider::start(Default::default()).await,
        ];
        let nyxid = FakeNyxid::start().await;
        let cma_base = reqwest::Url::parse(&ingress.channel.pointer.record_url)
            .unwrap()
            .origin()
            .ascii_serialization();
        let registry: Vec<Value> = ["cma_codex", "nyxbot"].iter().zip(&providers).map(|(slug, provider)| json!({
            "slug":slug,"base_url":provider.base_url(),"kind":"responses_http","max_inflight":8,
            "paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}",
                "conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"}
        })).collect();
        let mut management = ManagementState::new(
            Arc::new(
                ProviderClient::new(
                    ProviderRegistry::parse(&serde_json::to_string(&registry).unwrap()).unwrap(),
                    Duration::from_secs(1),
                    Duration::from_secs(5),
                    8,
                )
                .unwrap(),
            ),
            Arc::new(RecordClient::new(&cma_base, "cma-service").unwrap()),
            Arc::new(CreatorAuthenticator::new(&nyxid.base_url).unwrap()),
            cmaeg::ManagementConfig {
                master_keys: ingress.state.master_keys.clone(),
                public_url: ingress.base.clone(),
                cma_base_url: cma_base,
                tools_nyxid_slug: "cmaeg".into(),
                cma_service_token: cmaeg_core::channel::Secret::new("cma-service"),
                nyxid_base_url: nyxid.base_url.clone(),
            },
            Default::default(),
        );
        management.resolver = ingress.state.resolver.clone();
        management.routing = ingress.state.routing.clone();
        management.routing.dynamic_sources_enabled = dynamic_sources;
        management.media_tool_providers = ["cma_codex".parse().unwrap(), "nyxbot".parse().unwrap()]
            .into_iter()
            .collect();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let router = cmaeg::router_with_state(management.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            ingress,
            management,
            base,
            providers,
            nyxid,
            task,
        }
    }
    pub async fn call(
        &self,
        method: reqwest::Method,
        suffix: &str,
        body: Option<&Value>,
        status: u16,
    ) -> Value {
        let mut request = self
            .ingress
            .client
            .request(
                method,
                format!("{}/v1/channels/{}{}", self.base, self.ingress.id, suffix),
            )
            .bearer_auth("token-alice");
        if let Some(body) = body {
            request = request.json(body);
        }
        checked(request.send().await.unwrap(), status).await
    }
    /// Point only the fixture's JWKS reader at the local signing key server.
    pub async fn local_jwks(&self) -> u64 {
        let channel = self
            .management
            .resolver
            .resolve(&self.ingress.id)
            .await
            .unwrap();
        let pointer = &channel.pointer;
        let record = self
            .management
            .records
            .get(&pointer.record_url)
            .await
            .unwrap();
        let mut definition: ChannelDefinition = serde_json::from_slice(
            &open_blob(&self.management.master_keys, &record.blob)
                .unwrap()
                .payload,
        )
        .unwrap();
        for source in &mut definition.sources {
            if let Source::NyxidRelay { jwks_url, .. } = source {
                *jwks_url = format!("{}/jwks", self.ingress.platform_base);
            }
        }
        definition.validate().unwrap();
        let version = self
            .management
            .records
            .put(
                &definition.record_id,
                &definition.owner.subject,
                &seal_blob(
                    &self.management.master_keys,
                    definition.to_blob_json().to_string().as_bytes(),
                )
                .unwrap(),
                Some(record.version),
            )
            .await
            .unwrap();
        self.management
            .resolver
            .invalidate_record(&definition.record_id, Some(&self.ingress.id));
        version
    }
    pub async fn close(self) {
        self.task.abort();
        for provider in self.providers {
            provider.close().await;
        }
        self.nyxid.close().await;
    }
}
pub async fn checked(response: reqwest::Response, status: u16) -> Value {
    let actual = response.status();
    let value: Value = response.json().await.unwrap();
    assert_eq!(actual, status, "{value}");
    value
}
pub fn profile(slug: &str) -> Value {
    let mut profile = fixture::profile(slug);
    profile["metadata"]["owner"]["subject"] = "alice".into();
    profile
}
pub fn consumer(version: &Value, name: &str, slug: &str) -> Value {
    json!({"expected_version":version,"name":name,"label":name,"provider":{"slug":slug,"target":{"type":"profile"}},
        "profile":profile(slug),"credentials":{"agent_key":format!("fixture-{name}-key")},"mentionable":true,
        "reply":{"auto_final":true,"destination":"origin"}})
}
