use super::{
    config::{private_json, Config},
    ledger::hash,
    protocol::Event,
    Shared,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use cmaeg::ingress::routing_store::EventIdentity;
use cmaeg_core::{
    channel::ChannelDefinition,
    ids::EventId,
    seal::{seal_blob, ChannelPointer, MasterKeyRing},
};
use futures_util::TryStreamExt;
use mongodb::bson::{doc, spec::BinarySubtype, Binary, Document};
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

pub fn random_secret() -> Result<String, &'static str> {
    let mut value = [0; 32];
    SystemRandom::new()
        .fill(&mut value)
        .map_err(|_| "fixture randomness unavailable")?;
    Ok(STANDARD.encode(value))
}

pub async fn prepare(config: Config) -> Result<Arc<Shared>, &'static str> {
    super::config::private_dir(&config.private_output)?;
    private_json(
        &config.private_output.join("custody.json"),
        &json!({
            "database":config.database,"database_sha256":hash(config.database.as_bytes()),
            "state":"preparation_started","automatic_database_cleanup":false,
            "replicas_enabled":config.replicas_enabled
        }),
    )?;
    let admin = super::config::local_mongo(&config.admin_mongodb_uri).await?;
    // Validate all client destinations before the first database mutation.
    super::config::local_mongo(&config.gateway_mongodb_uri)
        .await?
        .shutdown()
        .await;
    super::config::local_mongo(&config.oracle_mongodb_uri)
        .await?
        .shutdown()
        .await;
    let database = admin.database(&config.database);
    if !database
        .list_collection_names()
        .await
        .map_err(|_| "fixture collection inventory")?
        .is_empty()
    {
        return Err("fixture database must be fresh and empty");
    }
    if config.replicas_enabled {
        cmaeg::records::mongo::migrate_replicas(&database)
            .await
            .map_err(|_| "fixture replica migration")?;
    } else {
        cmaeg::records::mongo::migrate(&database)
            .await
            .map_err(|_| "fixture migration")?;
    }
    let master = format!("1:{}", random_secret()?);
    let keys = Arc::new(MasterKeyRing::parse(&master).map_err(|_| "fixture master key")?);
    let provider_bearer = random_secret()?;
    let service_bearer = random_secret()?;
    let definition = channel(&config.advertised_origin, &provider_bearer)?;
    let channel_key = definition
        .secrets
        .channel_key
        .clone()
        .ok_or("fixture channel key")?;
    let record_id = definition.record_id.to_string();
    let owner = definition.owner.subject.clone();
    let pointer = ChannelPointer {
        v: 1,
        record_id: definition.record_id.clone(),
        record_url: format!(
            "{}/api/v1/gateway/channel-records/{record_id}",
            config.advertised_origin
        ),
    };
    let channel_id = pointer.seal_id(&keys).map_err(|_| "fixture channel seal")?;
    let blob = seal_blob(&keys, definition.to_blob_json().to_string().as_bytes())
        .map_err(|_| "fixture channel encryption")?;
    database.collection::<Document>("eg_channel_records").insert_one(doc!{"_id":&record_id,"owner":&owner,"blob":Binary{subtype:BinarySubtype::Generic,bytes:blob},"version":1_i64,"schema_version":1_i32}).await.map_err(|_| "fixture channel insert")?;
    let mut events = BTreeMap::new();
    let mut roots = BTreeMap::new();
    for label in [
        "healthy_a",
        "healthy_b",
        "healthy_c",
        "precommit",
        "known_abort",
        "stalled",
        "postcommit",
        "lifecycle",
    ] {
        let event_id = uuid::Uuid::new_v4().to_string();
        let identity = EventIdentity {
            owner: owner.clone(),
            channel_id: channel_id.clone(),
            record_id: definition.record_id.clone(),
            event_id: EventId::new(&event_id).map_err(|_| "fixture event ID")?,
        };
        roots.insert(label,json!({"event_id":event_id,"admission_root":identity.admission_id(),"admission_sha256":hash(identity.admission_id().as_bytes())}));
        events.insert(label.to_owned(), Event::new(identity));
    }
    let receipts = config.private_output.join("durable-receipts");
    super::config::private_dir(&receipts)?;
    let oracle = super::oracle::Oracle::open(
        &config.oracle_mongodb_uri,
        &config.database,
        keys.clone(),
        receipts,
    )
    .await?;
    let providers = json!([{"slug":"cma_codex","base_url":config.advertised_origin,"kind":"responses_http","max_inflight":4,
        "paths":{"agent_card":"/card","binding":"/bindings/{binding_id}","conversation":"/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses","recovery":{"response":"/api/v2/responses/{response_id}","events":"/api/v2/responses/{response_id}/events"}}}]);
    let env = json!({"CMAEG_MONGODB_URI":config.gateway_mongodb_uri,"CMAEG_MONGODB_DATABASE":config.database,"CMAEG_MASTER_KEYS":master,
        "CMAEG_CMA_BASE_URL":config.advertised_origin,"CMAEG_NYXID_BASE_URL":config.advertised_origin,"CMAEG_CMA_SERVICE_TOKEN":service_bearer,
        "CMAEG_NYXID_CLIENT_ID":uuid::Uuid::new_v4().to_string(),"CMAEG_NYXID_CLIENT_SECRET":random_secret()?,
        "CMAEG_WEB_ORIGIN":"https://eg-diagnostic.invalid","CMAEG_PUBLIC_URL":"http://127.0.0.1:18999",
        "CMAEG_ROUTED_CHANNEL_RECORDS":record_id,"CMAEG_ROUTED_ADMISSION_ENABLED":"true","CMAEG_ROUTED_RECOVERY_ENABLED":"true",
        "CMAEG_REPLICAS_ENABLED":config.replicas_enabled.to_string(),"CMAEG_PROVIDERS":providers.to_string(),"CMAEG_ACCEPT_BUDGET_MS":"20000","CMAEG_DRAIN_SECONDS":"15",
        "RUST_LOG":"cmaeg=info,cmaeg::ingress::admission_trace=debug,cmaeg::records::mongo::replica_observation=debug"});
    private_json(
        &config.private_output.join("bootstrap.json"),
        &json!({"env":env,"events":roots,"channel_id":channel_id,"cookie_key_base64":random_secret()?}),
    )?;
    let collections: Vec<_> = database
        .list_collections()
        .await
        .map_err(|_| "fixture schema inventory")?
        .try_collect()
        .await
        .map_err(|_| "fixture schema inventory")?;
    let mut bindings = Vec::new();
    for collection in collections {
        let options =
            serde_json::to_vec(&collection.options).map_err(|_| "fixture schema encoding")?;
        let indexes: Vec<_> = database
            .collection::<Document>(&collection.name)
            .list_indexes()
            .await
            .map_err(|_| "fixture index inventory")?
            .try_collect()
            .await
            .map_err(|_| "fixture index inventory")?;
        let indexes = serde_json::to_vec(&indexes).map_err(|_| "fixture index encoding")?;
        bindings.push(json!({"name":collection.name,"options_sha256":hash(&options),"indexes_sha256":hash(&indexes)}));
    }
    private_json(
        &config.private_output.join("schema-binding.json"),
        &json!({"database_sha256":hash(config.database.as_bytes()),"collections":bindings}),
    )?;
    let ledger = super::ledger::Ledger::open(&config.private_output.join("effects.jsonl"))?;
    admin.shutdown().await;
    Ok(Arc::new(Shared {
        config,
        provider_bearer,
        channel_key,
        events: Mutex::new(events),
        ledger: Mutex::new(ledger),
        oracle,
        run_lock: tokio::sync::Mutex::new(()),
        channel_id: channel_id.to_string(),
    }))
}

pub(super) fn channel(
    origin: &str,
    provider_bearer: &str,
) -> Result<ChannelDefinition, &'static str> {
    let record_id = uuid::Uuid::new_v4().to_string();
    let owner = uuid::Uuid::new_v4().to_string();
    let mut channel: Value = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .map_err(|_| "fixture channel template")?;
    channel["record_id"] = record_id.clone().into();
    channel["owner"]["subject"] = owner.clone().into();
    channel["profile"]["metadata"]["owner"]["subject"] = owner.clone().into();
    channel["provider"]["binding_id"] = uuid::Uuid::new_v4().to_string().into();
    channel["sources"] = json!([{"type":"nyxid_relay","issuer":"https://eg-diagnostic.invalid","jwks_url":format!("{}/jwks",origin),"key_id":"key-1","route_ids":["route-1"],"platform":"telegram","bot_username":"fixture_bot","admission":{"type":"all"}}]);
    channel["partition"]["key"] = "conversation_and_sender".into();
    channel["routing"] = json!({"version":1,"default_consumer":{"name":"lead"},"consumers":[],"default_targets":["default"]});
    channel["reply"]["ack_after_ms"] = 0.into();
    channel["reply"]["auto_final"] = true.into();
    channel["secrets"]["provider_credential"] = provider_bearer.into();
    channel["secrets"]["agent_key_hash"] =
        format!("sha256:{}", hash(provider_bearer.as_bytes())).into();
    channel["secrets"]["channel_key"] = random_secret()?.into();
    let mut definition: ChannelDefinition =
        serde_json::from_value(channel).map_err(|_| "fixture channel decode")?;
    definition.profile_digest = definition.profile.digest();
    definition
        .validate()
        .map_err(|_| "fixture channel contract")?;
    Ok(definition)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generated_channel_preserves_valid_ordinary_relay_routing_and_one_final_reply() {
        let provider = random_secret().unwrap();
        let definition = channel("http://127.0.0.1:19100", &provider).unwrap();
        definition.validate().unwrap();
        let value = definition.to_blob_json();
        assert_eq!(value["routing"]["default_targets"], json!(["default"]));
        assert_eq!(value["reply"]["ack_after_ms"], 0);
        assert_eq!(value["reply"]["auto_final"], true);
        assert_eq!(value["sources"][0]["type"], "nyxid_relay");
        assert_eq!(
            value["sources"][0]["jwks_url"],
            "http://127.0.0.1:19100/jwks"
        );
        assert_ne!(
            channel("http://127.0.0.1:19100", &provider)
                .unwrap()
                .record_id,
            definition.record_id
        );
    }
}
