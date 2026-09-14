use super::*;

fn empty_profile() -> SandboxProfile {
    let mut profile = sandbox_profile(
        "sbp_availability",
        "owner",
        WorkspaceProfileRef::new(WorkspaceProfileId("wkp_unused".into()), 1),
    );
    profile.draft.workspace_profiles.clear();
    profile
}

#[test]
fn availability_is_backward_compatible_and_preserves_pinned_content() {
    let profile = empty_profile();
    let mut legacy = serde_json::to_value(&profile).unwrap();
    legacy.as_object_mut().unwrap().remove("availability");
    let decoded: SandboxProfile = serde_json::from_value(legacy).unwrap();
    assert_eq!(decoded, profile);
    let mut changed = decoded;
    changed.availability = SandboxProfileAvailability {
        warmup_enabled: true,
        warmup_count: 5,
        always_keep_alive: true,
    };
    assert_eq!(
        content_digest(&changed.draft).unwrap(),
        content_digest(&profile.draft).unwrap()
    );
    for count in [0, 6, 255] {
        changed.availability.warmup_count = count;
        assert_eq!(
            changed.validate(bounds()).unwrap_err().code,
            "invalid_warmup_count"
        );
    }
    changed.availability.warmup_count = 5;
    changed.visibility = ProfileVisibility::Private;
    assert_eq!(
        changed.validate(bounds()).unwrap_err().code,
        "warmup_requires_public_profile"
    );
    changed.availability.warmup_enabled = false;
    changed.validate(bounds()).unwrap();
}

#[tokio::test]
async fn warmup_requires_publication_private_copies_reset_policy_and_unsharing_disables_it() {
    let store = InMemoryProfileStore::default();
    let mut profile = empty_profile();
    store.create_sandbox_profile(profile.clone()).await.unwrap();
    profile.draft_version += 1;
    profile.availability = SandboxProfileAvailability {
        warmup_enabled: true,
        warmup_count: 5,
        always_keep_alive: true,
    };
    assert!(store.save_sandbox_draft(1, profile.clone()).await.is_err());
    let scanner: [String; 0] = [];
    let pinned = store
        .publish_sandbox_profile(&profile.id, 1, NOW.into(), bounds(), &scanner)
        .await
        .unwrap();
    store.save_sandbox_draft(1, profile.clone()).await.unwrap();
    assert_eq!(
        store
            .sandbox_profile_revision(&profile.id, 1)
            .await
            .unwrap(),
        Some(pinned.clone())
    );
    let locator = MarketplaceLocator::Sandbox(profile.id.clone());
    let ForkedProfile::Sandbox(mut fork) = store
        .fork_profile(&SubjectId("visitor".into()), &locator, 1, NOW.into())
        .await
        .unwrap()
    else {
        panic!("expected Sandbox copy");
    };
    assert_ne!(fork.id, profile.id);
    assert_eq!(fork.owner, SubjectId("visitor".into()));
    assert_eq!(fork.visibility, ProfileVisibility::Private);
    assert_eq!(fork.availability, SandboxProfileAvailability::default());
    assert_eq!(fork.draft, pinned.content);
    fork.draft_version += 1;
    fork.draft.cpu_millis += 100;
    store.save_sandbox_draft(1, fork.clone()).await.unwrap();
    assert_eq!(
        store.sandbox_profile(&profile.id).await.unwrap(),
        Some(profile.clone())
    );
    store
        .set_profile_visibility(
            &profile.owner,
            &locator,
            ProfileVisibility::Private,
            NOW.into(),
            bounds(),
            &scanner,
        )
        .await
        .unwrap();
    let unshared = store.sandbox_profile(&profile.id).await.unwrap().unwrap();
    assert!(!unshared.availability.warmup_enabled);
    assert!(unshared.availability.always_keep_alive);
    assert_eq!(unshared.availability.warmup_count, 5);
    unshared.validate(bounds()).unwrap();
}
