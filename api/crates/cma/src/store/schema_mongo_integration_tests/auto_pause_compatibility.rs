//! Reader shapes copied from d50bbe72d. Unchanged nested types are reused.

use std::sync::Arc;

use layer2_core::platform::{
    CheckpointSettings, MarketplaceSettings, PlatformDefaults, PlatformLimits, RetentionSettings,
    TriggerSettings,
};
use layer2_core::profiles::{SandboxProfileContent, SandboxProfileSnapshot};
use layer2_core::sandbox::store::{SandboxMaintenanceStore, SandboxSessionStore, SandboxStore};
use layer2_core::{
    ActivityService, PlatformSettings, PlatformSettingsService, PlatformSettingsStore,
    SandboxProfileId,
};
use mongodb::bson::{deserialize_from_document, doc, serialize_to_document, Document};
use serde::{Deserialize, Serialize};

use super::{database, MongoPlatformSettingsStore, SETTINGS};
use crate::store::sandbox_mongo::{MongoSandboxStore, SANDBOXES};
use crate::store::sandbox_mongo_integration_tests::sandbox;
use crate::store::sandboxes::{read_sandbox, sandbox_document};

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PredecessorInactivitySettings {
    pause_after_seconds: u64,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PredecessorPlatformSettings {
    defaults: PlatformDefaults,
    inactivity: PredecessorInactivitySettings,
    #[serde(default)]
    checkpoints: CheckpointSettings,
    retention: RetentionSettings,
    limits: PlatformLimits,
    triggers: TriggerSettings,
    marketplace: MarketplaceSettings,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PredecessorSnapshot {
    id: SandboxProfileId,
    revision: u32,
    digest: String,
    content: SandboxProfileContent,
}

fn assert_predecessor_settings(document: &Document, settings: &PlatformSettings) {
    let stored = document.get_document("settings").unwrap();
    let old: PredecessorPlatformSettings = deserialize_from_document(stored.clone()).unwrap();
    assert_eq!(serialize_to_document(&old).unwrap(), *stored);
    assert_eq!(
        stored.get_document("inactivity").unwrap(),
        &doc! { "pause_after_seconds": settings.inactivity.pause_after_seconds as i64 }
    );
    assert_eq!(
        deserialize_from_document::<PlatformSettings>(stored.clone()).unwrap(),
        *settings
    );
}

#[test]
fn predecessor_snapshots_round_trip_without_a_preference_and_reject_explicit_values() {
    let mut snapshot = sandbox("sbx_compat_snapshot").profile;
    let neutral = serialize_to_document(&snapshot).unwrap();
    assert!(!neutral.contains_key("always_on"));
    let old: PredecessorSnapshot = deserialize_from_document(neutral.clone()).unwrap();
    assert_eq!(serialize_to_document(&old).unwrap(), neutral);
    assert_eq!(
        deserialize_from_document::<SandboxProfileSnapshot>(neutral).unwrap(),
        snapshot
    );
    for always_on in [false, true] {
        snapshot.always_on = Some(always_on);
        let document = serialize_to_document(&snapshot).unwrap();
        assert_eq!(document.get_bool("always_on").unwrap(), always_on);
        assert!(deserialize_from_document::<PredecessorSnapshot>(document.clone()).is_err());
        assert_eq!(
            deserialize_from_document::<SandboxProfileSnapshot>(document).unwrap(),
            snapshot
        );
    }
}

#[test]
fn predecessor_settings_reject_each_new_key_even_when_its_value_is_neutral() {
    let original = serialize_to_document(&PlatformSettings::default()).unwrap();
    let old: PredecessorPlatformSettings = deserialize_from_document(original.clone()).unwrap();
    assert_eq!(serialize_to_document(&old).unwrap(), original);
    for extra in [
        doc! { "auto_pause_enabled": false },
        doc! { "auto_pause_enabled": true },
        doc! { "grace_started_at": "2026-11-05T00:00:00Z" },
    ] {
        let mut document = original.clone();
        document
            .get_document_mut("inactivity")
            .unwrap()
            .extend(extra);
        assert!(
            deserialize_from_document::<PredecessorPlatformSettings>(document.clone()).is_err()
        );
        assert!(deserialize_from_document::<PlatformSettings>(document).is_ok());
    }
}

#[test]
fn desktop_fence_is_outside_the_predecessor_typed_aggregate() {
    let sandbox = sandbox("sbx_compat_fence");
    let mut document = sandbox_document(&sandbox, 0).unwrap();
    assert!(!document.contains_key("desktop_activity_fence"));
    document.insert("desktop_activity_fence", 3_i64);
    // d50bbe72d uses the same read_sandbox/read_record implementation: only
    // `record` is deserialized, and query metadata is checked by named fields.
    assert_eq!(read_sandbox(&document).unwrap(), sandbox);
    let profile = document
        .get_document("record")
        .unwrap()
        .get_document("profile")
        .unwrap();
    assert!(deserialize_from_document::<PredecessorSnapshot>(profile.clone()).is_ok());
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn settings_saves_preserve_old_readers_until_enablement() {
    let (client, name) = database().await;
    let store = Arc::new(
        MongoPlatformSettingsStore::open(&client, &name)
            .await
            .unwrap(),
    );
    let collection = client.database(&name).collection::<Document>(SETTINGS);
    let mut settings = PlatformSettings::default();
    let stored = store.replace(0, &settings).await.unwrap();
    let raw = collection
        .find_one(doc! { "_id": "platform" })
        .await
        .unwrap()
        .unwrap();
    assert_predecessor_settings(&raw, &settings);
    assert_eq!(store.read().await.unwrap(), stored);

    // An ordinary threshold/other-settings save on an existing singleton.
    settings.inactivity.pause_after_seconds = 1_200;
    settings.limits.max_sandboxes_per_user = 12;
    let stored = store.replace(stored.revision, &settings).await.unwrap();
    let raw = collection
        .find_one(doc! { "_id": "platform" })
        .await
        .unwrap()
        .unwrap();
    assert_predecessor_settings(&raw, &settings);
    assert!(store.replace(0, &settings).await.is_err());

    settings.inactivity.auto_pause_enabled = true;
    settings.inactivity.grace_started_at = Some("2026-11-05T00:00:00Z".into());
    let enabled = store.replace(stored.revision, &settings).await.unwrap();
    let raw = collection
        .find_one(doc! { "_id": "platform" })
        .await
        .unwrap()
        .unwrap();
    let saved = raw.get_document("settings").unwrap();
    assert_eq!(saved, &serialize_to_document(&settings).unwrap());
    let inactivity = saved.get_document("inactivity").unwrap();
    assert!(inactivity.get_bool("auto_pause_enabled").unwrap());
    assert_eq!(
        inactivity.get_str("grace_started_at").unwrap(),
        "2026-11-05T00:00:00Z"
    );
    assert!(deserialize_from_document::<PredecessorPlatformSettings>(saved.clone()).is_err());
    assert_eq!(store.read().await.unwrap(), enabled);

    // Whole-value $set must remove prior new keys when the service disables it.
    let service = PlatformSettingsService::new(
        store.clone(),
        ActivityService::new(Arc::new(
            super::MongoActivityStore::open(&client, &name, std::time::Duration::from_secs(3600))
                .await
                .unwrap(),
        )),
    );
    settings.inactivity.auto_pause_enabled = false;
    let caller = layer2_core::Caller::new(
        layer2_core::SubjectId::new("compat-operator"),
        layer2_core::Authorization::new([layer2_core::Capability::ConfigurePlatform]),
    );
    let disabled = service
        .configure_platform(&caller, enabled.revision, &settings)
        .await
        .unwrap();
    assert!(disabled.settings.inactivity.grace_started_at.is_none());
    let raw = collection
        .find_one(doc! { "_id": "platform" })
        .await
        .unwrap()
        .unwrap();
    assert_predecessor_settings(&raw, &disabled.settings);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB replica set"]
async fn sandbox_saves_and_desktop_sessions_preserve_neutral_snapshots() {
    let (client, name) = database().await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let collection = client.database(&name).collection::<Document>(SANDBOXES);
    let mut current = sandbox("sbx_compat_save");
    store.create_sandbox(current.clone()).await.unwrap();
    current.name = "renamed".into();
    current.revision += 1;
    store
        .save_sandbox(current.revision - 1, current.clone())
        .await
        .unwrap();
    store
        .open_sandbox_session(&current.id, "compat-desktop", "2026-11-05T00:01:00Z")
        .await
        .unwrap();
    let raw = collection
        .find_one(doc! { "_id": current.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(raw.get_i64("desktop_activity_fence").unwrap(), 1);
    let profile = raw
        .get_document("record")
        .unwrap()
        .get_document("profile")
        .unwrap();
    assert!(!profile.contains_key("always_on"));
    assert!(deserialize_from_document::<PredecessorSnapshot>(profile.clone()).is_ok());
    assert_eq!(
        store.get_sandbox(&current.id).await.unwrap().unwrap(),
        current
    );
    store
        .close_sandbox_session(&current.id, "compat-desktop")
        .await
        .unwrap();
    for always_on in [true, false] {
        current = store
            .set_sandbox_always_on(current.id.clone(), current.revision, always_on)
            .await
            .unwrap();
        let raw = collection
            .find_one(doc! { "_id": current.id.as_str() })
            .await
            .unwrap()
            .unwrap();
        let profile = raw
            .get_document("record")
            .unwrap()
            .get_document("profile")
            .unwrap();
        assert_eq!(profile.get_bool("always_on").unwrap(), always_on);
        assert!(deserialize_from_document::<PredecessorSnapshot>(profile.clone()).is_err());
        assert_eq!(
            store.get_sandbox(&current.id).await.unwrap().unwrap(),
            current
        );
    }
    client.database(&name).drop().await.unwrap();
}
