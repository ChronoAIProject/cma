use super::*;
use serde_json::{json, Value};
mod connection_channels;
mod consumer_fixture;
mod effect_deadline;
mod effect_deadline_http;
mod fixture;
mod key_expiry;
mod lifecycle;
mod nyxid_fixture;
mod provider_fixture;
mod rest;
mod route_inventory;
mod web;
use fixture::*;

fn secrets() -> BotSecrets {
    BotSecrets {
        agent: ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap(),
        route: Some(
            ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap(),
        ),
    }
}
fn keys(secrets: &BotSecrets, suffix: &str) -> BotCredentialSet {
    BotCredentialSet {
        agent_key_id: Some(format!("agent-{suffix}")),
        agent_key_hash: Some(secrets.agent.sha256()),
        route_key_id: Some(format!("route-{suffix}")),
        route_key_hash: Some(secrets.route.as_ref().unwrap().sha256()),
        expires_at: Some(layer2_core::rfc3339_at(
            std::time::SystemTime::now() + Duration::from_secs(30 * 86400),
        )),
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn compiled_gateway_accepts_cma_profiles_and_full_lifecycle_after_restart() {
    let server = Server::start().await;
    let mut gateway = Gateway::start(&server).await;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .unwrap();
    let client = client::GatewayClient::new(
        http,
        &GatewayConfig {
            binding_preparation_enabled: false,
            conversation_reset_enabled: false,
            base_url: gateway.base.clone(),
            public_url: gateway.base.clone(),
            public_api_url: gateway.base.clone(),
            public_api_aliases: vec![],
        },
        "https://nyx.example".into(),
    )
    .unwrap();
    let mut bot = bot().await;
    let original = secrets();
    bot.resources.credentials = keys(&original, "original");
    let created = client
        .create("owner-token", &bot, &original, Some("fixture_bot".into()))
        .await
        .unwrap();
    assert!(created.uses(&bot.resources.credentials));
    assert!(created.route_ids.is_empty());
    assert_eq!(created.channel.record_id, bot.record_id);
    assert_eq!(created.channel.level, "l2");
    let profile = server.remote.profiles.lock().unwrap()[0].clone();
    assert_eq!(profile["model"]["thinking_effort"], "very_high");
    assert_eq!(profile["environment"]["size"]["cpu_cores"], 1.5);
    assert_eq!(
        profile["permission"]["service_scope"],
        json!(["chrono-llm-public", "ornn-api"])
    );
    bot.resources.channel = Some(created.channel.clone());
    bot.resources.route_id = Some("exact-route".into());
    let attached = client
        .configure(
            "owner-token",
            &bot,
            &created,
            &bot.resources.credentials,
            None,
            true,
        )
        .await
        .unwrap();
    assert_eq!(attached.route_ids, ["exact-route"]);
    gateway.restart(&server).await;
    let recovered = client.channel("owner-token", &bot).await.unwrap().unwrap();
    assert_eq!(recovered.channel, attached.channel);
    let disabled = client
        .configure(
            "owner-token",
            &bot,
            &recovered,
            &bot.resources.credentials,
            None,
            false,
        )
        .await
        .unwrap();
    let body: Value = reqwest::Client::new()
        .get(format!(
            "{}/v1/channel-records/{}",
            gateway.base, bot.record_id
        ))
        .bearer_auth("owner-token")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        body["definition"]["sources"][0]["admission"],
        json!({"type":"scoped","senders":{"type":"deny"},"chats":{"type":"deny"},"groups":"deny"})
    );
    assert_eq!(body["definition"]["reply"]["ack_after_ms"], 0);
    let replacement = secrets();
    let replacement_keys = keys(&replacement, "replacement");
    let rotated = client
        .configure(
            "owner-token",
            &bot,
            &disabled,
            &replacement_keys,
            Some(&replacement),
            false,
        )
        .await
        .unwrap();
    assert!(rotated.uses(&replacement_keys));
    assert_eq!(
        server.remote.provider_keys.lock().unwrap()[&rotated.channel.binding_id],
        replacement.agent.expose()
    );
    assert!(
        client.delete("owner-token", &attached).await.is_err(),
        "stale delete must fail"
    );
    client.delete("owner-token", &rotated).await.unwrap();
    assert!(client.channel("owner-token", &bot).await.unwrap().is_none());
    assert!(server.remote.provider_keys.lock().unwrap().is_empty());
    assert!(server.remote.records.lock().unwrap().is_empty());
}
