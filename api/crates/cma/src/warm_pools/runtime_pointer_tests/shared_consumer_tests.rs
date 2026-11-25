use super::*;
use crate::{sandbox::worker::SandboxWorkerConfig, store::MongoSandboxStore};
use layer2_core::sandbox::{store::SandboxStore, SandboxPolicy, SandboxRuntimeUpdateService};
use layer3_adapter::ReleasedRuntime;

use super::released_source::{Call, ControlledSource};

fn release(revision: u64, agent: char, machine: char) -> ReleasedRuntime {
    ReleasedRuntime {
        lane_revisions: None,
        revision,
        agent: image(Lane::Agent, agent),
        machine: Some(image(Lane::Machine, machine)),
    }
}

fn observed_revision(source: &ControlledSource, expected: u64, surface: &str) {
    let revisions: Vec<_> = source
        .take_calls()
        .into_iter()
        .filter_map(|call| match call {
            Call::Snapshot(revision) => Some(revision),
            Call::RequireCurrent(_, _) => None,
        })
        .collect();
    assert!(
        !revisions.is_empty(),
        "{surface} did not observe the source"
    );
    assert!(
        revisions.iter().all(|revision| *revision == expected),
        "{surface}: {revisions:?}"
    );
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn live_consumers_observe_the_same_source_before_and_after_refresh() {
    let fixture = Fixture::new().await;
    let first = fixture.replica().await;
    let second = fixture.replica().await;
    let source = Arc::new(ControlledSource::new(release(1, 'a', '1')));
    let first_pools = first.pools.with_released_runtime(source.clone());
    let second_pools = second.pools.with_released_runtime(source.clone());
    let first_store = Arc::new(
        MongoSandboxStore::open(first.database.client(), first.database.name())
            .await
            .unwrap()
            .with_released_runtime(source.clone()),
    );
    let second_store = Arc::new(
        MongoSandboxStore::open(second.database.client(), second.database.name())
            .await
            .unwrap()
            .with_released_runtime(source.clone()),
    );
    let updating = [
        SandboxRuntimeUpdateService::with_released(first_store.clone(), source.clone()),
        SandboxRuntimeUpdateService::with_released(second_store.clone(), source.clone()),
    ];
    let worker = SandboxWorkerConfig::new(
        "worker",
        DEPLOYMENT,
        image(Lane::Agent, 'a'),
        SandboxPolicy::default(),
    )
    .with_released_runtime(source.clone());
    let caller = layer2_core::Caller::new(
        layer2_core::SubjectId::new("operator"),
        layer2_core::Authorization::new([layer2_core::Capability::ConfigurePlatform]),
    );
    for (revision, agent, machine) in [(1, 'a', '1'), (2, 'b', '2')] {
        if revision > 1 {
            source.refresh_to(release(revision, agent, machine));
        }
        source.take_calls();
        let expected = release(revision, agent, machine);
        for pools in [&first_pools, &second_pools] {
            let view = crate::warm_pools::WarmPoolAdministration::list(pools.as_ref(), &caller)
                .await
                .unwrap();
            assert_eq!(view.default_image, expected.agent);
            assert_eq!(view.machine_image, expected.machine);
            observed_revision(&source, revision, "warm pool list");
        }

        for store in [&first_store, &second_store] {
            assert_eq!(
                store.managed_runtime_version().as_deref(),
                Some(expected.agent.as_str())
            );
            observed_revision(&source, revision, "managed Sandbox release");
            assert_eq!(store.released_runtime(), expected);
            observed_revision(&source, revision, "managed Sandbox snapshot");
        }

        for service in &updating {
            assert_eq!(service.latest_version(), expected.agent);
            observed_revision(&source, revision, "runtime update service");
        }
        assert_eq!(worker.release().unwrap().revision, revision);
        observed_revision(&source, revision, "Sandbox worker configuration");
    }
    fixture.database.drop().await.unwrap();
}

struct DesktopHomes;

#[async_trait]
impl layer3_adapter::HomeVolumeStore for DesktopHomes {
    async fn inventory(&self) -> layer3_adapter::Result<Vec<layer3_adapter::HomeVolumeAttachment>> {
        panic!("creation capability must not inventory homes")
    }

    async fn prepare(
        &self,
        _: &layer3_adapter::HomeVolumeRequest,
    ) -> layer3_adapter::Result<layer3_adapter::HomeVolumeReceipt> {
        panic!("creation capability must not prepare a home")
    }

    async fn verify(&self, _: &layer3_adapter::HomeVolumeAttachment) -> layer3_adapter::Result<()> {
        panic!("creation capability for a new Sandbox must not verify a retained home")
    }

    async fn remove(
        &self,
        _: &layer3_adapter::HomeVolumeRequest,
        _: Option<&layer3_adapter::HomeVolumeReceipt>,
    ) -> layer3_adapter::Result<()> {
        panic!("creation capability must not remove a home")
    }
}

#[tokio::test]
async fn desktop_creation_uses_the_current_agent_release_for_the_default_pick() {
    use layer2_core::sandbox::desktop_creation::DesktopCreationService;

    let source = Arc::new(ControlledSource::new(release(1, 'a', '1')));
    let adapter = CodexAdapter::with_backend(
        resize::PoolBackend(Provider::default()),
        CodexSandboxSpec::default(),
    )
    .with_home_volumes(Arc::new(DesktopHomes));
    let verified = image(Lane::Agent, 'b');
    let desktop = DesktopCreationService::with_released(
        Arc::new(adapter),
        source.clone(),
        Some(verified.clone()),
    )
    .unwrap();
    let capacity = SandboxCapacity {
        memory_mib: 2048,
        ..capacity()
    };
    assert!(!desktop.capability(None, capacity, None).await.available());
    observed_revision(&source, 1, "desktop creation");
    source.refresh_to(release(2, 'b', '2'));
    let selected = desktop.capability(None, capacity, None).await;
    assert!(selected.available());
    assert_eq!(selected.runtime_image.as_deref(), Some(verified.as_str()));
    observed_revision(&source, 2, "desktop creation");
}
