//! Replica-set integration coverage for Trigger definition persistence.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use futures_util::future::join_all;
use layer2_core::ids::{AgentProfileId, SandboxProfileId, SubjectId, WorkspaceProfileId};
use layer2_core::profiles::{
    AgentProfile, AgentProfileContent, AgentProfileRef, AgentProfileStore, AgentProvider,
    ModelAccess, PermissionLevel, ProfileCapacityBounds, ProfileVisibility, SandboxProfile,
    SandboxProfileContent, SandboxProfileRef, SandboxProfileStore, ThinkingEffort,
    WorkspaceLocation, WorkspaceProfile, WorkspaceProfileContent, WorkspaceProfileRef,
    WorkspaceProfileStore,
};
use layer2_core::trigger_domain::{
    AgentReusePolicy, ButtonLabel, PlacementUpdate, Presentation, ReusePolicy, SandboxReusePolicy,
    Theme, Trigger, TriggerAvailability, TriggerConsent, TriggerContent, TriggerId,
    TriggerPageRequest, TriggerStore, TriggerStoreError, TriggerTarget, WorkspaceReusePolicy,
};
use mongodb::bson::{doc, Bson, Document};
use mongodb::Client;

use super::super::sandbox_mongo::MongoProfileStore;
use super::{MongoTriggerDefinitionStore, TRIGGER_DEFINITIONS, TRIGGER_DEFINITION_REVISIONS};

const T1: &str = "2026-09-05T01:00:00Z";
const T2: &str = "2026-09-05T02:00:00Z";
const T3: &str = "2026-09-05T03:00:00Z";
static DATABASE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(super) struct Seeded {
    pub(super) client: Client,
    pub(super) database: String,
    pub(super) profiles: Arc<MongoProfileStore>,
    pub(super) store: MongoTriggerDefinitionStore,
    pub(super) owner: SubjectId,
    pub(super) target: TriggerTarget,
}

async fn database(prefix: &str) -> (Client, String) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(uri)
        .await
        .expect("test Mongo connects");
    let sequence = DATABASE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let database = format!("cma_820_{prefix}_{}_{sequence}", std::process::id());
    (client, database)
}

pub(super) async fn seeded(prefix: &str) -> Seeded {
    let (client, database) = database(prefix).await;
    let profiles = Arc::new(MongoProfileStore::open(&client, &database).await.unwrap());
    let owner = SubjectId("publisher".into());
    let agent_id = AgentProfileId("agp_trigger_test".into());
    let workspace_id = WorkspaceProfileId("wkp_trigger_test".into());
    let sandbox_id = SandboxProfileId("sbp_trigger_test".into());
    let agent = AgentProfile {
        id: agent_id.clone(),
        owner: owner.clone(),
        name: "Trigger agent".into(),
        description: "Checks the request".into(),
        visibility: ProfileVisibility::Public,
        forked_from: None,
        uses_count: 0,
        draft: AgentProfileContent {
            isolated_agent: false,
            model: Some("gpt-5".into()),
            thinking_effort: ThinkingEffort::High,
            permission_level: PermissionLevel::Guarded,
            skills: vec![],
            system_prompt: "Check carefully".into(),
        },
        draft_version: 1,
        created_at: T1.into(),
        updated_at: T1.into(),
        retired_at: None,
    };
    profiles.create_agent_profile(agent).await.unwrap();
    profiles
        .publish_agent_profile(&agent_id, 1, T1.into())
        .await
        .unwrap();
    let workspace = WorkspaceProfile {
        id: workspace_id.clone(),
        owner: owner.clone(),
        name: "Trigger workspace".into(),
        description: "Prepared files".into(),
        visibility: ProfileVisibility::Public,
        forked_from: None,
        uses_count: 0,
        draft: WorkspaceProfileContent {
            location: WorkspaceLocation::Directory {
                name: "project".into(),
            },
            name: "project".into(),
            setup_commands: vec![],
            agent_profiles: vec![AgentProfileRef::new(agent_id.clone(), 1)],
        },
        draft_version: 1,
        created_at: T1.into(),
        updated_at: T1.into(),
        retired_at: None,
    };
    profiles.create_workspace_profile(workspace).await.unwrap();
    profiles
        .publish_workspace_profile(&workspace_id, 1, T1.into())
        .await
        .unwrap();
    let sandbox = SandboxProfile {
        availability: Default::default(),
        id: sandbox_id.clone(),
        owner: owner.clone(),
        name: "Trigger sandbox".into(),
        description: "Small isolated environment".into(),
        visibility: ProfileVisibility::Public,
        forked_from: None,
        uses_count: 0,
        draft: SandboxProfileContent {
            runtime_image: None,
            cpu_millis: 1_000,
            memory_mib: 2_048,
            nyxid_services: vec![],
            agent_provider: AgentProvider::Codex,
            model_access: ModelAccess::ChronoaiManaged,
            max_workspaces: 2,
            max_agents_per_workspace: 2,
            workspace_profiles: vec![WorkspaceProfileRef::new(workspace_id.clone(), 1)],
            software_install_commands: vec!["apt-get update".into()],
            environment_variables: vec![],
        },
        draft_version: 1,
        created_at: T1.into(),
        updated_at: T1.into(),
        retired_at: None,
    };
    profiles.create_sandbox_profile(sandbox).await.unwrap();
    profiles
        .publish_sandbox_profile(
            &sandbox_id,
            1,
            T1.into(),
            ProfileCapacityBounds {
                min_cpu_millis: 100,
                max_cpu_millis: 8_000,
                min_memory_mib: 128,
                max_memory_mib: 16_384,
            },
            &[],
        )
        .await
        .unwrap();
    let store = MongoTriggerDefinitionStore::open(&client, &database, Arc::clone(&profiles))
        .await
        .unwrap();
    Seeded {
        client,
        database,
        profiles,
        store,
        owner,
        target: TriggerTarget {
            sandbox_profile: SandboxProfileRef::new(sandbox_id, 1),
            workspace_profile: WorkspaceProfileRef::new(workspace_id, 1),
            agent_profile: AgentProfileRef::new(agent_id, 1),
        },
    }
}

fn content(target: TriggerTarget, title: &str) -> TriggerContent {
    TriggerContent {
        presentation: Presentation {
            title: title.into(),
            description: "Open a prepared workspace".into(),
            button_label: ButtonLabel::RunWithCma,
            theme: Theme::Auto,
            signup_hint: None,
        },
        target,
        reuse: ReusePolicy {
            sandbox: SandboxReusePolicy::ReuseExisting,
            workspace: WorkspaceReusePolicy::AlwaysNew,
            agent: AgentReusePolicy::Ask,
        },
        first_message: "Check this proof".into(),
        quick_messages: Vec::new(),
        consent: TriggerConsent {
            always_review: true,
        },
    }
}

pub(super) fn trigger(id: &str, owner: &SubjectId, target: &TriggerTarget) -> Trigger {
    Trigger::new(
        TriggerId(id.into()),
        owner.clone(),
        content(target.clone(), id),
        T1.into(),
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn schema_v1_startup_creates_indexes_and_refuses_future_documents() {
    let seed = seeded("schema").await;
    let names = seed
        .client
        .database(&seed.database)
        .list_collection_names()
        .await
        .unwrap();
    assert!(names.iter().any(|name| name == TRIGGER_DEFINITIONS));
    assert!(names
        .iter()
        .any(|name| name == TRIGGER_DEFINITION_REVISIONS));
    assert!(seed
        .store
        .definitions
        .list_index_names()
        .await
        .unwrap()
        .contains(&"trigger_definitions_public_id".to_string()));
    seed.client.database(&seed.database).drop().await.unwrap();

    let (client, database) = database("future").await;
    let profiles = Arc::new(MongoProfileStore::open(&client, &database).await.unwrap());
    client
        .database(&database)
        .collection::<Document>(TRIGGER_DEFINITIONS)
        .insert_one(doc! { "_id": "future", "document_schema_version": 2_i64 })
        .await
        .unwrap();
    let error = match MongoTriggerDefinitionStore::open(&client, &database, profiles).await {
        Ok(_) => panic!("future Trigger schema must stop startup"),
        Err(error) => error,
    };
    assert!(matches!(error, layer2_core::StoreError::Malformed(_)));
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn authoring_public_lookup_preview_and_retirement_are_durable() {
    let seed = seeded("workflow").await;
    for id in ["trg_a", "trg_b", "trg_c"] {
        seed.store
            .create_trigger(trigger(id, &seed.owner, &seed.target))
            .await
            .unwrap();
    }
    assert_eq!(
        seed.store
            .create_trigger(trigger("trg_a", &seed.owner, &seed.target))
            .await,
        Err(TriggerStoreError::Duplicate)
    );
    let first_page = seed
        .store
        .list_own_triggers(
            &seed.owner,
            &TriggerPageRequest {
                limit: 2,
                cursor: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(first_page.next.as_deref(), Some("trg_b"));
    assert_eq!(first_page.items[0].id.as_str(), "trg_a");

    let id = TriggerId("trg_a".into());
    let saved = seed
        .store
        .save_draft(
            &id,
            &seed.owner,
            1,
            content(seed.target.clone(), "First public title"),
            T2.into(),
        )
        .await
        .unwrap();
    assert_eq!(saved.draft_version, 2);
    assert!(matches!(
        seed.store
            .save_draft(&id, &seed.owner, 1, saved.draft.clone(), T2.into())
            .await,
        Err(TriggerStoreError::Conflict { actual: 2, .. })
    ));
    let first = seed
        .store
        .publish_trigger(&id, &seed.owner, 2, T2.into())
        .await
        .unwrap();
    let first_revision = seed
        .store
        .revisions
        .find_one(doc! { "trigger_id": id.as_str(), "revision": 1_i64 })
        .await
        .unwrap()
        .unwrap();
    let placement = seed
        .store
        .create_placement(
            &id,
            &seed.owner,
            "https://example.com/readme#run".into(),
            "README".into(),
        )
        .await
        .unwrap();
    assert_eq!(placement.source_url, "https://example.com/readme");
    seed.store
        .update_placement(
            &id,
            &seed.owner,
            &placement.id,
            PlacementUpdate {
                source_url: "https://example.com/docs".into(),
                label: "Docs".into(),
            },
        )
        .await
        .unwrap();
    seed.store
        .retire_placement(&id, &seed.owner, &placement.id, T3.into())
        .await
        .unwrap();
    assert!(seed
        .store
        .list_placements(&id, &seed.owner, false)
        .await
        .unwrap()
        .is_empty());

    seed.store
        .save_draft(
            &id,
            &seed.owner,
            2,
            content(seed.target.clone(), "Latest public title"),
            T3.into(),
        )
        .await
        .unwrap();
    let second = seed
        .store
        .publish_trigger(&id, &seed.owner, 3, T3.into())
        .await
        .unwrap();
    assert_eq!(first.public_id, second.public_id);
    assert_eq!(second.revision.revision, 2);
    assert_eq!(
        seed.store
            .trigger_revision(&id, &first.revision.digest)
            .await
            .unwrap(),
        Some(first.revision.clone())
    );
    assert_eq!(
        seed.store
            .revisions
            .find_one(doc! { "trigger_id": id.as_str(), "revision": 1_i64 })
            .await
            .unwrap()
            .unwrap(),
        first_revision
    );
    let restarted =
        MongoTriggerDefinitionStore::open(&seed.client, &seed.database, Arc::clone(&seed.profiles))
            .await
            .unwrap();
    assert_eq!(
        restarted
            .trigger_by_public_id(&first.public_id)
            .await
            .unwrap()
            .unwrap()
            .revision,
        second.revision
    );
    assert_eq!(
        restarted
            .preview_by_public_id(&first.public_id, "Publisher".into())
            .await
            .unwrap()
            .unwrap()
            .title,
        "Latest public title"
    );
    assert!(matches!(
        restarted
            .guard_agent_profile_retirement(&seed.target.agent_profile.id)
            .await,
        Err(TriggerStoreError::ProfileReferenced { .. })
    ));
    restarted
        .retire_trigger(&id, &seed.owner, T3.into())
        .await
        .unwrap();
    restarted
        .guard_agent_profile_retirement(&seed.target.agent_profile.id)
        .await
        .unwrap();
    assert_eq!(
        restarted
            .trigger_by_public_id(&first.public_id)
            .await
            .unwrap()
            .unwrap()
            .availability,
        TriggerAvailability::Retired
    );
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn deletion_hides_ownership_and_preserves_every_published_trigger() {
    let seed = seeded("deletion").await;
    let draft_id = TriggerId("trg_delete_draft".into());
    seed.store
        .create_trigger(trigger(draft_id.as_str(), &seed.owner, &seed.target))
        .await
        .unwrap();
    assert_eq!(
        seed.store
            .delete_unpublished_trigger(&draft_id, &SubjectId::new("other"))
            .await,
        Err(TriggerStoreError::NotFound)
    );
    seed.store
        .delete_unpublished_trigger(&draft_id, &seed.owner)
        .await
        .unwrap();
    assert!(seed
        .store
        .trigger(&draft_id, &seed.owner)
        .await
        .unwrap()
        .is_none());

    let published_id = TriggerId("trg_delete_published".into());
    seed.store
        .create_trigger(trigger(published_id.as_str(), &seed.owner, &seed.target))
        .await
        .unwrap();
    seed.store
        .publish_trigger(&published_id, &seed.owner, 1, T2.into())
        .await
        .unwrap();
    assert_eq!(
        seed.store
            .delete_unpublished_trigger(&published_id, &seed.owner)
            .await,
        Err(TriggerStoreError::AlreadyPublished)
    );
    assert!(seed
        .store
        .trigger(&published_id, &seed.owner)
        .await
        .unwrap()
        .is_some());
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn exact_revision_reads_validate_the_stored_content_digest() {
    let seed = seeded("exact_revision").await;
    let id = TriggerId("trg_exact_revision".into());
    seed.store
        .create_trigger(trigger(id.as_str(), &seed.owner, &seed.target))
        .await
        .unwrap();
    let public = seed
        .store
        .publish_trigger(&id, &seed.owner, 1, T2.into())
        .await
        .unwrap();
    assert_eq!(
        seed.store
            .trigger_revision(&id, &public.revision.digest)
            .await
            .unwrap(),
        Some(public.revision.clone())
    );

    seed.store
        .revisions
        .update_one(
            doc! { "trigger_id": id.as_str(), "revision": 1_i64 },
            doc! { "$set": { "record.content.presentation.title": "corrupt title" } },
        )
        .await
        .unwrap();
    assert!(matches!(
        seed.store
            .trigger_revision(&id, &public.revision.digest)
            .await,
        Err(TriggerStoreError::Corrupt(_))
    ));
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publication_rolls_back_profile_fences_and_counters_are_atomic() {
    let seed = seeded("atomic").await;
    let broken_id = TriggerId("trg_broken".into());
    let mut broken_target = seed.target.clone();
    broken_target.agent_profile.revision = 99;
    seed.store
        .create_trigger(trigger("trg_broken", &seed.owner, &broken_target))
        .await
        .unwrap();
    assert_eq!(
        seed.store
            .publish_trigger(&broken_id, &seed.owner, 1, T2.into())
            .await
            .unwrap_err()
            .code(),
        "trigger_reference_unavailable"
    );
    assert_eq!(
        seed.store
            .revisions
            .count_documents(doc! { "trigger_id": broken_id.as_str() })
            .await
            .unwrap(),
        0
    );
    for collection in [
        &seed.profiles.sandbox_profiles,
        &seed.profiles.workspace_profiles,
        &seed.profiles.agent_profiles,
    ] {
        let profile = collection.find_one(doc! {}).await.unwrap().unwrap();
        assert_eq!(profile.get("trigger_publication_fence"), None);
    }

    let id = TriggerId("trg_atomic".into());
    seed.store
        .create_trigger(trigger("trg_atomic", &seed.owner, &seed.target))
        .await
        .unwrap();
    seed.store
        .publish_trigger(&id, &seed.owner, 1, T2.into())
        .await
        .unwrap();
    for collection in [
        &seed.profiles.sandbox_profiles,
        &seed.profiles.workspace_profiles,
        &seed.profiles.agent_profiles,
    ] {
        let profile = collection.find_one(doc! {}).await.unwrap().unwrap();
        assert_eq!(
            profile.get("trigger_publication_fence"),
            Some(&Bson::Int64(1))
        );
    }
    join_all((0..64).map(|_| seed.store.record_launch(&id)))
        .await
        .into_iter()
        .for_each(|result| {
            result.unwrap();
        });
    join_all((0..64).map(|_| seed.store.record_agent_ready(&id)))
        .await
        .into_iter()
        .for_each(|result| {
            result.unwrap();
        });
    let stored = seed.store.trigger(&id, &seed.owner).await.unwrap().unwrap();
    assert_eq!(stored.stats.launches_total, 64);
    assert_eq!(stored.stats.agents_ready_total, 64);
    seed.client.database(&seed.database).drop().await.unwrap();
}
