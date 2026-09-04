use layer2_core::ids::SubjectId;
use layer2_core::profiles::{
    MarketplaceLocator, MarketplaceStore, ProfileCapacityBounds, ProfileVisibility,
};
use layer2_core::trigger_domain::{TriggerId, TriggerStore};

use super::integration_tests::{seeded, trigger};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publication_uses_current_visibility_but_preview_uses_exact_revision() {
    let seed = seeded("visibility").await;
    let published_id = TriggerId("trg_before_private".into());
    seed.store
        .create_trigger(trigger(published_id.as_str(), &seed.owner, &seed.target))
        .await
        .unwrap();
    let public = seed
        .store
        .publish_trigger(&published_id, &seed.owner, 1, "2026-09-05T02:00:00Z".into())
        .await
        .unwrap();
    seed.profiles
        .set_profile_visibility(
            &seed.owner,
            &MarketplaceLocator::Sandbox(seed.target.sandbox_profile.id.clone()),
            ProfileVisibility::Private,
            "2026-09-05T03:00:00Z".into(),
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
    assert!(seed
        .store
        .preview_by_public_id(&public.public_id, "Publisher".into())
        .await
        .unwrap()
        .is_some());

    let private_id = TriggerId("trg_private_owner".into());
    seed.store
        .create_trigger(trigger(private_id.as_str(), &seed.owner, &seed.target))
        .await
        .unwrap();
    assert_eq!(
        seed.store
            .publish_trigger(&private_id, &seed.owner, 1, "2026-09-05T03:00:00Z".into())
            .await
            .unwrap_err()
            .code(),
        "trigger_reference_not_public"
    );

    let consumer = SubjectId("consumer".into());
    let unavailable_id = TriggerId("trg_private_consumer".into());
    seed.store
        .create_trigger(trigger(unavailable_id.as_str(), &consumer, &seed.target))
        .await
        .unwrap();
    assert_eq!(
        seed.store
            .publish_trigger(&unavailable_id, &consumer, 1, "2026-09-05T03:00:00Z".into(),)
            .await
            .unwrap_err()
            .code(),
        "trigger_reference_unavailable"
    );
    seed.client.database(&seed.database).drop().await.unwrap();
}
