//! #1548 component qualification. No runtime, money valuation or allocation proof.
use super::*;

async fn setup() -> (
    Arc<InMemorySandboxStore>,
    Arc<InMemoryProfileStore>,
    SandboxProfileRef,
) {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let reference = publish(
        &profiles,
        profile(
            "sbp_cost_limits",
            "owner",
            "cost limits",
            ProfileVisibility::Private,
            false,
        ),
    )
    .await;
    (
        Arc::new(InMemorySandboxStore::default()),
        profiles,
        reference,
    )
}

// These two requirements are known absent at e4047c926. Keep them executable
// explicitly: ignoring them in the ordinary unit suite does not qualify #1548.
#[tokio::test]
#[ignore = "M-SUST-05 qualification: default owner quota is currently ten, not two"]
async fn default_policy_refuses_the_third_sandbox() {
    let (store, profiles, reference) = setup().await;
    let service = SandboxService::new(
        store,
        profiles,
        SandboxPolicy::default(),
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
    );
    let owner = caller("owner", [Capability::CreateSandbox]);
    for _ in 0..2 {
        service
            .create(&owner, request(reference.clone()))
            .await
            .unwrap();
    }
    match service.create(&owner, request(reference)).await {
        Ok(_) => panic!("third Sandbox admitted under the unchanged default policy"),
        Err(error) => assert_eq!(
            error,
            SandboxServiceError::SandboxLimitReached { limit: 2, live: 2 }
        ),
    }
}

#[test]
#[ignore = "M-SUST-05 qualification: default model budgets are currently disabled"]
fn default_budget_protects_an_unlisted_owner() {
    let settings = crate::platform::PlatformSettings::default();
    let policy = settings
        .model_budget
        .for_owner(&SubjectId::new("fresh-owner"))
        .expect("a public default must cover a fresh owner before the first turn");
    policy.validate().unwrap();
    assert_eq!(policy.day_timezone, "UTC");
    // USD 5 additionally needs an approved private accounting-unit valuation.
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_two_limit_serializes_competing_service_instances() {
    let (store, profiles, reference) = setup().await;
    let make_service = || {
        SandboxService::new(
            store.clone(),
            profiles.clone(),
            SandboxPolicy {
                max_sandboxes_per_owner: 2,
                ..SandboxPolicy::default()
            },
            Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        )
    };
    let first = make_service();
    let peer = make_service();
    let owner = caller("owner", [Capability::CreateSandbox]);
    first
        .create(&owner, request(reference.clone()))
        .await
        .unwrap();
    let barrier = Arc::new(tokio::sync::Barrier::new(13));
    let mut tasks = Vec::new();
    for index in 0..12 {
        let service = if index % 2 == 0 { &first } else { &peer }.clone();
        let owner = owner.clone();
        let create = request(reference.clone());
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            service.create(&owner, create).await
        }));
    }
    barrier.wait().await;
    let mut accepted = 0;
    let mut refused = 0;
    for task in tasks {
        match task.await.unwrap() {
            Ok(_) => accepted += 1,
            Err(SandboxServiceError::SandboxLimitReached { limit: 2, live: 2 }) => refused += 1,
            other => panic!("unexpected result: {other:?}"),
        }
    }
    assert_eq!((accepted, refused), (1, 11));
    assert_eq!(sandbox_use_count(&profiles, &reference.id).await, 2);
    assert_eq!(
        store
            .list_sandboxes(&SubjectId::new("owner"), &PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        2
    );
    // This checks one in-memory authority shared by two services. It does not
    // claim two AMs, Mongo transaction isolation or absence of Kubernetes writes.
}
