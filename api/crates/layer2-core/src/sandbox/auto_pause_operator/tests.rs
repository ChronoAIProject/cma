use super::*;
use crate::{
    ActivityQuery, ActivityStore, Authorization, PlatformSettingsStore, StoreError,
    StoredPlatformSettings, SubjectId,
};
use std::sync::Mutex;

#[derive(Default)]
struct Audit {
    records: Mutex<Vec<ActivityRecord>>,
    fail: Mutex<bool>,
    fail_after: Mutex<Option<usize>>,
}
#[async_trait::async_trait]
impl ActivityStore for Audit {
    async fn append(&self, record: &ActivityRecord) -> Result<(), StoreError> {
        if *self.fail.lock().unwrap()
            || self
                .fail_after
                .lock()
                .unwrap()
                .is_some_and(|count| self.records.lock().unwrap().len() >= count)
        {
            return Err(StoreError::Unavailable("audit offline".into()));
        }
        self.records.lock().unwrap().push(record.clone());
        Ok(())
    }
    async fn list(&self, _: &ActivityQuery) -> Result<crate::Page<ActivityRecord>, StoreError> {
        Ok(crate::Page::new(self.records.lock().unwrap().clone(), None))
    }
}
struct Settings(Mutex<StoredPlatformSettings>, Mutex<bool>);
#[async_trait::async_trait]
impl PlatformSettingsStore for Settings {
    async fn read(&self) -> Result<StoredPlatformSettings, StoreError> {
        let slow = *self.1.lock().unwrap();
        if slow {
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
        Ok(self.0.lock().unwrap().clone())
    }
    async fn replace(
        &self,
        _: u64,
        _: &crate::PlatformSettings,
    ) -> Result<StoredPlatformSettings, StoreError> {
        unreachable!()
    }
}
struct Maintenance {
    audit: Arc<Audit>,
    calls: Mutex<Vec<SandboxInactivityPauseRequest>>,
    fail: Mutex<Option<String>>,
    slow: Mutex<bool>,
    slow_discovery: Mutex<bool>,
}
#[async_trait::async_trait]
impl SandboxMaintenanceStore for Maintenance {
    async fn sandbox_maintenance_candidates(
        &self,
        page: &PageRequest,
    ) -> Result<Page<crate::SandboxId>, SandboxStoreError> {
        let slow = *self.slow_discovery.lock().unwrap();
        if slow {
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
        let mut ids: Vec<_> = ["a", "b", "c"]
            .into_iter()
            .filter(|id| page.cursor.as_deref().is_none_or(|cursor| *id > cursor))
            .take(usize::from(page.limit) + 1)
            .map(|id| crate::SandboxId(id.into()))
            .collect();
        let more = ids.len() > usize::from(page.limit);
        if more {
            ids.pop();
        }
        Ok(Page {
            next: more.then(|| ids.last().unwrap().to_string()),
            items: ids,
        })
    }
    async fn schedule_inactivity_pause(
        &self,
        request: SandboxInactivityPauseRequest,
    ) -> Result<SandboxMaintenanceOutcome, SandboxStoreError> {
        assert!(
            !self.audit.records.lock().unwrap().is_empty(),
            "audit must precede effects"
        );
        assert_eq!(request.cause, InactivityPauseCause::OperatorInactivity);
        self.calls.lock().unwrap().push(request.clone());
        let slow = *self.slow.lock().unwrap();
        if slow {
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
        if self.fail.lock().unwrap().as_deref() == Some(request.sandbox_id.as_str()) {
            return Err(SandboxStoreError::Unavailable("uncertain pause".into()));
        }
        Ok(SandboxMaintenanceOutcome::Scheduled)
    }
    async fn preview_inactivity_pause(
        &self,
        request: SandboxInactivityPauseRequest,
    ) -> Result<SandboxMaintenanceOutcome, SandboxStoreError> {
        assert_eq!(request.cause, InactivityPauseCause::Inactivity);
        Ok(SandboxMaintenanceOutcome::Scheduled)
    }
    async fn set_sandbox_always_on(
        &self,
        _: crate::SandboxId,
        _: u64,
        _: bool,
    ) -> Result<super::super::Sandbox, SandboxStoreError> {
        unreachable!()
    }
    async fn schedule_image_rotation(
        &self,
        _: SandboxImageRotationRequest,
    ) -> Result<SandboxMaintenanceOutcome, SandboxStoreError> {
        unreachable!()
    }
}
fn fixture() -> (
    SandboxAutoPauseService,
    Arc<Maintenance>,
    Arc<Settings>,
    Arc<Audit>,
    Caller,
) {
    let audit = Arc::new(Audit::default());
    let activity = ActivityService::new(audit.clone());
    let mut policy = crate::PlatformSettings::default();
    policy.inactivity.auto_pause_enabled = true;
    policy.inactivity.grace_started_at = Some(crate::now_rfc3339());
    let settings = Arc::new(Settings(
        Mutex::new(StoredPlatformSettings {
            revision: 5,
            settings: policy,
        }),
        Mutex::new(false),
    ));
    let maintenance = Arc::new(Maintenance {
        audit: audit.clone(),
        calls: Mutex::default(),
        fail: Mutex::default(),
        slow: Mutex::new(false),
        slow_discovery: Mutex::new(false),
    });
    let service = SandboxAutoPauseService::new(
        Arc::new(PlatformSettingsService::new(
            settings.clone(),
            activity.clone(),
        )),
        maintenance.clone(),
        activity,
    );
    let caller = Caller {
        subject: SubjectId::new("operator"),
        authorization: Authorization::new([Capability::ConfigurePlatform]),
    };
    (service, maintenance, settings, audit, caller)
}

#[tokio::test]
async fn bounded_pages_preserve_audit_revision_and_preview_has_no_writes() {
    let (service, store, _, audit, caller) = fixture();
    let page = PageRequest::new(2, None).unwrap();
    let preview = service.batch(&caller, page.clone(), false).await.unwrap();
    assert_eq!(
        (preview.candidates, preview.next.as_deref()),
        (2, Some("b"))
    );
    assert!(store.calls.lock().unwrap().is_empty());
    assert!(audit.records.lock().unwrap().is_empty());
    let first = service.batch(&caller, page, true).await.unwrap();
    assert_eq!((first.scheduled, first.policy_revision), (2, 5));
    let last = service
        .batch(&caller, PageRequest::new(2, first.next).unwrap(), true)
        .await
        .unwrap();
    assert_eq!(last.scheduled, 1);
    assert_eq!(last.next, None);
    assert!(store
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|request| request.expected_policy_revision == Some(5)));
    let records = audit.records.lock().unwrap();
    assert_eq!(records.len(), 6);
    assert_eq!(records[0].detail.get(DetailKey::Status), Some("requested"));
    assert_eq!(
        records[1].detail.get(DetailKey::CorrelationId),
        Some(records[0].id.0.as_str())
    );
}

#[tokio::test]
async fn partial_failure_retains_counts_and_retries_from_before_uncertain_candidate() {
    let (service, store, _, audit, caller) = fixture();
    *store.fail.lock().unwrap() = Some("b".into());
    let result = service
        .batch(&caller, PageRequest::new(3, None).unwrap(), true)
        .await
        .unwrap();
    assert_eq!(
        (result.scheduled, result.failed, result.next.as_deref()),
        (1, 1, Some("a"))
    );
    assert_eq!(store.calls.lock().unwrap().len(), 2);
    assert_eq!(
        audit
            .records
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .detail
            .get(DetailKey::Status),
        Some("uncertain")
    );
}

#[tokio::test]
async fn disabled_policy_authority_and_failed_audit_never_dispatch() {
    let (service, store, settings, audit, caller) = fixture();
    let unauthorized = Caller {
        subject: caller.subject.clone(),
        authorization: Authorization::new([]),
    };
    assert_eq!(
        service
            .batch(&unauthorized, PageRequest::default(), true)
            .await,
        Err(OperatorError::Forbidden)
    );
    settings
        .0
        .lock()
        .unwrap()
        .settings
        .inactivity
        .auto_pause_enabled = false;
    assert!(
        !service
            .batch(&caller, PageRequest::new(1, None).unwrap(), true)
            .await
            .unwrap()
            .enabled
    );
    settings
        .0
        .lock()
        .unwrap()
        .settings
        .inactivity
        .auto_pause_enabled = true;
    *audit.fail.lock().unwrap() = true;
    let result = service
        .batch(&caller, PageRequest::new(1, None).unwrap(), true)
        .await
        .unwrap();
    assert_eq!(result.failed, 1);
    assert_eq!(result.next.as_deref(), Some(""));
    assert!(store.calls.lock().unwrap().is_empty());
    assert!(service
        .batch(&caller, PageRequest::default(), true)
        .await
        .is_err());
}

#[tokio::test(start_paused = true)]
async fn batch_deadline_keeps_durable_intent_and_a_recoverable_cursor() {
    let (service, store, _, audit, caller) = fixture();
    *store.slow.lock().unwrap() = true;
    let result = service
        .batch(&caller, PageRequest::new(3, None).unwrap(), true)
        .await
        .unwrap();
    assert_eq!((result.failed, result.next.as_deref()), (1, Some("")));
    assert_eq!(store.calls.lock().unwrap().len(), 1);
    assert_eq!(
        audit.records.lock().unwrap()[0]
            .detail
            .get(DetailKey::Status),
        Some("requested")
    );
}

#[tokio::test(start_paused = true)]
async fn batch_deadline_includes_settings_and_discovery_before_effects() {
    for discovery in [false, true] {
        let (service, store, settings, audit, caller) = fixture();
        if discovery {
            *store.slow_discovery.lock().unwrap() = true;
        } else {
            *settings.1.lock().unwrap() = true;
        }
        let started = tokio::time::Instant::now();
        assert!(matches!(
            service
                .batch(&caller, PageRequest::new(1, None).unwrap(), true)
                .await,
            Err(OperatorError::Unavailable(_))
        ));
        assert_eq!(started.elapsed(), Duration::from_secs(20));
        assert!(store.calls.lock().unwrap().is_empty());
        assert!(audit.records.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn failed_outcome_audit_preserves_intent_and_never_claims_confirmed_scheduling() {
    let (service, store, _, audit, caller) = fixture();
    *audit.fail_after.lock().unwrap() = Some(1);
    let result = service
        .batch(&caller, PageRequest::new(2, None).unwrap(), true)
        .await
        .unwrap();
    assert_eq!(
        (result.scheduled, result.failed, result.next.as_deref()),
        (0, 1, Some(""))
    );
    assert_eq!(store.calls.lock().unwrap().len(), 1);
    assert_eq!(audit.records.lock().unwrap().len(), 1);
}
