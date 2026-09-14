use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use layer2_core::{
    ActivityQuery, ActivityRecord, ActivityService, ActivityStore, Page, PlatformSettings,
    PlatformSettingsStore, StoreError, StoredPlatformSettings,
};

use super::*;

struct CountedSettings {
    result: Mutex<Result<StoredPlatformSettings, StoreError>>,
    reads: AtomicUsize,
}

impl CountedSettings {
    fn available(enabled: bool, orchestration_paused: bool) -> Arc<Self> {
        let mut settings = PlatformSettings::default();
        settings.triggers.enabled = enabled;
        settings.triggers.orchestration_paused = orchestration_paused;
        Arc::new(Self {
            result: Mutex::new(Ok(StoredPlatformSettings {
                revision: 1,
                settings,
            })),
            reads: AtomicUsize::new(0),
        })
    }

    fn set_triggers(&self, enabled: bool, orchestration_paused: bool) {
        let mut result = self.result.lock().unwrap();
        let stored = result.as_mut().expect("settings remain available");
        stored.revision += 1;
        stored.settings.triggers.enabled = enabled;
        stored.settings.triggers.orchestration_paused = orchestration_paused;
    }

    fn fail_with(&self, error: StoreError) {
        *self.result.lock().unwrap() = Err(error);
    }

    fn read_count(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl PlatformSettingsStore for CountedSettings {
    async fn read(&self) -> Result<StoredPlatformSettings, StoreError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.result.lock().unwrap().clone()
    }

    async fn replace(
        &self,
        _expected_revision: u64,
        _settings: &PlatformSettings,
    ) -> Result<StoredPlatformSettings, StoreError> {
        unreachable!("launch policy reads never replace platform settings")
    }
}

struct NoopActivity;

#[async_trait::async_trait]
impl ActivityStore for NoopActivity {
    async fn append(&self, _record: &ActivityRecord) -> Result<(), StoreError> {
        Ok(())
    }

    async fn list(&self, _query: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
        Ok(Page::new(Vec::new(), None))
    }
}

fn live_policy(store: Arc<CountedSettings>) -> LiveLaunchPolicy {
    let settings = Arc::new(PlatformSettingsService::new(
        store,
        ActivityService::new(Arc::new(NoopActivity)),
    ));
    LiveLaunchPolicy::new(settings)
}

#[tokio::test]
async fn policy_projects_both_trigger_controls_exactly() {
    let store = CountedSettings::available(true, false);
    let policy = live_policy(store);

    assert_eq!(
        policy.current_launch_policy().await,
        Ok(LaunchPolicy {
            enabled: true,
            orchestration_paused: false,
        })
    );
}

#[tokio::test]
async fn policy_reads_settings_again_and_observes_later_changes() {
    let store = CountedSettings::available(true, false);
    let policy = live_policy(Arc::clone(&store));
    assert_eq!(
        policy.current_launch_policy().await.unwrap(),
        LaunchPolicy {
            enabled: true,
            orchestration_paused: false,
        }
    );

    store.set_triggers(false, true);

    assert_eq!(
        policy.current_launch_policy().await.unwrap(),
        LaunchPolicy {
            enabled: false,
            orchestration_paused: true,
        }
    );
    assert_eq!(store.read_count(), 2);
}

#[tokio::test]
async fn settings_failures_return_one_sanitized_unavailable_error() {
    const BACKEND_DETAIL: &str = "settings database node rejected the read";

    let store = CountedSettings::available(true, false);
    store.fail_with(StoreError::Unavailable(BACKEND_DETAIL.into()));
    let policy = live_policy(Arc::clone(&store));

    let error = policy.current_launch_policy().await.unwrap_err();
    assert_eq!(
        error,
        LaunchServiceError::Unavailable(SETTINGS_UNAVAILABLE.into())
    );
    assert!(!error.to_string().contains(BACKEND_DETAIL));
    assert_eq!(store.read_count(), 1);
}
