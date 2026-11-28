use super::*;
use crate::store::settings_barrier::test_support::{Pause, Point};
use layer2_core::{sandbox::store::SandboxLimitClass, PlatformSettingsStore};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn catalogue_admission_rechecks_current_caps_after_operator_cutoff() {
    for class in [SandboxLimitClass::Owner, SandboxLimitClass::Visitor] {
        for lowered in [0, 5] {
            let mut fixture = Fixture::new("catalogue_cutoff").await;
            fixture.executing(LaunchEffectKind::CreateSandbox).await;
            let mut sandbox = fixture.create_sandbox().await.unwrap();
            let operator = crate::store::MongoPlatformSettingsStore::open(
                &fixture.seed.client,
                &fixture.seed.database,
            )
            .await
            .unwrap();
            let mut settings = operator.read().await.unwrap();
            settings.settings.limits.visitor_max_sandboxes = 10;
            settings = operator
                .replace(settings.revision, &settings.settings)
                .await
                .unwrap();
            sandbox.id = SandboxId::generate();
            sandbox.origin = SandboxOrigin::Manual;
            sandbox.name = "catalogue-cutoff".into();
            let mut admission = SandboxAdmission::new(
                sandbox,
                SandboxAdmissionName::Exact("catalogue-cutoff".into()),
                8,
                layer2_core::now_rfc3339(),
            )
            .unwrap();
            admission.limit_class = class;
            let pause = Arc::new(Pause::default());
            let observed = pause.client().await;
            let candidate =
                crate::store::MongoSandboxStore::open(&observed, &fixture.seed.database)
                    .await
                    .unwrap();
            let release = pause.arm(Point::Snapshot);
            let admitting =
                tokio::spawn(async move { candidate.admit_catalogue_sandbox(admission).await });
            pause.wait().await;
            match class {
                SandboxLimitClass::Owner => {
                    settings.settings.limits.max_sandboxes_per_user = lowered
                }
                SandboxLimitClass::Visitor => {
                    settings.settings.limits.visitor_max_sandboxes = lowered
                }
            }
            operator
                .replace(settings.revision, &settings.settings)
                .await
                .unwrap();
            drop(release);
            let result = admitting.await.unwrap();
            if lowered == 0 {
                assert!(matches!(
                    result,
                    Err(SandboxStoreError::LimitReached {
                        limit: 0,
                        live: 1,
                        ..
                    })
                ));
            } else {
                assert!(matches!(result, Ok(SandboxAdmissionResult::Created(_))));
            }
            assert!(
                pause.attempts() >= 2,
                "catalogue admission kept its old snapshot"
            );
            assert_eq!(
                fixture
                    .resources
                    .sandboxes
                    .count_documents(doc! {})
                    .await
                    .unwrap(),
                if lowered == 0 { 1 } else { 2 }
            );
            fixture
                .seed
                .client
                .database(&fixture.seed.database)
                .drop()
                .await
                .unwrap();
        }
    }
}
