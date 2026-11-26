use super::bots::MongoBotStore;
use layer2_core::{bots::*, profiles::content_digest, SubjectId};
use mongodb::{bson::doc, Client};
use serde_json::json;

pub(super) fn bot(owner: &str, key: &str) -> Bot {
    let now = layer2_core::now_rfc3339();
    let agent = json!({"model":"test","thinking_effort":"medium","permission_level":"guarded","skills":[],"system_prompt":""});
    let sandbox = json!({"cpu_millis":1000,"memory_mib":2048,"nyxid_services":[],"agent_provider":"codex","model_access":"chronoai_managed","max_workspaces":1,"max_agents_per_workspace":1,"workspace_profiles":[],"software_install_commands":[],"environment_variables":[]});
    let workspace = json!({"location":{"type":"directory","name":"project"},"name":"project","setup_commands":[],"agent_profiles":[]});
    serde_json::from_value(json!({"bot_id":format!("bot_{}",uuid::Uuid::new_v4().simple()),"owner":owner,"request":{"name":"test","external_bot_id":uuid::Uuid::new_v4().to_string(),"profile":{"id":"agp_test","revision":1},"admission":{"type":"all"},"reply":{"auto_final":true,"ack_after_ms":2000}},
        "profiles":{"resolved_model":"test","agent":{"profile_id":"agp_test","revision":1,"digest":content_digest(&agent).unwrap(),"content":agent,"published_at":now},"sandbox":{"profile_id":"sbp_test","revision":1,"digest":content_digest(&sandbox).unwrap(),"content":sandbox,"published_at":now},"workspace":{"profile_id":"wkp_test","revision":1,"digest":content_digest(&workspace).unwrap(),"content":workspace,"published_at":now}},
        "provider":"registered","record_id":uuid::Uuid::new_v4().to_string(),"resources":{"channel":null,"route_id":null,"credentials":{},"replacement":null},"state":"creating","work":{"id":uuid::Uuid::new_v4().to_string(),"operation":"create","step":"admitted","generation":1,"holder":null,"expires_at":null},"generation":1,"revision":1,"creation_key_hash":key.repeat(64),"request_fingerprint":"b".repeat(64),"created_at":now,"updated_at":now,"last_test":null})).unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable MongoDB replica set"]
async fn bot_creation_and_worker_cas_survive_store_reopen() {
    let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let database = format!("cma_bots_test_{}", uuid::Uuid::new_v4().simple());
    let a = MongoBotStore::open(&client, &database).await.unwrap();
    let b = MongoBotStore::open(&client, &database).await.unwrap();
    let first = bot("owner", "a");
    let second = bot("owner", "a");
    let (left, right) = tokio::join!(a.insert(&first), b.insert(&second));
    assert_ne!(left.is_ok(), right.is_ok());
    let winner = a
        .creation(&SubjectId::new("owner"), &"a".repeat(64))
        .await
        .unwrap()
        .unwrap();
    let mut replacement = winner.clone();
    replacement.revision += 1;
    replacement.work.as_mut().unwrap().holder = Some("new-worker".into());
    let (left, right) = tokio::join!(a.replace(1, &replacement), b.replace(1, &replacement));
    assert_ne!(left.is_ok(), right.is_ok());
    drop(a);
    drop(b);
    let reopened = MongoBotStore::open(&client, &database).await.unwrap();
    assert_eq!(
        reopened.get(&winner.bot_id).await.unwrap().unwrap(),
        replacement
    );
    assert!(reopened
        .list(&SubjectId::new("foreign"), None, 100)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        reopened.pending(None, 100).await.unwrap(),
        [replacement.clone()]
    );
    let raw = client
        .database(&database)
        .collection::<mongodb::bson::Document>("bots")
        .find_one(doc! {"_id":&winner.bot_id})
        .await
        .unwrap()
        .unwrap();
    assert!(raw
        .get_document("bot")
        .unwrap()
        .get_document("resources")
        .unwrap()
        .get_document("credentials")
        .unwrap()
        .get("agent_key")
        .is_none());
    replacement.revision += 1;
    replacement.owner = SubjectId::new("foreign");
    assert_eq!(
        reopened.replace(2, &replacement).await,
        Err(BotError::Conflict)
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable MongoDB replica set"]
async fn worker_revisions_are_atomic_owner_fenced_and_survive_reopen() {
    use super::event_workers::MongoEventWorkerProfileStore;
    use layer2_core::event_workers::*;
    use layer2_core::trigger_domain::TriggerProfileReader;
    let seed = super::trigger_definitions::integration_tests::seeded("worker_revision_pins").await;
    let client = seed.client.clone();
    let database = seed.database.clone();
    let a = MongoEventWorkerProfileStore::open(&client, &database)
        .await
        .unwrap();
    let b = MongoEventWorkerProfileStore::open(&client, &database)
        .await
        .unwrap();
    let mut bot = bot("owner", "a");
    let resolved = seed
        .profiles
        .read_trigger_profiles(&seed.target)
        .await
        .unwrap();
    bot.profiles.agent = resolved.agent;
    bot.profiles.workspace = resolved.workspace;
    bot.profiles.sandbox = resolved.sandbox;
    let source = EventWorkerProfileInput {
        name: "Worker".into(),
        description: String::new(),
        agent_profile: seed.target.agent_profile,
        workspace_profile: seed.target.workspace_profile,
        sandbox_profile: seed.target.sandbox_profile,
    };
    let snapshot = projection::snapshot(&projection::WorkerProjection {
        name: &source.name,
        owner: &bot.owner,
        provider: "cma_codex",
        profiles: &bot.profiles,
    })
    .unwrap();
    let first = EventWorkerProfile {
        provider: "cma_codex".into(),
        id: format!("ewp_{}", uuid::Uuid::new_v4().simple()),
        owner: bot.owner,
        revision: 1,
        source,
        profiles: bot.profiles,
        digest: format!("sha256:{}", content_digest(&snapshot).unwrap()),
        snapshot,
        created_at: bot.created_at,
        updated_at: bot.updated_at,
        retired_at: None,
        creation_fingerprint: "a".repeat(64),
    };
    let (left, right) = tokio::join!(a.save(None, &first), b.save(None, &first));
    assert_ne!(left.is_ok(), right.is_ok());
    let mut next = first.clone();
    next.revision = 2;
    next.retired_at = Some(next.updated_at.clone());
    let (left, right) = tokio::join!(a.save(Some(1), &next), b.save(Some(1), &next));
    assert_ne!(left.is_ok(), right.is_ok());
    drop(a);
    drop(b);
    let reopened = MongoEventWorkerProfileStore::open(&client, &database)
        .await
        .unwrap();
    assert_eq!(
        reopened.get(&first.id, None).await.unwrap(),
        Some(next.clone())
    );
    assert_eq!(
        reopened.get(&first.id, Some(1)).await.unwrap(),
        Some(first.clone())
    );
    assert_eq!(reopened.get(&first.id, Some(2)).await.unwrap(), Some(next));
    assert!(reopened
        .list(&first.owner, None, 100)
        .await
        .unwrap()
        .is_empty());
    assert!(reopened
        .list(&SubjectId::new("foreign"), None, 100)
        .await
        .unwrap()
        .is_empty());
    // A record under another identity must not become authority for that key.
    let revisions = client
        .database(&database)
        .collection::<mongodb::bson::Document>("event_worker_profile_revisions");
    let mut corrupt = revisions
        .find_one(doc! {"_id":format!("{}:1", first.id)})
        .await
        .unwrap()
        .unwrap();
    corrupt.insert("_id", format!("{}:3", first.id));
    revisions.insert_one(corrupt).await.unwrap();
    assert_eq!(
        reopened.get(&first.id, Some(3)).await,
        Err(BotError::Unavailable)
    );
    client.database(&database).drop().await.unwrap();
}

struct NoProfileResolution;
#[async_trait::async_trait]
impl BotProfileResolver for NoProfileResolution {
    async fn resolve(&self, _: &SubjectId, _: &CreateBot) -> Result<BotProfiles, BotError> {
        panic!("an already admitted operation must reuse its immutable profiles")
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable MongoDB replica set"]
async fn bot_effect_custody_survives_independent_clients_expiry_and_old_writer_cas() {
    use std::{
        sync::Arc,
        time::{Duration, SystemTime},
    };
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let first_client = Client::with_uri_str(&uri).await.unwrap();
    let second_client = Client::with_uri_str(&uri).await.unwrap();
    let database = format!("cma_bot_effects_test_{}", uuid::Uuid::new_v4().simple());
    let a = Arc::new(MongoBotStore::open(&first_client, &database).await.unwrap());
    let b = Arc::new(
        MongoBotStore::open(&second_client, &database)
            .await
            .unwrap(),
    );
    let first = BotService::new(
        a.clone(),
        Arc::new(NoProfileResolution),
        "registered".into(),
    )
    .with_effect_reservations(true);
    let second = BotService::new(
        b.clone(),
        Arc::new(NoProfileResolution),
        "registered".into(),
    )
    .with_effect_reservations(true);
    // The enabled worker can upgrade only untouched admitted legacy work.
    let initial = bot("owner", "a");
    a.insert(&initial).await.unwrap();
    let mut owned = first
        .claim(&initial.bot_id, "first", SystemTime::now())
        .await
        .unwrap()
        .unwrap();
    let mut before_reservation = owned.clone();
    first
        .reserve_effect(&mut owned, "first", "mint_agent_key")
        .await
        .unwrap();
    let reserved = b.get(&initial.bot_id).await.unwrap().unwrap();
    assert_eq!(reserved, owned);
    assert!(reserved.work.as_ref().unwrap().effect.is_some());
    assert!(second
        .claim(
            &initial.bot_id,
            "successor",
            SystemTime::now() + Duration::from_secs(3600)
        )
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        first
            .checkpoint(&mut before_reservation, "first", "stale")
            .await,
        Err(BotError::Conflict)
    );
    let records = first_client
        .database(&database)
        .collection::<mongodb::bson::Document>("bots");
    let raw = records
        .find_one(doc! {"_id": &initial.bot_id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(raw.get_i32("document_schema_version").unwrap(), 2);
    // Exercise the real startup guard against serializer output, not only the decoder.
    super::schema::refuse_unknown_versions(&first_client.database(&database), &["bots"])
        .await
        .unwrap();
    // This is the exact old binary's replace predicate, even supplied the
    // current revision. Its payload cannot erase the new reservation.
    let legacy = records
        .replace_one(
            doc! {"_id": &initial.bot_id,
            "revision": i64::try_from(owned.revision).unwrap(), "document_schema_version": 1},
            raw.clone(),
        )
        .await
        .unwrap();
    assert_eq!(legacy.matched_count, 0);
    let mut forged = owned.clone();
    forged.revision += 1;
    forged.work.as_mut().unwrap().holder = Some("successor".into());
    assert_eq!(
        b.replace(owned.revision, &forged).await,
        Err(BotError::Conflict)
    );
    // A definitive late response publishes its resource identity atomically
    // with clearing the exact reservation, without a lease-expiry takeover.
    owned.resources.credentials.agent_key_id = Some(uuid::Uuid::new_v4().to_string());
    owned.resources.credentials.agent_key_hash = Some("c".repeat(64));
    first.settle_effect(&mut owned, "first").await.unwrap();
    let settled = b.get(&initial.bot_id).await.unwrap().unwrap();
    assert_eq!(settled, owned);
    assert!(settled.work.as_ref().unwrap().effect.is_none());
    assert!(settled.resources.credentials.agent_key_id.is_some());
    let mut cleanup = second
        .claim(
            &initial.bot_id,
            "successor",
            SystemTime::now() + Duration::from_secs(3600),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        cleanup.work.as_ref().unwrap().operation,
        BotOperation::Cleanup
    );
    assert_eq!(cleanup.resources, settled.resources);
    let mut stale = reserved;
    assert_eq!(
        first.settle_effect(&mut stale, "first").await,
        Err(BotError::Conflict)
    );
    assert_eq!(b.get(&initial.bot_id).await.unwrap().unwrap(), cleanup);
    cleanup.resources = BotResources::default();
    let state = cleanup.state.clone();
    second
        .complete(&mut cleanup, "successor", state)
        .await
        .unwrap();
    let raw = records
        .find_one(doc! {"_id": &initial.bot_id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        raw.get_i32("document_schema_version").unwrap(),
        2,
        "completion cannot lower the old-writer floor"
    );
    let off = BotService::new(
        b.clone(),
        Arc::new(NoProfileResolution),
        "registered".into(),
    );
    let caller = layer2_core::Caller::new(
        SubjectId::new("owner"),
        layer2_core::Authorization::new([layer2_core::Capability::ManageOwnBots]),
    );
    assert_eq!(
        off.admit(&caller, &initial.bot_id, BotOperation::Delete)
            .await,
        Err(BotError::Conflict)
    );
    let mut corrupt = raw;
    corrupt.insert("document_schema_version", 1);
    let work = mongodb::bson::serialize_to_bson(&owned.work).unwrap();
    corrupt
        .get_document_mut("bot")
        .unwrap()
        .insert("work", work);
    records
        .replace_one(doc! {"_id": &initial.bot_id}, corrupt)
        .await
        .unwrap();
    assert_eq!(b.get(&initial.bot_id).await, Err(BotError::Unavailable));
    first_client.database(&database).drop().await.unwrap();
}
