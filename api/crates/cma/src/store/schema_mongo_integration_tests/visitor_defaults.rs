use super::*;
use crate::store::{
    mongo::MongoConnectionPolicy, settings_defaults::InitialSettings, MongoAdmissionCeilingsStore,
    MongoAdmissionControlStore, MongoSandboxStore,
};
use cma_service_auth::mongodb_authority::MongoAuthorityMode;
use layer2_core::{
    admission::{AdmissionCeilings, AdmissionCeilingsStore, AdmissionControlStore},
    sandbox::SandboxPolicySource,
    ActivityService, PlatformSettingsStore, SubjectId,
};
use std::sync::Arc;

async fn seed(client: &Client, name: &str, initial: InitialSettings, source: &str) {
    match source {
        "budget" => {
            MongoSandboxStore::open_with_schema(
                client,
                name,
                crate::store::checkpoint_storage::test_storage(name),
                &mut crate::store::schema::StartupObserver::new(Default::default()),
                initial,
            )
            .await
            .unwrap();
        }
        "control" => {
            MongoAdmissionControlStore::open_with_defaults(client, name, initial)
                .await
                .unwrap()
                .change(0, &SubjectId::new("operator"), "test stop", true, 100)
                .await
                .unwrap();
        }
        "ceilings" => {
            MongoAdmissionCeilingsStore::open_with_defaults(client, name, 0, initial)
                .await
                .unwrap()
                .replace(
                    0,
                    &SubjectId::new("operator"),
                    "test ceiling",
                    AdmissionCeilings {
                        registrations: Some(10),
                        ..Default::default()
                    },
                    100,
                )
                .await
                .unwrap();
        }
        _ => panic!("unknown initializer"),
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn every_initializer_agrees_with_absent_settings_read_in_both_modes() {
    for enabled in [false, true] {
        let initial = InitialSettings::new(enabled);
        for source in ["budget", "control", "ceilings"] {
            let (client, name) = database().await;
            let store = MongoPlatformSettingsStore::open_with_defaults(&client, &name, initial)
                .await
                .unwrap();
            let before = store.read().await.unwrap();
            assert_eq!(before.revision, 0);
            assert_eq!(before.settings, initial.platform());
            assert_eq!(
                client
                    .database(&name)
                    .collection::<Document>(SETTINGS)
                    .count_documents(doc! {})
                    .await
                    .unwrap(),
                0,
                "reading defaults must not persist settings"
            );
            seed(&client, &name, initial, source).await;
            assert_eq!(
                store.read().await.unwrap(),
                before,
                "{source}, enabled={enabled}"
            );
            let opposite = MongoPlatformSettingsStore::open_with_defaults(
                &client,
                &name,
                InitialSettings::new(!enabled),
            )
            .await
            .unwrap();
            assert_eq!(opposite.read().await.unwrap(), before);
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn saved_settings_and_revision_survive_all_initializers_and_flag_changes() {
    for enabled in [false, true] {
        for (revision, limit) in [(0_i64, 0_u32), (7, 1), (19, 4)] {
            let (client, name) = database().await;
            let initial = InitialSettings::new(enabled);
            let store = MongoPlatformSettingsStore::open_with_defaults(&client, &name, initial)
                .await
                .unwrap();
            let mut saved = layer2_core::PlatformSettings::default();
            saved.limits.visitor_max_sandboxes = limit;
            saved.limits.visitor_max_launches_per_day = 3;
            saved.inactivity.pause_after_seconds = 1800;
            insert(&client, &name, SETTINGS, doc! {
                "_id": "platform", "document_schema_version": 1_i64,
                "revision": revision, "settings": mongodb::bson::serialize_to_bson(&saved).unwrap(),
                "preserved_marker": "synthetic",
            }).await;
            let expected = store.read().await.unwrap();
            for source in ["budget", "control", "ceilings"] {
                seed(&client, &name, initial, source).await;
                assert_eq!(
                    store.read().await.unwrap(),
                    expected,
                    "{source}, enabled={enabled}"
                );
            }
            seed(&client, &name, InitialSettings::new(!enabled), "budget").await;
            assert_eq!(store.read().await.unwrap(), expected);
            let persisted = client
                .database(&name)
                .collection::<Document>(SETTINGS)
                .find_one(doc! {"_id": "platform"})
                .await
                .unwrap()
                .unwrap();
            assert_eq!(persisted.get_str("preserved_marker").unwrap(), "synthetic");
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn production_startup_injects_the_same_default_into_admin_and_sandbox_policy() {
    for enabled in [false, true] {
        let (client, name) = database().await;
        let password = uuid::Uuid::new_v4().simple().to_string();
        client
            .database(&name)
            .run_command(doc! {
                "createUser": "visitor-defaults-runtime", "pwd": &password,
                "roles": [{"role": "readWrite", "db": &name}, {"role": "dbAdmin", "db": &name}],
            })
            .await
            .unwrap();
        let mut uri = url::Url::parse(&std::env::var("CHRONO_TEST_MONGODB_URI").unwrap()).unwrap();
        uri.set_username("visitor-defaults-runtime").unwrap();
        uri.set_password(Some(&password)).unwrap();
        uri.query_pairs_mut().append_pair("authSource", &name);
        let uri = uri.to_string();
        let connect = |enabled, pending_home_reconciliation| {
            let uri = uri.clone();
            let name = name.clone();
            async move {
                MongoStores::connect_with_policy(
                    &uri,
                    &name,
                    "visitor-defaults-fixture",
                    &auth_key(),
                    Duration::from_secs(86_400),
                    ("runtime-fixture", None),
                    MongoConnectionPolicy {
                        authority: MongoAuthorityMode::SharedSchema,
                        agent_manager_replicas: false,
                        startup_schema: Default::default(),
                        shared_observation: false,
                        initial_settings: InitialSettings::new(enabled),
                        released: None,
                        runtime_pointer_required: false,
                    },
                )
                .await
                .unwrap()
                .configure_admission_ceilings(0, false, pending_home_reconciliation)
                .await
                .unwrap()
            }
        };
        let stores = connect(enabled, false).await;
        assert!(!stores.admission_ceilings.pending_homes.enabled());
        let before = stores.settings.read().await.unwrap();
        assert_eq!(before.settings, InitialSettings::new(enabled).platform());
        let service = Arc::new(layer2_core::PlatformSettingsService::new(
            stores.settings.clone(),
            ActivityService::new(stores.activity.clone()),
        ));
        let policy = crate::sandbox::LiveSandboxPolicy::new(service, Vec::new());
        assert_eq!(
            u32::from(
                policy
                    .current_sandbox_policy()
                    .await
                    .unwrap()
                    .max_sandboxes_per_visitor
            ),
            before.settings.limits.visitor_max_sandboxes,
        );
        let reopened = connect(!enabled, true).await;
        assert!(reopened.admission_ceilings.pending_homes.enabled());
        assert_eq!(reopened.settings.read().await.unwrap(), before);
        use layer2_core::sandbox::store::{
            SandboxAdmission, SandboxAdmissionName, SandboxAdmissionResult, SandboxAdmissionStore,
            SandboxStoreError,
        };
        let limit = policy
            .current_sandbox_policy()
            .await
            .unwrap()
            .max_sandboxes_per_visitor;
        let results = futures_util::future::join_all((0..6).map(|index| {
            let store = if index % 2 == 0 {
                &stores.sandbox_model
            } else {
                &reopened.sandbox_model
            };
            async move {
                let sandbox = crate::store::sandbox_admission_mongo_integration_tests::sandbox(
                    &format!("visitor_{index}"),
                    &format!("launch_{index}"),
                );
                let name = SandboxAdmissionName::Allocate(sandbox.name.clone());
                store
                    .admit_sandbox(
                        SandboxAdmission::new(sandbox, name, limit, "2026-09-05T00:00:00Z".into())
                            .unwrap(),
                    )
                    .await
            }
        }))
        .await;
        let mut created = 0;
        let mut refused = 0;
        for result in results {
            match result {
                Ok(SandboxAdmissionResult::Created(_)) => created += 1,
                Err(SandboxStoreError::LimitReached {
                    limit: actual,
                    live,
                }) => {
                    assert_eq!((actual, live), (limit, limit));
                    refused += 1;
                }
                other => panic!("unexpected visitor admission: {other:?}"),
            }
        }
        assert_eq!((created, refused), (limit, 6 - limit));

        let mut saved = before.settings;
        saved.limits.visitor_max_sandboxes = 7;
        let saved = stores
            .settings
            .replace(before.revision, &saved)
            .await
            .unwrap();
        let restarted = connect(enabled, true).await;
        assert_eq!(restarted.settings.read().await.unwrap(), saved);
        client
            .database(&name)
            .collection::<Document>(SETTINGS)
            .delete_one(doc! {"_id": "platform"})
            .await
            .unwrap();
        restarted
            .admission_ceilings
            .replace(
                0,
                &SubjectId::new("operator"),
                "seed configured defaults",
                AdmissionCeilings {
                    registrations: Some(10),
                    ..Default::default()
                },
                100,
            )
            .await
            .unwrap();
        assert_eq!(
            restarted.settings.read().await.unwrap().settings,
            InitialSettings::new(enabled).platform(),
            "configuring pending-home recovery must preserve visitor defaults"
        );
        client
            .database(&name)
            .run_command(doc! {"dropUser": "visitor-defaults-runtime"})
            .await
            .unwrap();
        client.database(&name).drop().await.unwrap();
    }
}
