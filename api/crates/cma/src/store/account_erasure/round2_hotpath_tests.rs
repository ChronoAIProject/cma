//! Command monitoring proves catalogue admission fences only read catalogue rows.
mod resource_admission_tests;

use crate::store::trigger_definitions::integration_tests::{seeded, trigger};
use layer2_core::profiles::{
    MarketplaceLocator, MarketplaceStore, ProfileUseRecorder, SandboxProfileStore,
    WorkspaceProfileStore,
};
use layer2_core::sandbox::{
    CreateSandbox, FixedSandboxCredentialPrerequisite, SandboxOrigin, SandboxPolicy, SandboxService,
};
use layer2_core::trigger_domain::{
    Launch, LaunchAuthorityClass, LaunchResolution, LaunchSeed, LaunchTrustManifest,
    RequirementsSnapshot, ReviewRequirements, TriggerId, TriggerStore,
};
use layer2_core::{
    Authorization, Caller, Capability, PlatformDefaults, PlatformSettings, PlatformSettingsStore,
    SubjectId,
};
use mongodb::{
    bson::{doc, Document},
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::sync::{Arc, Mutex};

const NOW: &str = "2026-11-14T00:00:00Z";
const CATALOGUE: [&str; 8] = [
    "agent_profiles",
    "workspace_profiles",
    "sandbox_profiles",
    "agent_profile_revisions",
    "workspace_profile_revisions",
    "sandbox_profile_revisions",
    "trigger_definitions",
    "trigger_definition_revisions",
];

#[derive(Clone, Default)]
struct Commands(Arc<Mutex<Vec<(String, Document)>>>);
impl Commands {
    async fn client(&self) -> Client {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
        let mut options = ClientOptions::parse(uri).await.unwrap();
        let observed = self.0.clone();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let CommandEvent::Started(event) = event {
                observed
                    .lock()
                    .unwrap()
                    .push((event.command_name, event.command));
            }
        }));
        Client::with_options(options).unwrap()
    }
    fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
    fn catalogue_writes(&self) -> Vec<(String, String)> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(name, command)| {
                if !["insert", "update", "delete", "findAndModify"].contains(&name.as_str()) {
                    return None;
                }
                let collection = command.get_str(name).unwrap();
                CATALOGUE
                    .contains(&collection)
                    .then(|| (name.clone(), collection.into()))
            })
            .collect()
    }
    fn assert_writes(&self, expected: &[(&str, &str)]) {
        assert_eq!(
            self.catalogue_writes(),
            expected
                .iter()
                .map(|(name, collection)| (name.to_string(), collection.to_string()))
                .collect::<Vec<_>>()
        );
        for (_, command) in self.0.lock().unwrap().iter() {
            let encoded = command.to_string();
            assert!(!encoded.contains("profile_admission_fence"));
            assert!(!encoded.contains("publisher_agent_admission_fence"));
        }
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn hotpath_draft_defaults_and_fork_only_write_the_requested_record() {
    let seed = seeded("round2_hotpath_authoring").await;
    let observed = Commands::default();
    let client = observed.client().await;
    let profiles = Arc::new(
        crate::store::sandbox_mongo::MongoProfileStore::open(&client, &seed.database)
            .await
            .unwrap(),
    );
    let settings =
        crate::store::settings::MongoPlatformSettingsStore::open(&client, &seed.database)
            .await
            .unwrap();
    let triggers = crate::store::trigger_definitions::MongoTriggerDefinitionStore::open(
        &client,
        &seed.database,
        profiles.clone(),
    )
    .await
    .unwrap();
    let mut workspace = profiles
        .workspace_profile(&seed.target.workspace_profile.id)
        .await
        .unwrap()
        .unwrap();
    workspace.id = layer2_core::WorkspaceProfileId("wkp_readonly_draft".into());
    observed.clear();
    profiles
        .create_workspace_profile(workspace.clone())
        .await
        .unwrap();
    observed.assert_writes(&[("insert", "workspace_profiles")]);
    workspace.draft_version = 2;
    observed.clear();
    profiles.save_workspace_draft(1, workspace).await.unwrap();
    observed.assert_writes(&[("update", "workspace_profiles")]);

    let defaults = PlatformSettings {
        defaults: PlatformDefaults {
            agent_profile: seed.target.agent_profile.clone(),
            workspace_profile: seed.target.workspace_profile.clone(),
            sandbox_profile: seed.target.sandbox_profile.clone(),
        },
        ..PlatformSettings::default()
    };
    observed.clear();
    settings.replace(0, &defaults).await.unwrap();
    observed.assert_writes(&[]);
    observed.clear();
    profiles
        .fork_profile(
            &SubjectId::new("visitor"),
            &MarketplaceLocator::Sandbox(seed.target.sandbox_profile.id.clone()),
            1,
            NOW.into(),
        )
        .await
        .unwrap();
    observed.assert_writes(&[("insert", "sandbox_profiles")]);

    let definition = trigger("trg_readonly_authoring", &seed.owner, &seed.target);
    observed.clear();
    triggers.create_trigger(definition.clone()).await.unwrap();
    observed.assert_writes(&[("insert", "trigger_definitions")]);
    observed.clear();
    triggers
        .save_draft(&definition.id, &seed.owner, 1, definition.draft, NOW.into())
        .await
        .unwrap();
    observed.assert_writes(&[("update", "trigger_definitions")]);
    // Publishing already fenced mutable visibility before account erasure.
    // Keep precisely those baseline writes, without extra admission counters.
    observed.clear();
    triggers
        .publish_trigger(&definition.id, &seed.owner, 2, NOW.into())
        .await
        .unwrap();
    observed.assert_writes(&[
        ("update", "sandbox_profiles"),
        ("update", "workspace_profiles"),
        ("update", "agent_profiles"),
        ("insert", "trigger_definition_revisions"),
        ("update", "trigger_definitions"),
    ]);
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn hotpath_event_worker_publication_reads_catalogue_and_refuses_fenced_pin() {
    use layer2_core::bots::BotError;
    use layer2_core::event_workers::{
        projection, EventWorkerProfile, EventWorkerProfileInput, EventWorkerProfileStore,
    };
    use layer2_core::profiles::content_digest;
    use layer2_core::trigger_domain::TriggerProfileReader;
    let seed = seeded("round2_hotpath_event_worker").await;
    let observed = Commands::default();
    let client = observed.client().await;
    let store =
        crate::store::event_workers::MongoEventWorkerProfileStore::open(&client, &seed.database)
            .await
            .unwrap();
    let resolved = seed
        .profiles
        .read_trigger_profiles(&seed.target)
        .await
        .unwrap();
    let mut bot = crate::store::bots_mongo_integration_tests::bot("visitor", "a");
    bot.profiles.agent = resolved.agent;
    bot.profiles.workspace = resolved.workspace;
    bot.profiles.sandbox = resolved.sandbox;
    let source = EventWorkerProfileInput {
        name: "Worker".into(),
        description: String::new(),
        agent_profile: seed.target.agent_profile.clone(),
        workspace_profile: seed.target.workspace_profile.clone(),
        sandbox_profile: seed.target.sandbox_profile.clone(),
    };
    let snapshot = projection::snapshot(&projection::WorkerProjection {
        name: &source.name,
        owner: &bot.owner,
        provider: "cma_codex",
        profiles: &bot.profiles,
    })
    .unwrap();
    let value = EventWorkerProfile {
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
    observed.clear();
    store.save(None, &value).await.unwrap();
    observed.assert_writes(&[]);
    let db = seed.client.database(&seed.database);
    db.collection::<Document>("agent_profiles")
        .update_one(
            doc! {"_id":seed.target.agent_profile.id.as_str()},
            doc! {"$set":{"account_erasure_fenced":true}},
        )
        .await
        .unwrap();
    let mut refused = value.clone();
    refused.id = format!("ewp_{}", uuid::Uuid::new_v4().simple());
    observed.clear();
    assert_eq!(store.save(None, &refused).await, Err(BotError::Conflict));
    observed.assert_writes(&[]);
    assert!(store.get(&refused.id, None).await.unwrap().is_none());
    assert_eq!(
        db.collection::<Document>("event_worker_profile_revisions")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    db.drop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn hotpath_marketplace_fork_tolerates_a_source_use_counter_change() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    let seed = seeded("round2_hotpath_fork_counter").await;
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let noticed = Arc::new(tokio::sync::Notify::new());
    let notify = noticed.clone();
    let (release, receiver) = mpsc::channel();
    let receiver = Mutex::new(receiver);
    let fired = AtomicBool::new(false);
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            // The fork transaction's first profile read. Every store transaction
            // now opens with a barrier read, so this is no longer its first command.
            if event.command.get_str("find").ok() == Some("sandbox_profiles")
                && event.command.get_bool("autocommit").ok() == Some(false)
                && !fired.swap(true, Ordering::SeqCst)
            {
                notify.notify_one();
                receiver
                    .lock()
                    .unwrap()
                    .recv_timeout(std::time::Duration::from_secs(30))
                    .unwrap();
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = crate::store::sandbox_mongo::MongoProfileStore::open(&client, &seed.database)
        .await
        .unwrap();
    let locator = MarketplaceLocator::Sandbox(seed.target.sandbox_profile.id.clone());
    let source = locator.clone();
    let task = tokio::spawn(async move {
        store
            .fork_profile(&SubjectId::new("visitor"), &locator, 1, NOW.into())
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(20), noticed.notified())
        .await
        .unwrap();
    seed.profiles.record_profile_use(&source).await.unwrap();
    release.send(()).unwrap();
    task.await
        .unwrap()
        .expect("a source use counter is independent of fork visibility");
    let db = seed.client.database(&seed.database);
    assert_eq!(
        db.collection::<Document>("sandbox_profiles")
            .count_documents(doc! {"owner":"visitor"})
            .await
            .unwrap(),
        1
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn hotpath_publisher_admission_reads_definition_and_exact_revision_fences() {
    let seed = seeded("round2_hotpath_publisher").await;
    let id = TriggerId("trg_readonly_publisher".into());
    seed.store
        .create_trigger(trigger(id.as_str(), &seed.owner, &seed.target))
        .await
        .unwrap();
    let public = seed
        .store
        .publish_trigger(&id, &seed.owner, 1, NOW.into())
        .await
        .unwrap();
    let launch = Launch::new(LaunchSeed {
        trigger_id: id.clone(),
        revision_digest: public.revision.digest,
        public_id: public.public_id,
        owner: SubjectId::new("visitor"),
        authority: LaunchAuthorityClass::SandboxCreator,
        placement_id: None,
        referrer_origin: None,
        requirements: RequirementsSnapshot {
            services: vec![],
            checked_at: NOW.into(),
        },
        review: ReviewRequirements {
            needs: vec![],
            can_start_now: true,
        },
        trust: LaunchTrustManifest::none(),
        resolution: LaunchResolution::pending(),
        has_first_message: true,
        command_key_hash: "a".repeat(64),
        command_fingerprint: "b".repeat(64),
        created_at: NOW.into(),
    })
    .unwrap();
    let observed = Commands::default();
    let client = observed.client().await;
    let db = client.database(&seed.database);
    let mut session = client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    observed.clear();
    assert!(
        crate::store::trigger_definitions::fence_new_agent_publisher(&db, &launch, &mut session)
            .await
            .unwrap()
    );
    session.commit_transaction().await.unwrap();
    observed.assert_writes(&[]);
    for (collection, filter) in [
        ("trigger_definitions", doc! {"_id":id.as_str()}),
        (
            "trigger_definition_revisions",
            doc! {"trigger_id":id.as_str(),"revision":1_i64},
        ),
    ] {
        seed.client
            .database(&seed.database)
            .collection::<Document>(collection)
            .update_one(
                filter.clone(),
                doc! {"$set":{"account_erasure_fenced":true}},
            )
            .await
            .unwrap();
        crate::store::sandbox_mongo::start_transaction(&mut session)
            .await
            .unwrap();
        observed.clear();
        assert!(
            !crate::store::trigger_definitions::fence_new_agent_publisher(
                &db,
                &launch,
                &mut session
            )
            .await
            .unwrap(),
            "fenced {collection} admitted a new publisher effect"
        );
        session.abort_transaction().await.unwrap();
        observed.assert_writes(&[]);
        seed.client
            .database(&seed.database)
            .collection::<Document>(collection)
            .update_one(filter, doc! {"$unset":{"account_erasure_fenced":""}})
            .await
            .unwrap();
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn hotpath_trigger_mutations_read_fence_before_replacing_definition() {
    use layer2_core::trigger_domain::{TriggerReferenceError, TriggerStoreError};
    let seed = seeded("round2_hotpath_trigger_mutations").await;
    let observed = Commands::default();
    let client = observed.client().await;
    let store = crate::store::trigger_definitions::MongoTriggerDefinitionStore::open(
        &client,
        &seed.database,
        seed.profiles.clone(),
    )
    .await
    .unwrap();
    let db = seed.client.database(&seed.database);
    for mutation in ["draft", "publication", "retirement"] {
        let definition = trigger(&format!("trg_fenced_{mutation}"), &seed.owner, &seed.target);
        seed.store.create_trigger(definition.clone()).await.unwrap();
        seed.store
            .publish_trigger(&definition.id, &seed.owner, 1, NOW.into())
            .await
            .unwrap();
        let before = db
            .collection::<Document>("trigger_definitions")
            .find_one_and_update(
                doc! {"_id":definition.id.as_str()},
                doc! {"$set":{"account_erasure_fenced":true,"account_erasure_fenced_at":NOW}},
            )
            .return_document(mongodb::options::ReturnDocument::After)
            .await
            .unwrap()
            .unwrap();
        observed.clear();
        let result = match mutation {
            "draft" => store
                .save_draft(&definition.id, &seed.owner, 1, definition.draft, NOW.into())
                .await
                .map(|_| ()),
            "publication" => store
                .publish_trigger(&definition.id, &seed.owner, 1, NOW.into())
                .await
                .map(|_| ()),
            _ => store
                .retire_trigger(&definition.id, &seed.owner, NOW.into())
                .await
                .map(|_| ()),
        };
        assert!(
            matches!(
                result,
                Err(TriggerStoreError::Reference(
                    TriggerReferenceError::Unavailable { .. }
                ))
            ),
            "fenced {mutation} must return a typed refusal: {result:?}"
        );
        observed.assert_writes(&[]);
        assert_eq!(
            db.collection::<Document>("trigger_definitions")
                .find_one(doc! {"_id":definition.id.as_str()})
                .await
                .unwrap(),
            Some(before)
        );
        assert_eq!(
            db.collection::<Document>("trigger_definition_revisions")
                .count_documents(doc! {"trigger_id":definition.id.as_str()})
                .await
                .unwrap(),
            1
        );
    }
    db.drop().await.unwrap();
}
