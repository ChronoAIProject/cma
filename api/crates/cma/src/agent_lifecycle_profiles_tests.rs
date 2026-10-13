use super::*;
use layer2_core::profiles::{InMemoryProfileStore, SandboxProfileStore};
use layer3_adapter::SandboxCapacity;

fn bounds() -> ProfileCapacityBounds {
    ProfileCapacityBounds {
        min_cpu_millis: 100,
        max_cpu_millis: 8000,
        min_memory_mib: 128,
        max_memory_mib: 16384,
    }
}

#[tokio::test]
async fn lifecycle_uses_captured_defaults_and_replays_profiles_after_configuration_changes() {
    let store = Arc::new(InMemoryProfileStore::default());
    let profiles =
        PrivateProfileCreationService::new(store.clone(), bounds(), Arc::new([] as [String; 0]));
    let caller = Caller::new(SubjectId::new("owner"), Authorization::new([]));
    let request = serde_json::from_value(serde_json::json!({"name":"Test Agent"})).unwrap();
    let input = LifecycleInput {
        request,
        resolved_image: None,
        capacity: Some(SandboxCapacity::default()),
        capabilities: vec![],
        actor: None,
    };
    let (_, _, first) = create_profiles(
        &profiles,
        &caller,
        &input.request,
        None,
        "original",
        input.captured_capacity(bounds()),
        input.captured_limits(),
    )
    .await
    .unwrap();
    let materialized = store
        .sandbox_profile_revision(&first.id, first.revision)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            materialized.content.cpu_millis,
            materialized.content.memory_mib
        ),
        (1000, 1024)
    );
    assert_eq!(materialized.content.max_workspaces, 16);
    assert_eq!(materialized.content.max_agents_per_workspace, 16);
    let resumed: LifecycleInput =
        serde_json::from_slice(&serde_json::to_vec(&input).unwrap()).unwrap();
    let changed_bounds = ProfileCapacityBounds {
        min_cpu_millis: 2000,
        min_memory_mib: 4096,
        ..bounds()
    };
    assert_eq!(
        resumed.captured_capacity(changed_bounds),
        SandboxCapacity::default()
    );
    let (_, _, replayed) = create_profiles(
        &profiles,
        &caller,
        &resumed.request,
        None,
        "original",
        resumed.captured_capacity(changed_bounds),
        resumed.captured_limits(),
    )
    .await
    .unwrap();
    assert_eq!(replayed, first);
}

#[tokio::test]
async fn legacy_lifecycle_without_capacity_replays_the_old_minimum_reservation() {
    let store = Arc::new(InMemoryProfileStore::default());
    let profiles =
        PrivateProfileCreationService::new(store.clone(), bounds(), Arc::new([] as [String; 0]));
    let caller = Caller::new(SubjectId::new("owner"), Authorization::new([]));
    let input: LifecycleInput = serde_json::from_value(serde_json::json!({
        "request":{"name":"Legacy Agent"},"resolved_image":null
    }))
    .unwrap();
    assert!(input.capacity.is_none());
    let (_, _, original) = create_profiles(
        &profiles,
        &caller,
        &input.request,
        None,
        "legacy",
        SandboxCapacity {
            cpu_millis: 100,
            memory_mib: 128,
        },
        input.captured_limits(),
    )
    .await
    .unwrap();
    let (_, _, replayed) = create_profiles(
        &profiles,
        &caller,
        &input.request,
        None,
        "legacy",
        input.captured_capacity(bounds()),
        input.captured_limits(),
    )
    .await
    .unwrap();
    assert_eq!(original, replayed);
    let retained = store
        .sandbox_profile_revision(&original.id, original.revision)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.content.max_workspaces, 1);
    assert_eq!(retained.content.max_agents_per_workspace, 1);
    assert_eq!(
        (retained.content.cpu_millis, retained.content.memory_mib),
        (100, 128)
    );
}
