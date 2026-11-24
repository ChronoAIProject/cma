use super::*;
use crate::agent_lifecycle::runtime_canary::Canaries;
use crate::agent_models::test_runtime::Probe;
use crate::config::desktop_canary_config;
use layer1_public::rest::runtime_canary::{
    CanaryGrantRequest, CanaryPolicyRequest, CanaryRedeemRequest, CanaryRefusal,
    RuntimeCanaryService,
};
use mongodb::bson::{doc, DateTime, Document};

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn desktop_canary_checks_capacity_and_browser_before_consuming_the_grant() {
    for (cpu, memory, with_browser) in [(900, 2048, true), (1000, 1024, true), (1000, 2048, false)]
    {
        let (client, database, mut harness) = isolated_environment().await;
        desktop::configure(&mut harness).await;
        let config = desktop_canary_config(cpu, memory, true);
        let owner = SubjectId::new("owner");
        let mut context = v2_tests::context(&owner);
        context.caller.authorization =
            Authorization::new([Capability::UseOwnSandboxes, Capability::CreateSandbox]);
        let support = |browser: bool| desktop::CreationSupport {
            connections: Arc::new(
                layer2_core::sandbox::FixedSandboxCredentialPrerequisite::SetupRequired,
            ),
            adapter: Some(if browser {
                harness.adapter.clone()
            } else {
                Arc::new(Probe::new(
                    serde_json::from_value(json!({
                        "sandbox_id": "desktop-fixture",
                        "generation": 1,
                        "home_identity": "a".repeat(43),
                        "boot_id": "a".repeat(32),
                        "installation_ready": true,
                        "credential_ready": true,
                        "restore_pending": false,
                        "agent_processes": 0,
                    }))
                    .unwrap(),
                ))
            }),
            policy: None,
            machine_image: None,
        };
        let (_, lifecycle) = fixture_with_support(
            harness.store.clone(),
            harness.registry.clone(),
            &database,
            &client,
            owner.as_str(),
            true,
            support(with_browser),
        )
        .await;
        let service = Canaries::new(&config, lifecycle.clone()).unwrap();
        let policy = service
            .configure(CanaryPolicyRequest {
                expected_revision: 0,
                open: true,
            })
            .await
            .unwrap();
        let grant = service
            .grant(CanaryGrantRequest {
                owner: owner.clone(),
                release_key: "debian-candidate".into(),
                expected_revision: 0,
                policy_revision: policy.revision,
                consent_sha256: "c".repeat(64),
                expires_at: DateTime::now()
                    .saturating_add_duration(Duration::from_secs(120))
                    .try_to_rfc3339_string()
                    .unwrap(),
            })
            .await
            .unwrap();
        let slots = database.collection::<Document>("sandbox_admissions");
        let owner_filter = doc! { "_id": owner.as_str() };
        let before = slots.find_one(owner_filter.clone()).await.unwrap().unwrap();
        let operation_rows = database.collection::<Document>("agent_lifecycle_operations");
        let request = || CanaryRedeemRequest {
            grant_id: grant.grant_id.clone(),
            release_key: "debian-candidate".into(),
            policy_revision: policy.revision,
            name: "Desktop canary fixture".into(),
            initial_message: "Open the desktop".into(),
        };
        assert!(matches!(
            service.redeem(&context, "desktop-canary", request()).await,
            Err(CanaryRefusal::Closed)
        ));
        assert_eq!(operation_rows.count_documents(doc! {}).await.unwrap(), 0);
        assert_eq!(
            slots.find_one(owner_filter.clone()).await.unwrap(),
            Some(before.clone())
        );

        if !with_browser {
            let (_, capable) = fixture_with_support(
                harness.store.clone(),
                harness.registry.clone(),
                &database,
                &client,
                owner.as_str(),
                true,
                support(true),
            )
            .await;
            let capable_service = Canaries::new(&config, capable.clone()).unwrap();
            let accepted = capable_service
                .redeem(&context, "desktop-canary", request())
                .await
                .unwrap();
            let consumed = slots.find_one(owner_filter.clone()).await.unwrap().unwrap();
            assert_ne!(consumed, before);
            let stored = operation_rows
                .find_one(doc! { "_id": &accepted.id })
                .await
                .unwrap()
                .unwrap();
            assert!(stored
                .get_document("input")
                .unwrap()
                .get_document("request")
                .unwrap()
                .get_bool("desktop")
                .unwrap());
            let replay = service
                .redeem(&context, "desktop-canary", request())
                .await
                .unwrap();
            assert_eq!(replay.id, accepted.id);
            assert_eq!(slots.find_one(owner_filter).await.unwrap(), Some(consumed));
            assert_eq!(operation_rows.count_documents(doc! {}).await.unwrap(), 1);
            capable
                .drain_handle()
                .drain(Duration::from_secs(5))
                .await
                .unwrap();
        }
        lifecycle
            .drain_handle()
            .drain(Duration::from_secs(5))
            .await
            .unwrap();
        database.drop().await.unwrap();
    }
}
