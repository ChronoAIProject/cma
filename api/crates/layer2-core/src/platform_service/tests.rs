#[path = "budget_rollout_tests.rs"]
mod budget_rollout;

use std::sync::Mutex;

use super::*;
use crate::{ActivityQuery, ActivityStore, Authorization, Page, SubjectId};

#[derive(Default)]
struct MemoryActivity {
    records: Mutex<Vec<ActivityRecord>>,
}

#[async_trait::async_trait]
impl ActivityStore for MemoryActivity {
    async fn append(&self, record: &ActivityRecord) -> Result<(), StoreError> {
        self.records.lock().unwrap().push(record.clone());
        Ok(())
    }

    async fn list(&self, _query: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
        Ok(Page::new(self.records.lock().unwrap().clone(), None))
    }
}

struct MemoryStore {
    stored: Mutex<StoredPlatformSettings>,
    reads: Mutex<usize>,
    conflicts_remaining: Mutex<usize>,
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self {
            stored: Mutex::new(StoredPlatformSettings {
                changes: Default::default(),
                revision: 0,
                settings: PlatformSettings::default(),
            }),
            reads: Mutex::new(0),
            conflicts_remaining: Mutex::new(0),
        }
    }
}

#[async_trait::async_trait]
impl PlatformSettingsStore for MemoryStore {
    async fn read(&self) -> Result<StoredPlatformSettings, StoreError> {
        *self.reads.lock().unwrap() += 1;
        Ok(self.stored.lock().unwrap().clone())
    }

    async fn replace(
        &self,
        expected_revision: u64,
        settings: &PlatformSettings,
    ) -> Result<StoredPlatformSettings, StoreError> {
        self.replace_with_changes(expected_revision, settings, &Default::default())
            .await
    }

    async fn replace_with_changes(
        &self,
        expected_revision: u64,
        settings: &PlatformSettings,
        changes: &PlatformSettingChanges,
    ) -> Result<StoredPlatformSettings, StoreError> {
        let mut conflicts = self.conflicts_remaining.lock().unwrap();
        if *conflicts > 0 {
            *conflicts -= 1;
            let mut stored = self.stored.lock().unwrap();
            stored.revision += 1;
            return Err(StoreError::RevisionConflict {
                expected: expected_revision,
                actual: stored.revision,
            });
        }
        drop(conflicts);

        let mut stored = self.stored.lock().unwrap();
        if stored.revision != expected_revision {
            return Err(StoreError::RevisionConflict {
                expected: expected_revision,
                actual: stored.revision,
            });
        }
        *stored = StoredPlatformSettings {
            changes: changes.clone(),
            revision: expected_revision + 1,
            settings: settings.clone(),
        };
        Ok(stored.clone())
    }
}

fn caller(capabilities: impl IntoIterator<Item = Capability>) -> Caller {
    Caller::new(
        SubjectId::new("operator-a"),
        Authorization::new(capabilities),
    )
}

fn service() -> (
    PlatformSettingsService,
    Arc<MemoryStore>,
    Arc<MemoryActivity>,
) {
    let store = Arc::new(MemoryStore::default());
    let activity = Arc::new(MemoryActivity::default());
    let activity_service = ActivityService::new(activity.clone());
    (
        PlatformSettingsService::new(store.clone(), activity_service),
        store,
        activity,
    )
}

#[tokio::test]
async fn operator_entry_points_require_only_platform_configuration() {
    let unrelated = caller([
        Capability::ReadAllSandboxes,
        Capability::ControlAnySandbox,
        Capability::ReadActivityLog,
    ]);
    let (service, store, activity) = service();

    assert_eq!(service.read(&unrelated).await, Err(CoreError::Forbidden));
    assert_eq!(
        service
            .configure_platform(&unrelated, 0, &PlatformSettings::default())
            .await,
        Err(CoreError::Forbidden)
    );
    assert_eq!(
        service.pause_triggers(&unrelated).await,
        Err(CoreError::Forbidden)
    );
    assert_eq!(
        service.resume_triggers(&unrelated).await,
        Err(CoreError::Forbidden)
    );
    assert_eq!(*store.reads.lock().unwrap(), 0);
    assert!(activity.records.lock().unwrap().is_empty());
}

#[tokio::test]
async fn configuration_validates_and_preserves_revision_cas() {
    let operator = caller([Capability::ConfigurePlatform]);
    let (service, _store, activity) = service();
    let mut changed = PlatformSettings::default();
    changed.marketplace.enabled = false;
    let replaced = service
        .configure_platform(&operator, 0, &changed)
        .await
        .unwrap();
    assert_eq!(replaced.revision, 1);
    assert!(!replaced.settings.marketplace.enabled);

    assert!(matches!(
        service
            .configure_platform(&operator, 0, &PlatformSettings::default())
            .await,
        Err(CoreError::InvalidState(_))
    ));
    let records = activity.records.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].actor.as_ref(), Some(&operator.subject));
    assert_eq!(
        records[0].target,
        Some(ActivityTarget::Resource(SETTINGS_TARGET.into()))
    );
    assert_eq!(records[0].detail.get(DetailKey::Revision), Some("1"));
    assert_eq!(
        records[0].detail.get(DetailKey::Setting),
        Some(SETTINGS_TARGET)
    );
    assert!(!serde_json::to_string(&records[0])
        .unwrap()
        .contains("marketplace"));
}

#[tokio::test]
async fn auto_pause_enablement_records_and_resets_grace_period() {
    let operator = caller([Capability::ConfigurePlatform]);
    let (service, _store, _activity) = service();
    let mut enabled = PlatformSettings::default();
    enabled.inactivity.auto_pause_enabled = true;

    let first = service
        .configure_platform(&operator, 0, &enabled)
        .await
        .unwrap();
    let first_grace = first.settings.inactivity.grace_started_at.clone();
    assert!(first_grace
        .as_deref()
        .is_some_and(|value| crate::seconds_from_rfc3339(value).is_some()));

    let unchanged = service
        .configure_platform(&operator, first.revision, &enabled)
        .await
        .unwrap();
    assert_eq!(unchanged.settings.inactivity.grace_started_at, first_grace);

    enabled.inactivity.pause_after_seconds += 60;
    let changed = service
        .configure_platform(&operator, unchanged.revision, &enabled)
        .await
        .unwrap();
    assert!(changed.settings.inactivity.grace_started_at.is_some());

    enabled.inactivity.auto_pause_enabled = false;
    let disabled = service
        .configure_platform(&operator, changed.revision, &enabled)
        .await
        .unwrap();
    assert!(!disabled.settings.inactivity.auto_pause_enabled);
    assert!(disabled.settings.inactivity.grace_started_at.is_none());
}

#[tokio::test]
async fn invalid_configuration_never_reaches_the_store_or_activity_log() {
    let operator = caller([Capability::ConfigurePlatform]);
    let (service, store, activity) = service();
    let mut invalid = PlatformSettings::default();
    invalid.inactivity.pause_after_seconds = 1;

    assert!(matches!(
        service.configure_platform(&operator, 0, &invalid).await,
        Err(CoreError::InvalidRequest(_))
    ));
    assert_eq!(store.stored.lock().unwrap().revision, 0);
    assert!(activity.records.lock().unwrap().is_empty());
}

#[tokio::test]
async fn internal_reads_are_distinct_from_audited_operator_reads() {
    let operator = caller([Capability::ConfigurePlatform]);
    let (service, _store, activity) = service();

    assert_eq!(service.current().await.unwrap().revision, 0);
    assert!(activity.records.lock().unwrap().is_empty());
    assert_eq!(service.read(&operator).await.unwrap().revision, 0);

    let records = activity.records.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].action, ActivityAction::Read);
    assert_eq!(records[0].actor.as_ref(), Some(&operator.subject));
    assert_eq!(
        records[0].target,
        Some(ActivityTarget::Resource(SETTINGS_TARGET.into()))
    );
}

#[tokio::test]
async fn trigger_controls_merge_over_conflicts_and_are_idempotent() {
    let operator = caller([Capability::ConfigurePlatform]);
    let (service, store, activity) = service();
    *store.conflicts_remaining.lock().unwrap() = 2;

    let paused = service.pause_triggers(&operator).await.unwrap();
    assert!(paused.settings.triggers.orchestration_paused);
    assert_eq!(paused.revision, 3);

    let unchanged = service.pause_triggers(&operator).await.unwrap();
    assert_eq!(unchanged.revision, paused.revision);
    let resumed = service.resume_triggers(&operator).await.unwrap();
    assert!(!resumed.settings.triggers.orchestration_paused);
    assert_eq!(resumed.revision, 4);

    let records = activity.records.lock().unwrap();
    assert_eq!(records.len(), 3);
    assert!(records.iter().all(|record| {
        record.actor.as_ref() == Some(&operator.subject)
            && record.target == Some(ActivityTarget::Resource(TRIGGERS_TARGET.into()))
            && record.detail.get(DetailKey::Revision).is_some()
    }));
    assert_eq!(records[0].detail.get(DetailKey::Status), Some("paused"));
    assert_eq!(records[2].detail.get(DetailKey::Status), Some("running"));
}

#[tokio::test]
async fn only_the_winning_effective_change_updates_attribution() {
    let alice = caller([Capability::ConfigurePlatform]);
    let bob = Caller::new(
        SubjectId::new("operator-b"),
        Authorization::new([Capability::ConfigurePlatform]),
    );
    let (service, _, _) = service();
    let mut settings = service.current().await.unwrap().settings;
    settings.retention.agent_log_days += 1;
    let first = service
        .configure_platform(&alice, 0, &settings)
        .await
        .unwrap();
    let retention = first.changes["agent_log_retention"].clone();
    assert_eq!(retention.actor, alice.subject);
    assert_eq!(retention.revision, 1);
    assert!(crate::seconds_from_rfc3339(&retention.at).is_some());

    settings.limits.visitor_max_sandboxes += 1;
    assert!(matches!(
        service.configure_platform(&bob, 0, &settings).await,
        Err(CoreError::InvalidState(_))
    ));
    assert_eq!(service.current().await.unwrap().changes, first.changes);
    let second = service
        .configure_platform(&bob, 1, &settings)
        .await
        .unwrap();
    assert_eq!(second.changes.len(), 2);
    assert_eq!(second.changes["agent_log_retention"], retention);
    assert_eq!(second.changes["visitor_sandboxes"].actor, bob.subject);
    assert_eq!(second.changes["visitor_sandboxes"].revision, 2);
    let third = service
        .configure_platform(&alice, 2, &settings)
        .await
        .unwrap();
    assert_eq!(third.changes, second.changes);

    let paused = service.pause_triggers(&bob).await.unwrap();
    assert_eq!(paused.changes["trigger_orchestration"].actor, bob.subject);
    assert_eq!(paused.changes["trigger_orchestration"].revision, 4);
    let unchanged = service.pause_triggers(&alice).await.unwrap();
    assert_eq!(unchanged.changes, paused.changes);
}
