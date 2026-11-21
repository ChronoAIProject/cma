//! Real Mongo admission coverage for every mutable Profile retention root.
mod trigger_and_fork_tests;

use crate::store::trigger_definitions::integration_tests::{seeded, trigger};
use layer2_core::profiles::{
    AgentProfileStore, MarketplaceError, MarketplaceLocator, MarketplaceStore, ProfileStoreError,
    SandboxProfileStore, WorkspaceProfileStore,
};
use layer2_core::trigger_domain::{
    TriggerId, TriggerReferenceError, TriggerStore, TriggerStoreError,
};
use layer2_core::{AgentProfileId, SandboxProfileId, SubjectId, WorkspaceProfileId};
use mongodb::{
    bson::{doc, Document},
    Database,
};

const NOW: &str = "2026-11-14T00:00:00Z";

async fn fence(db: &Database, collection: &str, id: &str, enabled: bool) {
    let update = if enabled {
        doc! {"$set":{"account_erasure_fenced":true}}
    } else {
        doc! {"$unset":{"account_erasure_fenced":""}}
    };
    assert_eq!(
        db.collection::<Document>(collection)
            .update_one(doc! {"_id":id}, update)
            .await
            .unwrap()
            .matched_count,
        1
    );
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn workspace_draft_create_rejects_fenced_child_without_inserting_a_dangling_root() {
    let seed = seeded("erasure_workspace_draft_create").await;
    let db = seed.client.database(&seed.database);
    let mut candidate = seed
        .profiles
        .workspace_profile(&seed.target.workspace_profile.id)
        .await
        .unwrap()
        .unwrap();
    candidate.owner = SubjectId::new("survivor");
    candidate.id = WorkspaceProfileId("wkp_before_fence".into());
    seed.profiles
        .create_workspace_profile(candidate.clone())
        .await
        .unwrap();
    fence(
        &db,
        "agent_profiles",
        seed.target.agent_profile.id.as_str(),
        true,
    )
    .await;
    candidate.id = WorkspaceProfileId("wkp_fenced_child".into());
    assert!(matches!(
        seed.profiles
            .create_workspace_profile(candidate.clone())
            .await,
        Err(ProfileStoreError::ReferenceUnavailable)
    ));
    assert!(seed
        .profiles
        .workspace_profile(&candidate.id)
        .await
        .unwrap()
        .is_none());
    let mut unrelated = candidate.clone();
    unrelated.id = WorkspaceProfileId("wkp_unrelated_inline".into());
    unrelated.internal = true;
    unrelated.visibility = layer2_core::profiles::ProfileVisibility::Private;
    unrelated.draft.agent_profiles.clear();
    seed.profiles
        .create_workspace_profile(unrelated)
        .await
        .unwrap();
    fence(
        &db,
        "agent_profiles",
        seed.target.agent_profile.id.as_str(),
        false,
    )
    .await;
    seed.profiles
        .create_workspace_profile(candidate)
        .await
        .unwrap();
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn workspace_draft_save_rejects_new_fenced_child_and_preserves_the_previous_draft() {
    let seed = seeded("erasure_workspace_draft_save").await;
    let db = seed.client.database(&seed.database);
    let mut baseline = seed
        .profiles
        .workspace_profile(&seed.target.workspace_profile.id)
        .await
        .unwrap()
        .unwrap();
    baseline.owner = SubjectId::new("survivor");
    baseline.id = WorkspaceProfileId("wkp_survivor_draft".into());
    let references = std::mem::take(&mut baseline.draft.agent_profiles);
    seed.profiles
        .create_workspace_profile(baseline.clone())
        .await
        .unwrap();
    fence(
        &db,
        "agent_profiles",
        seed.target.agent_profile.id.as_str(),
        true,
    )
    .await;
    let mut candidate = baseline.clone();
    candidate.draft.agent_profiles = references;
    candidate.draft_version = 2;
    candidate.updated_at = NOW.into();
    assert!(matches!(
        seed.profiles
            .save_workspace_draft(1, candidate.clone())
            .await,
        Err(ProfileStoreError::ReferenceUnavailable)
    ));
    assert_eq!(
        seed.profiles.workspace_profile(&baseline.id).await.unwrap(),
        Some(baseline)
    );
    fence(
        &db,
        "agent_profiles",
        seed.target.agent_profile.id.as_str(),
        false,
    )
    .await;
    seed.profiles
        .save_workspace_draft(1, candidate.clone())
        .await
        .unwrap();
    assert_eq!(
        seed.profiles
            .workspace_profile(&candidate.id)
            .await
            .unwrap(),
        Some(candidate)
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_draft_create_rejects_fenced_child_without_inserting_a_dangling_root() {
    let seed = seeded("erasure_sandbox_draft_create").await;
    let db = seed.client.database(&seed.database);
    let mut candidate = seed
        .profiles
        .sandbox_profile(&seed.target.sandbox_profile.id)
        .await
        .unwrap()
        .unwrap();
    candidate.owner = SubjectId::new("survivor");
    candidate.id = SandboxProfileId("sbp_before_fence".into());
    seed.profiles
        .create_sandbox_profile(candidate.clone())
        .await
        .unwrap();
    fence(
        &db,
        "workspace_profiles",
        seed.target.workspace_profile.id.as_str(),
        true,
    )
    .await;
    candidate.id = SandboxProfileId("sbp_fenced_child".into());
    assert!(matches!(
        seed.profiles
            .create_sandbox_profile(candidate.clone())
            .await,
        Err(ProfileStoreError::ReferenceUnavailable)
    ));
    assert!(seed
        .profiles
        .sandbox_profile(&candidate.id)
        .await
        .unwrap()
        .is_none());
    let mut unrelated = candidate.clone();
    unrelated.id = SandboxProfileId("sbp_unrelated_inline".into());
    unrelated.internal = true;
    unrelated.visibility = layer2_core::profiles::ProfileVisibility::Private;
    unrelated.draft.workspace_profiles.clear();
    seed.profiles
        .create_sandbox_profile(unrelated)
        .await
        .unwrap();
    fence(
        &db,
        "workspace_profiles",
        seed.target.workspace_profile.id.as_str(),
        false,
    )
    .await;
    seed.profiles
        .create_sandbox_profile(candidate)
        .await
        .unwrap();
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_draft_save_rejects_new_fenced_child_and_preserves_the_previous_draft() {
    let seed = seeded("erasure_sandbox_draft_save").await;
    let db = seed.client.database(&seed.database);
    let mut baseline = seed
        .profiles
        .sandbox_profile(&seed.target.sandbox_profile.id)
        .await
        .unwrap()
        .unwrap();
    baseline.owner = SubjectId::new("survivor");
    baseline.id = SandboxProfileId("sbp_survivor_draft".into());
    let references = std::mem::take(&mut baseline.draft.workspace_profiles);
    seed.profiles
        .create_sandbox_profile(baseline.clone())
        .await
        .unwrap();
    fence(
        &db,
        "workspace_profiles",
        seed.target.workspace_profile.id.as_str(),
        true,
    )
    .await;
    let mut candidate = baseline.clone();
    candidate.draft.workspace_profiles = references;
    candidate.draft_version = 2;
    candidate.updated_at = NOW.into();
    assert!(matches!(
        seed.profiles.save_sandbox_draft(1, candidate.clone()).await,
        Err(ProfileStoreError::ReferenceUnavailable)
    ));
    assert_eq!(
        seed.profiles.sandbox_profile(&baseline.id).await.unwrap(),
        Some(baseline)
    );
    fence(
        &db,
        "workspace_profiles",
        seed.target.workspace_profile.id.as_str(),
        false,
    )
    .await;
    seed.profiles
        .save_sandbox_draft(1, candidate.clone())
        .await
        .unwrap();
    assert_eq!(
        seed.profiles.sandbox_profile(&candidate.id).await.unwrap(),
        Some(candidate)
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn platform_defaults_reject_new_fenced_pins_and_preserve_prior_settings() {
    use layer2_core::{PlatformDefaults, PlatformSettings, PlatformSettingsStore};
    let seed = seeded("erasure_platform_defaults_pins").await;
    let db = seed.client.database(&seed.database);
    let store =
        crate::store::settings::MongoPlatformSettingsStore::open(&seed.client, &seed.database)
            .await
            .unwrap();
    let settings = PlatformSettings {
        defaults: PlatformDefaults {
            sandbox_profile: seed.target.sandbox_profile.clone(),
            workspace_profile: seed.target.workspace_profile.clone(),
            agent_profile: seed.target.agent_profile.clone(),
        },
        ..PlatformSettings::default()
    };
    store.replace(0, &settings).await.unwrap();
    let mut added = seed
        .profiles
        .agent_profile(&seed.target.agent_profile.id)
        .await
        .unwrap()
        .unwrap();
    added.id = AgentProfileId("agp_new_platform_default".into());
    seed.profiles
        .create_agent_profile(added.clone())
        .await
        .unwrap();
    seed.profiles
        .publish_agent_profile(&added.id, 1, NOW.into())
        .await
        .unwrap();
    let mut changed = settings.clone();
    changed.defaults.agent_profile.id = added.id.clone();
    fence(&db, "agent_profiles", added.id.as_str(), true).await;
    assert!(matches!(
        store.replace(1, &changed).await,
        Err(layer2_core::StoreError::ReferenceUnavailable(_))
    ));
    assert_eq!(store.read().await.unwrap().settings, settings);
    fence(&db, "agent_profiles", added.id.as_str(), false).await;
    assert_eq!(store.replace(1, &changed).await.unwrap().settings, changed);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn unresolved_draft_and_default_placeholders_keep_the_existing_write_contract() {
    use layer2_core::{PlatformSettings, PlatformSettingsStore};
    let seed = seeded("erasure_unresolved_draft_compatibility").await;
    let db = seed.client.database(&seed.database);
    let owner = SubjectId::new("survivor");
    let mut unpublished = seed
        .profiles
        .agent_profile(&seed.target.agent_profile.id)
        .await
        .unwrap()
        .unwrap();
    unpublished.id = AgentProfileId("agp_unpublished_child".into());
    unpublished.owner = owner.clone();
    seed.profiles
        .create_agent_profile(unpublished.clone())
        .await
        .unwrap();
    let mut workspace = seed
        .profiles
        .workspace_profile(&seed.target.workspace_profile.id)
        .await
        .unwrap()
        .unwrap();
    workspace.owner = owner.clone();
    workspace.id = WorkspaceProfileId("wkp_unresolved_parent".into());
    workspace.draft.agent_profiles[0].id = unpublished.id;
    seed.profiles
        .create_workspace_profile(workspace.clone())
        .await
        .unwrap();
    workspace.draft_version = 2;
    workspace.draft.agent_profiles[0].id = AgentProfileId("agp_unresolved_child".into());
    workspace.description = "An editable unresolved workspace draft".into();
    seed.profiles
        .save_workspace_draft(1, workspace)
        .await
        .unwrap();
    let mut sandbox = seed
        .profiles
        .sandbox_profile(&seed.target.sandbox_profile.id)
        .await
        .unwrap()
        .unwrap();
    sandbox.owner = owner.clone();
    sandbox.id = SandboxProfileId("sbp_unresolved_parent".into());
    sandbox.draft.workspace_profiles[0].id = WorkspaceProfileId("wkp_unresolved_parent".into());
    seed.profiles
        .create_sandbox_profile(sandbox.clone())
        .await
        .unwrap();
    sandbox.draft_version = 2;
    sandbox.draft.workspace_profiles[0].id = WorkspaceProfileId("wkp_unresolved_child".into());
    sandbox.description = "An editable unresolved sandbox draft".into();
    seed.profiles.save_sandbox_draft(1, sandbox).await.unwrap();
    let mut target = seed.target.clone();
    target.agent_profile.id = AgentProfileId("agp_unresolved_trigger".into());
    target.workspace_profile.id = WorkspaceProfileId("wkp_unresolved_trigger".into());
    target.sandbox_profile.id = SandboxProfileId("sbp_unresolved_trigger".into());
    let id = TriggerId("trg_unresolved_draft".into());
    let candidate = trigger(id.as_str(), &owner, &target);
    seed.store.create_trigger(candidate.clone()).await.unwrap();
    let mut draft = candidate.draft;
    draft.presentation.description = "An editable unresolved Trigger draft".into();
    seed.store
        .save_draft(&id, &owner, 1, draft, NOW.into())
        .await
        .unwrap();
    let settings =
        crate::store::settings::MongoPlatformSettingsStore::open(&seed.client, &seed.database)
            .await
            .unwrap();
    settings
        .replace(0, &PlatformSettings::default())
        .await
        .unwrap();
    assert_eq!(
        settings.read().await.unwrap().settings,
        PlatformSettings::default()
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn fenced_aggregate_cannot_downgrade_a_deleted_revision_to_an_unresolved_placeholder() {
    use layer2_core::{PlatformDefaults, PlatformSettings, PlatformSettingsStore};
    let seed = seeded("erasure_fenced_missing_revision").await;
    let db = seed.client.database(&seed.database);
    fence(
        &db,
        "agent_profiles",
        seed.target.agent_profile.id.as_str(),
        true,
    )
    .await;
    db.collection::<Document>("agent_profile_revisions")
        .delete_many(doc! {"profile_id":seed.target.agent_profile.id.as_str()})
        .await
        .unwrap();
    let mut workspace = seed
        .profiles
        .workspace_profile(&seed.target.workspace_profile.id)
        .await
        .unwrap()
        .unwrap();
    workspace.owner = SubjectId::new("survivor");
    workspace.id = WorkspaceProfileId("wkp_fenced_missing_revision".into());
    assert!(matches!(
        seed.profiles.create_workspace_profile(workspace).await,
        Err(ProfileStoreError::ReferenceUnavailable)
    ));
    assert!(matches!(
        seed.store
            .create_trigger(trigger(
                "trg_fenced_missing_revision",
                &SubjectId::new("survivor"),
                &seed.target
            ))
            .await,
        Err(TriggerStoreError::Reference(
            TriggerReferenceError::Unavailable { .. }
        ))
    ));
    let store =
        crate::store::settings::MongoPlatformSettingsStore::open(&seed.client, &seed.database)
            .await
            .unwrap();
    let settings = PlatformSettings {
        defaults: PlatformDefaults {
            sandbox_profile: seed.target.sandbox_profile.clone(),
            workspace_profile: seed.target.workspace_profile.clone(),
            agent_profile: seed.target.agent_profile.clone(),
        },
        ..PlatformSettings::default()
    };
    assert!(matches!(
        store.replace(0, &settings).await,
        Err(layer2_core::StoreError::ReferenceUnavailable(_))
    ));
    assert_eq!(store.read().await.unwrap().revision, 0);
    db.drop().await.unwrap();
}

mod round3_settings_service_tests;
mod round3_transaction_tests;

mod round4_error_tests;
