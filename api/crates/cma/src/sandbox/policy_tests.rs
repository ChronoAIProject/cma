use std::sync::Mutex;

use layer2_core::{
    ActivityQuery, ActivityRecord, ActivityService, ActivityStore, Page, PlatformSettings,
    PlatformSettingsStore, StoredPlatformSettings,
};

use super::*;

struct MemorySettings(Mutex<StoredPlatformSettings>);

#[async_trait::async_trait]
impl PlatformSettingsStore for MemorySettings {
    async fn read(&self) -> Result<StoredPlatformSettings, StoreError> {
        Ok(self.0.lock().unwrap().clone())
    }

    async fn replace(
        &self,
        _expected_revision: u64,
        _settings: &PlatformSettings,
    ) -> Result<StoredPlatformSettings, StoreError> {
        unreachable!("policy reads never replace platform settings")
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

fn live_policy() -> (Arc<MemorySettings>, LiveSandboxPolicy) {
    let store = Arc::new(MemorySettings(Mutex::new(StoredPlatformSettings {
        revision: 1,
        settings: PlatformSettings::default(),
    })));
    let settings = Arc::new(PlatformSettingsService::new(
        store.clone(),
        ActivityService::new(Arc::new(NoopActivity)),
    ));
    let policy = LiveSandboxPolicy::new(
        settings,
        vec![ServiceId::new("chrono-llm"), ServiceId::new("ornn-api")],
    );
    (store, policy)
}

#[tokio::test]
async fn policy_reads_live_limits_retention_and_configured_services() {
    let (store, policy) = live_policy();
    let initial = policy.current_sandbox_policy().await.unwrap();
    assert_eq!(initial.max_sandboxes_per_owner, 10);
    assert_eq!(initial.max_sandboxes_per_visitor, 1);
    assert_eq!(initial.inactivity_pause_after_seconds, 900);
    assert_eq!(initial.retention_grace, Duration::from_secs(7 * 86_400));
    assert_eq!(
        initial.baseline_services,
        [ServiceId::new("chrono-llm"), ServiceId::new("ornn-api")]
    );

    {
        let mut stored = store.0.lock().unwrap();
        stored.settings.limits.max_sandboxes_per_user = 9;
        stored.settings.limits.visitor_max_sandboxes = 4;
        stored.settings.inactivity.pause_after_seconds = 1_800;
        stored.settings.retention.checkpoint_grace_days = 12;
    }
    let updated = policy.current_sandbox_policy().await.unwrap();
    assert_eq!(updated.max_sandboxes_per_owner, 9);
    assert_eq!(updated.max_sandboxes_per_visitor, 4);
    assert_eq!(updated.inactivity_pause_after_seconds, 1_800);
    assert_eq!(updated.retention_grace, Duration::from_secs(12 * 86_400));
}

#[tokio::test]
async fn policy_refuses_limits_that_do_not_fit_the_domain_model() {
    let (store, policy) = live_policy();
    store
        .0
        .lock()
        .unwrap()
        .settings
        .limits
        .max_sandboxes_per_user = u32::from(u16::MAX) + 1;
    let error = policy.current_sandbox_policy().await.unwrap_err();
    assert!(matches!(error, StoreError::Malformed(detail)
        if detail.contains("limits.max_sandboxes_per_user")));

    let (store, policy) = live_policy();
    store
        .0
        .lock()
        .unwrap()
        .settings
        .limits
        .visitor_max_sandboxes = u32::from(u16::MAX) + 1;
    let error = policy.current_sandbox_policy().await.unwrap_err();
    assert!(matches!(error, StoreError::Malformed(detail)
        if detail.contains("limits.visitor_max_sandboxes")));
}
