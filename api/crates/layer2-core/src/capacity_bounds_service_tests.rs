//! Capacity-bound behavior across profile storage and session creation.

use std::num::NonZeroU32;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::environment::{Baseline, EnvironmentProfileSpec};
use crate::environment_service::EnvironmentProfileService;
use crate::environment_support::{fake_agent, profile_spec};
use crate::store::{PlatformSettingsStore, StoreError, StoredSettings};
use crate::test_support::{
    service_with_environments_and_settings, service_with_environments_and_settings_service, spec,
    user, FakeAdapter, FakeProfileStore, FakeStore,
};
use crate::{
    CoreError, OutOfBounds, PlatformSettings, PlatformSettingsService, SessionSize, SizeDimension,
};

struct ReadSettingsStore {
    settings: Option<PlatformSettings>,
    broken: bool,
    reads: AtomicUsize,
}

impl ReadSettingsStore {
    fn holding(settings: PlatformSettings) -> Self {
        Self {
            settings: Some(settings),
            broken: false,
            reads: AtomicUsize::new(0),
        }
    }

    fn broken() -> Self {
        Self {
            settings: None,
            broken: true,
            reads: AtomicUsize::new(0),
        }
    }

    fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl PlatformSettingsStore for ReadSettingsStore {
    async fn load(&self) -> Result<Option<StoredSettings>, StoreError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if self.broken {
            return Err(StoreError::Unavailable("settings cannot be read".into()));
        }
        Ok(self.settings.clone().map(|settings| StoredSettings {
            settings,
            revision: 1,
            previous: None,
        }))
    }

    async fn save_if_unchanged(
        &self,
        _expected: u64,
        _settings: &PlatformSettings,
    ) -> Result<Option<StoredSettings>, StoreError> {
        Err(StoreError::Unavailable(
            "this test store records reads only".into(),
        ))
    }
}

fn size(cpu_millis: u32, memory_mib: u32) -> SessionSize {
    SessionSize::new(cpu_millis, memory_mib).expect("test sizes are non-zero")
}

fn profile_with_size(name: &str, requested: Option<SessionSize>) -> EnvironmentProfileSpec {
    let mut profile = profile_spec(name);
    profile.agent.size = requested;
    profile
}

fn narrow_memory_bounds() -> PlatformSettings {
    let mut settings = PlatformSettings::default();
    settings.size_bounds.max_memory_mib =
        NonZeroU32::new(512).expect("the test maximum is non-zero");
    settings
}

fn profile_service(
    store: Arc<FakeProfileStore>,
    settings: Arc<PlatformSettingsService>,
) -> Arc<EnvironmentProfileService> {
    EnvironmentProfileService::new(
        store,
        Baseline::default(),
        None,
        None,
        fake_agent(),
        settings,
    )
}

async fn await_provisioned(adapter: &Arc<FakeAdapter>) -> layer3_adapter::RuntimeSpec {
    for _ in 0..400 {
        if let Some(spec) = adapter.last_spec.lock().await.clone() {
            return spec;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("nothing was ever provisioned");
}

#[tokio::test]
async fn an_out_of_bounds_size_is_refused_before_a_session_is_stored() {
    let profiles = crate::test_support::environments(Arc::new(FakeProfileStore::default()));
    let owner = user("subject-a");
    let profile = profiles
        .create(
            &owner,
            profile_with_size("too large", Some(size(4_001, 1_024))),
        )
        .await
        .expect("the profile remains storable");
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let sessions = service_with_environments_and_settings(
        Arc::clone(&store),
        Arc::clone(&adapter),
        profiles,
        PlatformSettings::default(),
    );

    let refusal = sessions
        .create_session(
            &owner,
            crate::session::SessionSpec {
                environment_profile: Some(profile.id),
                ..spec()
            },
        )
        .await
        .expect_err("the stated size is outside the range");

    assert_eq!(
        refusal,
        CoreError::SizeOutOfBounds(OutOfBounds {
            dimension: SizeDimension::CpuMillis,
            requested: 4_001,
            minimum: 250,
            maximum: 4_000,
        })
    );
    assert!(store.sessions.lock().await.is_empty());
    assert!(adapter.last_spec.lock().await.is_none());
}

#[tokio::test]
async fn an_in_bounds_size_reaches_the_runtime() {
    let profiles = crate::test_support::environments(Arc::new(FakeProfileStore::default()));
    let owner = user("subject-a");
    let profile = profiles
        .create(
            &owner,
            profile_with_size("ordinary", Some(size(1_500, 3_072))),
        )
        .await
        .expect("profile creates");
    let adapter = Arc::new(FakeAdapter::default());
    let sessions = service_with_environments_and_settings(
        Arc::new(FakeStore::default()),
        Arc::clone(&adapter),
        profiles,
        PlatformSettings::default(),
    );

    sessions
        .create_session(
            &owner,
            crate::session::SessionSpec {
                environment_profile: Some(profile.id),
                ..spec()
            },
        )
        .await
        .expect("the size is permitted");

    let capacity = await_provisioned(&adapter)
        .await
        .capacity
        .expect("the stated size reaches the runtime");
    assert_eq!(capacity.cpu_millis.get(), 1_500);
    assert_eq!(capacity.memory_mib.get(), 3_072);
}

#[tokio::test]
async fn no_profile_and_no_size_uses_the_deployments_own_reservation() {
    let adapter = Arc::new(FakeAdapter::default());
    let sessions = service_with_environments_and_settings(
        Arc::new(FakeStore::default()),
        Arc::clone(&adapter),
        crate::test_support::environments(Arc::new(FakeProfileStore::default())),
        PlatformSettings::default(),
    );

    sessions
        .create_session(&user("subject-a"), spec())
        .await
        .expect("an unnamed size is accepted");

    assert_eq!(await_provisioned(&adapter).await.capacity, None);
}

#[tokio::test]
async fn an_unnamed_size_is_not_measured_against_narrow_bounds() {
    let adapter = Arc::new(FakeAdapter::default());
    let sessions = service_with_environments_and_settings(
        Arc::new(FakeStore::default()),
        Arc::clone(&adapter),
        crate::test_support::environments(Arc::new(FakeProfileStore::default())),
        narrow_memory_bounds(),
    );

    sessions
        .create_session(&user("subject-a"), spec())
        .await
        .expect("nothing was asked for, so nothing is measured");

    assert_eq!(await_provisioned(&adapter).await.capacity, None);
}

#[tokio::test]
async fn an_unreadable_settings_store_falls_back_during_creation() {
    let settings_store = Arc::new(ReadSettingsStore::broken());
    let settings = Arc::new(PlatformSettingsService::new(
        Arc::clone(&settings_store) as Arc<dyn PlatformSettingsStore>
    ));
    let profiles = crate::test_support::environments(Arc::new(FakeProfileStore::default()));
    let owner = user("subject-a");
    let profile = profiles
        .create(
            &owner,
            profile_with_size("shipped size", Some(size(1_000, 1_024))),
        )
        .await
        .expect("profile creates");
    let adapter = Arc::new(FakeAdapter::default());
    let sessions = service_with_environments_and_settings_service(
        Arc::new(FakeStore::default()),
        Arc::clone(&adapter),
        profiles,
        settings,
    );

    sessions
        .create_session(
            &owner,
            crate::session::SessionSpec {
                environment_profile: Some(profile.id),
                ..spec()
            },
        )
        .await
        .expect("the shipped bounds are the fallback");

    let capacity = await_provisioned(&adapter)
        .await
        .capacity
        .expect("the explicit size reaches the runtime");
    assert_eq!(capacity.cpu_millis.get(), 1_000);
    assert_eq!(capacity.memory_mib.get(), 1_024);
    assert_eq!(settings_store.reads(), 1);
}

#[tokio::test]
async fn an_out_of_bounds_profile_is_stored_on_create_and_update() {
    let settings_store = Arc::new(ReadSettingsStore::holding(PlatformSettings::default()));
    let settings = Arc::new(PlatformSettingsService::new(
        Arc::clone(&settings_store) as Arc<dyn PlatformSettingsStore>
    ));
    let owner = user("subject-a");
    let service = profile_service(Arc::new(FakeProfileStore::default()), Arc::clone(&settings));

    let created = service
        .create(&owner, profile_with_size("large", Some(size(4_001, 1_024))))
        .await
        .expect("warning does not replace the write");
    assert_eq!(
        service.get(&owner, &created.id).await.unwrap().id,
        created.id
    );

    let updated = service
        .update(
            &owner,
            &created.id,
            profile_with_size("larger", Some(size(4_002, 1_024))),
        )
        .await
        .expect("an update is warned about but stored");
    assert_eq!(
        service.get(&owner, &created.id).await.unwrap().agent.size,
        updated.agent.size
    );
    assert_eq!(settings_store.reads(), 2);
}

#[tokio::test]
async fn a_profile_with_no_size_does_not_read_settings_for_a_warning() {
    let settings_store = Arc::new(ReadSettingsStore::holding(PlatformSettings::default()));
    let settings = Arc::new(PlatformSettingsService::new(
        Arc::clone(&settings_store) as Arc<dyn PlatformSettingsStore>
    ));
    let owner = user("subject-a");
    let service = profile_service(Arc::new(FakeProfileStore::default()), Arc::clone(&settings));

    let created = service
        .create(&owner, profile_with_size("ordinary", None))
        .await
        .expect("profile creates");
    service
        .update(
            &owner,
            &created.id,
            profile_with_size("ordinary still", None),
        )
        .await
        .expect("profile updates");

    assert_eq!(settings_store.reads(), 0);
}
