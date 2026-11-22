//! Real settings configuration preserves permanent rejection and infrastructure errors.
use super::*;
use crate::store::{
    activity::MongoActivityStore, settings::MongoPlatformSettingsStore,
    trigger_definitions::integration_tests::Seeded,
};
use layer2_core::{
    ActivityAction, ActivityQuery, ActivityService, ActivityStore, Authorization, Caller,
    Capability, CoreError, PlatformDefaults, PlatformSettings, PlatformSettingsService,
    PlatformSettingsStore,
};
use mongodb::{options::ClientOptions, Client};
use std::sync::Arc;

pub(super) async fn service(
    seed: &Seeded,
    client: &Client,
) -> (
    Arc<MongoPlatformSettingsStore>,
    Arc<MongoActivityStore>,
    PlatformSettingsService,
    Caller,
) {
    let settings = Arc::new(
        MongoPlatformSettingsStore::open(client, &seed.database)
            .await
            .unwrap(),
    );
    let activity = Arc::new(
        MongoActivityStore::open(
            client,
            &seed.database,
            std::time::Duration::from_secs(86400),
        )
        .await
        .unwrap(),
    );
    let service =
        PlatformSettingsService::new(settings.clone(), ActivityService::new(activity.clone()));
    let caller = Caller::new(
        SubjectId::new("settings-operator"),
        Authorization::new([Capability::ConfigurePlatform]),
    );
    (settings, activity, service, caller)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn configure_platform_rejects_fenced_defaults_permanently_without_writing_or_auditing() {
    let seed = seeded("round3_settings_service_reference").await;
    let db = seed.client.database(&seed.database);
    let (store, activity, service, caller) = service(&seed, &seed.client).await;
    let initial = PlatformSettings {
        defaults: PlatformDefaults {
            sandbox_profile: seed.target.sandbox_profile.clone(),
            workspace_profile: seed.target.workspace_profile.clone(),
            agent_profile: seed.target.agent_profile.clone(),
        },
        ..PlatformSettings::default()
    };
    let previous = store.replace(0, &initial).await.unwrap();
    let mut added = seed
        .profiles
        .agent_profile(&seed.target.agent_profile.id)
        .await
        .unwrap()
        .unwrap();
    added.id = AgentProfileId("agp_settings_service_fenced".into());
    seed.profiles
        .create_agent_profile(added.clone())
        .await
        .unwrap();
    seed.profiles
        .publish_agent_profile(&added.id, 1, NOW.into())
        .await
        .unwrap();
    let mut changed = initial;
    changed.defaults.agent_profile.id = added.id.clone();
    fence(&db, "agent_profiles", added.id.as_str(), true).await;
    let result = service
        .configure_platform(&caller, previous.revision, &changed)
        .await;
    assert!(matches!(result, Err(CoreError::InvalidRequest(_))));
    assert_eq!(store.read().await.unwrap(), previous);
    assert!(activity
        .list(&ActivityQuery::default())
        .await
        .unwrap()
        .items
        .is_empty());
    fence(&db, "agent_profiles", added.id.as_str(), false).await;
    let configured = service
        .configure_platform(&caller, previous.revision, &changed)
        .await
        .unwrap();
    assert_eq!(configured.revision, previous.revision + 1);
    assert_eq!(configured.settings.defaults, changed.defaults);
    let records = activity
        .list(&ActivityQuery::default())
        .await
        .unwrap()
        .items;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].action, ActivityAction::Configure);
    assert_eq!(records[0].actor.as_ref(), Some(&caller.subject));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn configure_platform_keeps_real_store_failures_unavailable_and_retries_normally() {
    let seed = seeded("round3_settings_service_infrastructure").await;
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI")
        .expect("CHRONO_TEST_MONGODB_FAILPOINT_URI must name a failpoint-enabled replica set");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let app_name = format!("cma-settings-service-{}", uuid::Uuid::new_v4().simple());
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    let client = Client::with_options(options).unwrap();
    let (store, activity, service, caller) = service(&seed, &client).await;
    let previous = store
        .replace(0, &PlatformSettings::default())
        .await
        .unwrap();
    let mut changed = previous.settings.clone();
    changed.inactivity.pause_after_seconds += 60;
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand", "mode":{"times":1_i32},
            "data":{"failCommands":["findAndModify"], "appName":app_name, "errorCode":1_i32},
        })
        .await
        .unwrap();
    let result = service
        .configure_platform(&caller, previous.revision, &changed)
        .await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
        .await
        .expect("disable the isolated failpoint before checking the service result");
    assert!(matches!(result, Err(CoreError::RuntimeUnavailable(_))));
    assert_eq!(store.read().await.unwrap(), previous);
    assert!(activity
        .list(&ActivityQuery::default())
        .await
        .unwrap()
        .items
        .is_empty());
    let configured = service
        .configure_platform(&caller, previous.revision, &changed)
        .await
        .unwrap();
    assert_eq!(configured.revision, previous.revision + 1);
    assert_eq!(
        configured.settings.inactivity.pause_after_seconds,
        changed.inactivity.pause_after_seconds
    );
    let records = activity
        .list(&ActivityQuery::default())
        .await
        .unwrap()
        .items;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].action, ActivityAction::Configure);
    seed.client.database(&seed.database).drop().await.unwrap();
}
