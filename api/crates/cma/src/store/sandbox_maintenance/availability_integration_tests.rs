use super::*;
use crate::store::schema::DOCUMENT_SCHEMA_VERSION;
use crate::store::MongoProfileStore;
use layer2_core::profiles::{
    MarketplaceLocator, MarketplaceStore, ProfileCapacityBounds, ProfileVisibility, SandboxProfile,
    SandboxProfileStore,
};
use layer2_core::sandbox::store::{
    SandboxLifecycleCommand, SandboxLifecycleCommandResult, SandboxLifecycleCommandStore,
};
use mongodb::bson::Document;

async fn profile_fixture(
    client: &Client,
    name: &str,
    sandbox: &Sandbox,
) -> (MongoProfileStore, SandboxProfile) {
    let profiles = MongoProfileStore::open(client, name).await.unwrap();
    let profile = SandboxProfile {
        availability: Default::default(),
        id: sandbox.profile.id.clone(),
        owner: sandbox.owner.clone(),
        name: "Availability".into(),
        description: String::new(),
        visibility: ProfileVisibility::Public,
        forked_from: None,
        uses_count: 0,
        draft: sandbox.profile.content.clone(),
        draft_version: 1,
        created_at: IDLE_AT.into(),
        updated_at: IDLE_AT.into(),
        retired_at: None,
    };
    profiles
        .create_sandbox_profile(profile.clone())
        .await
        .unwrap();
    (profiles, profile)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn keep_alive_policy_is_durable_preserves_manual_pause_and_pinned_environment() {
    let (client, name, store) = database().await;
    let (sandbox, _) = fixture(&store, "sbx_keepalive").await;
    let (profiles, mut profile) = profile_fixture(&client, &name, &sandbox).await;
    profile.availability.always_keep_alive = true;
    profile.draft_version = 2;
    profiles
        .save_sandbox_draft(1, profile.clone())
        .await
        .unwrap();
    assert_eq!(
        profiles.sandbox_profile(&profile.id).await.unwrap(),
        Some(profile.clone())
    );
    assert_eq!(
        store
            .schedule_inactivity_pause(pause(&sandbox.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    assert_eq!(
        store.get_sandbox(&sandbox.id).await.unwrap(),
        Some(sandbox.clone())
    );
    assert!(store
        .sandbox_events(&sandbox.id, 0, 100)
        .await
        .unwrap()
        .is_empty());
    let manual = store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            sandbox.id.clone(),
            sandbox.owner.clone(),
            LATER.into(),
            None,
        ))
        .await
        .unwrap();
    assert!(matches!(manual, SandboxLifecycleCommandResult::Accepted(_)));
    let (second, _) = fixture(&store, "sbx_keepalive_disabled").await;
    assert_eq!(second.profile.id, profile.id);
    profile.availability.always_keep_alive = false;
    profile.draft_version = 3;
    profiles.save_sandbox_draft(2, profile).await.unwrap();
    assert_eq!(
        store
            .schedule_inactivity_pause(pause(&second.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    assert_eq!(
        store
            .get_sandbox(&second.id)
            .await
            .unwrap()
            .unwrap()
            .profile,
        second.profile
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn concurrent_keep_alive_edit_invalidates_stale_inactivity_transaction() {
    let (client, name, store) = database().await;
    let (sandbox, _) = fixture(&store, "sbx_policy_race").await;
    let (profiles, mut profile) = profile_fixture(&client, &name, &sandbox).await;
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    store
        .sandbox_profiles
        .find_one(doc! { "_id": profile.id.as_str() })
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    profile.availability.always_keep_alive = true;
    profile.draft_version = 2;
    profiles.save_sandbox_draft(1, profile).await.unwrap();
    let outcome = apply(
        &store,
        &MaintenanceRequest::Inactivity(pause(&sandbox.id)),
        &mut session,
    )
    .await;
    assert!(matches!(outcome, Err(ApplyError::Driver(ref error)) if is_transient(error)));
    let _ = session.abort_transaction().await;
    assert_eq!(
        store.get_sandbox(&sandbox.id).await.unwrap(),
        Some(sandbox.clone())
    );
    assert_eq!(
        store
            .schedule_inactivity_pause(pause(&sandbox.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn warmup_policy_persists_after_publication_and_resets_on_private_copy() {
    let (client, name, store) = database().await;
    let sandbox = active_sandbox("sbx_warmup_policy");
    let (profiles, mut profile) = profile_fixture(&client, &name, &sandbox).await;
    profile.availability.warmup_enabled = true;
    profile.availability.warmup_count = 5;
    profile.availability.always_keep_alive = true;
    profile.draft_version = 2;
    assert!(profiles
        .save_sandbox_draft(1, profile.clone())
        .await
        .is_err());
    let bounds = ProfileCapacityBounds {
        min_cpu_millis: 100,
        max_cpu_millis: 16000,
        min_memory_mib: 128,
        max_memory_mib: 32768,
    };
    let scanner: [String; 0] = [];
    let pinned = profiles
        .publish_sandbox_profile(&profile.id, 1, IDLE_AT.into(), bounds, &scanner)
        .await
        .unwrap();
    profiles
        .save_sandbox_draft(1, profile.clone())
        .await
        .unwrap();
    assert_eq!(
        profiles.sandbox_profile(&profile.id).await.unwrap(),
        Some(profile.clone())
    );
    let locator = MarketplaceLocator::Sandbox(profile.id.clone());
    let layer2_core::profiles::ForkedProfile::Sandbox(fork) = profiles
        .fork_profile(&SubjectId("visitor".into()), &locator, 1, LATER.into())
        .await
        .unwrap()
    else {
        panic!("expected private Sandbox Profile copy");
    };
    assert_ne!(fork.id, profile.id);
    assert_eq!(fork.visibility, ProfileVisibility::Private);
    assert_eq!(fork.availability, Default::default());
    assert_eq!(fork.draft, pinned.content);
    profiles
        .set_profile_visibility(
            &profile.owner,
            &locator,
            ProfileVisibility::Private,
            LATER.into(),
            bounds,
            &scanner,
        )
        .await
        .unwrap();
    let unshared = profiles
        .sandbox_profile(&profile.id)
        .await
        .unwrap()
        .unwrap();
    assert!(!unshared.availability.warmup_enabled);
    assert!(unshared.availability.always_keep_alive);
    assert_eq!(unshared.availability.warmup_count, 5);
    assert_eq!(
        profiles
            .sandbox_profile_revision(&profile.id, 1)
            .await
            .unwrap(),
        Some(pinned)
    );
    // The maintenance adapter shares the same durable Profile authority.
    assert!(store
        .sandbox_profiles
        .find_one(doc! { "_id": profile.id.as_str() })
        .await
        .unwrap()
        .is_some());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn removed_catalogue_policy_keeps_the_exact_instance_active_after_store_reopen() {
    let (client, name, store) = database().await;
    let (sandbox, _) = fixture(&store, "sbx_catalogue_cleanup").await;
    let (profiles, profile) = profile_fixture(&client, &name, &sandbox).await;
    let rows = client
        .database(&name)
        .collection::<Document>("sandbox_retained_availability");
    rows.insert_one(doc!{"_id":sandbox.id.as_str(),"document_schema_version":DOCUMENT_SCHEMA_VERSION,"owner":sandbox.owner.as_str(),"sandbox_created_at":&sandbox.created_at,"profile_id":sandbox.profile.id.as_str(),"always_keep_alive":true}).await.unwrap();
    profiles
        .sandbox_profiles
        .delete_one(doc! {"_id":profile.id.as_str()})
        .await
        .unwrap();
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened
            .schedule_inactivity_pause(pause(&sandbox.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    assert_eq!(
        reopened.get_sandbox(&sandbox.id).await.unwrap(),
        Some(sandbox.clone())
    );
    rows.update_one(
        doc! {"_id":sandbox.id.as_str()},
        doc! {"$set":{"owner":"different"}},
    )
    .await
    .unwrap();
    assert!(reopened
        .schedule_inactivity_pause(pause(&sandbox.id))
        .await
        .is_err());
    assert_eq!(
        reopened.get_sandbox(&sandbox.id).await.unwrap(),
        Some(sandbox.clone())
    );
    rows.update_one(
        doc! {"_id":sandbox.id.as_str()},
        doc! {"$set":{"owner":sandbox.owner.as_str(),"always_keep_alive":false}},
    )
    .await
    .unwrap();
    assert_eq!(
        reopened
            .schedule_inactivity_pause(pause(&sandbox.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    client.database(&name).drop().await.unwrap();
}
