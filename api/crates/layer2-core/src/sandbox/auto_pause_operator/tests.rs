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
#[derive(Clone, Copy)]
enum ReadFault {
    Timeout,
    Unavailable,
}

struct Settings(Mutex<StoredPlatformSettings>, Mutex<Option<ReadFault>>);
#[async_trait::async_trait]
impl PlatformSettingsStore for Settings {
    async fn read(&self) -> Result<StoredPlatformSettings, StoreError> {
        let fault = *self.1.lock().unwrap();
        match fault {
            Some(ReadFault::Timeout) => tokio::time::sleep(Duration::from_secs(30)).await,
            Some(ReadFault::Unavailable) => {
                return Err(StoreError::Unavailable("policy offline".into()))
            }
            None => {}
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
    discovery_fault: Mutex<Option<ReadFault>>,
    show_refusals: Mutex<bool>,
    invalid_discovery_page: Mutex<bool>,
    supplied_page: Mutex<Option<Page<crate::SandboxId>>>,
}
#[async_trait::async_trait]
impl SandboxMaintenanceStore for Maintenance {
    async fn sandbox_maintenance_candidates(
        &self,
        page: &PageRequest,
    ) -> Result<Page<crate::SandboxId>, SandboxStoreError> {
        let fault = *self.discovery_fault.lock().unwrap();
        match fault {
            Some(ReadFault::Timeout) => tokio::time::sleep(Duration::from_secs(30)).await,
            Some(ReadFault::Unavailable) => {
                return Err(SandboxStoreError::Unavailable("discovery offline".into()));
            }
            None => {}
        }
        if let Some(page) = self.supplied_page.lock().unwrap().clone() {
            return Ok(page);
        }
        if *self.invalid_discovery_page.lock().unwrap() {
            return Ok(Page {
                items: vec![crate::SandboxId("a".into())],
                next: Some("b".into()),
            });
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
        if self.fail.lock().unwrap().as_deref() == Some(request.sandbox_id.as_str()) {
            return Err(SandboxStoreError::Unavailable(
                "unknown idle evidence".into(),
            ));
        }
        Ok(SandboxMaintenanceOutcome::Scheduled)
    }
    async fn preview_inactivity_pause_with_reason(
        &self,
        request: SandboxInactivityPauseRequest,
    ) -> Result<InactivityPausePreview, SandboxStoreError> {
        let refusal = match (
            *self.show_refusals.lock().unwrap(),
            request.sandbox_id.as_str(),
        ) {
            (true, "a") => Some(InactivityPauseRefusal::LegacyLogExpectedRows),
            (true, "b") => Some(InactivityPauseRefusal::RecentAgentActivity),
            _ => None,
        };
        let outcome = if refusal.is_some() {
            SandboxMaintenanceOutcome::NotEligible
        } else {
            self.preview_inactivity_pause(request).await?
        };
        Ok(InactivityPausePreview { outcome, refusal })
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
            changes: Default::default(),
            revision: 5,
            settings: policy,
        }),
        Mutex::default(),
    ));
    let maintenance = Arc::new(Maintenance {
        audit: audit.clone(),
        calls: Mutex::default(),
        fail: Mutex::default(),
        slow: Mutex::new(false),
        discovery_fault: Mutex::default(),
        show_refusals: Mutex::default(),
        invalid_discovery_page: Mutex::new(false),
        supplied_page: Mutex::default(),
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
            *store.discovery_fault.lock().unwrap() = Some(ReadFault::Timeout);
        } else {
            *settings.1.lock().unwrap() = Some(ReadFault::Timeout);
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

#[tokio::test(start_paused = true)]
async fn preview_reports_bounded_reasons_for_503_before_effects() {
    use metrics::{
        atomics::AtomicU64, Counter, Gauge, Histogram, Key, KeyName, Metadata, Recorder,
        SharedString, Unit,
    };
    use std::sync::atomic::Ordering;

    #[derive(Default)]
    struct Counters(Mutex<Vec<(Key, Arc<AtomicU64>)>>);

    impl Recorder for Counters {
        fn describe_counter(&self, _: KeyName, _: Option<Unit>, _: SharedString) {}
        fn describe_gauge(&self, _: KeyName, _: Option<Unit>, _: SharedString) {}
        fn describe_histogram(&self, _: KeyName, _: Option<Unit>, _: SharedString) {}
        fn register_counter(&self, key: &Key, _: &Metadata<'_>) -> Counter {
            let value = Arc::new(AtomicU64::new(0));
            self.0.lock().unwrap().push((key.clone(), value.clone()));
            Counter::from_arc(value)
        }
        fn register_gauge(&self, _: &Key, _: &Metadata<'_>) -> Gauge {
            Gauge::noop()
        }
        fn register_histogram(&self, _: &Key, _: &Metadata<'_>) -> Histogram {
            Histogram::noop()
        }
    }

    let counters = Counters::default();
    let _recorder = metrics::set_default_local_recorder(&counters);
    let reasons = [
        (
            Some(ReadFault::Unavailable),
            None,
            false,
            "policy_read_error",
        ),
        (Some(ReadFault::Timeout), None, false, "policy_read_timeout"),
        (None, Some(ReadFault::Unavailable), false, "discovery_error"),
        (None, Some(ReadFault::Timeout), false, "discovery_timeout"),
        (None, None, true, "invalid_discovery_page"),
    ];
    for (policy, discovery, invalid_page, _) in reasons {
        let (service, store, settings, audit, caller) = fixture();
        *settings.1.lock().unwrap() = policy;
        *store.discovery_fault.lock().unwrap() = discovery;
        *store.invalid_discovery_page.lock().unwrap() = invalid_page;
        let unauthorized = Caller {
            subject: caller.subject.clone(),
            authorization: Authorization::new([]),
        };
        let page = PageRequest::new(1, None).unwrap();
        assert_eq!(
            service.batch(&unauthorized, page.clone(), false).await,
            Err(OperatorError::Forbidden)
        );
        assert_eq!(
            service.batch(&caller, page, false).await,
            Err(OperatorError::Unavailable(
                "auto-pause evidence or audit unavailable".into()
            ))
        );
        assert!(store.calls.lock().unwrap().is_empty());
        assert!(audit.records.lock().unwrap().is_empty());
    }
    let counters = counters.0.lock().unwrap();
    assert_eq!(counters.len(), reasons.len());
    for ((key, value), (_, _, _, reason)) in counters.iter().zip(reasons) {
        assert_eq!(key.name(), "cma_auto_pause_operator_unavailable_total");
        assert_eq!(
            key.labels()
                .map(|label| (label.key(), label.value()))
                .collect::<Vec<_>>(),
            [("mode", "preview"), ("reason", reason)]
        );
        assert_eq!(value.load(Ordering::Relaxed), 1);
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

#[tokio::test]
async fn partial_preview_retries_unknown_evidence_without_audit_or_pause_writes() {
    let (service, store, _, audit, caller) = fixture();
    *store.fail.lock().unwrap() = Some("b".into());
    let partial = service
        .batch(&caller, PageRequest::new(3, None).unwrap(), false)
        .await
        .unwrap();
    assert_eq!(
        (partial.candidates, partial.failed, partial.next.as_deref()),
        (1, 1, Some("a"))
    );
    *store.fail.lock().unwrap() = None;
    let remaining = service
        .batch(&caller, PageRequest::new(3, partial.next).unwrap(), false)
        .await
        .unwrap();
    assert_eq!(
        (remaining.candidates, remaining.failed, remaining.next),
        (2, 0, None)
    );
    assert_eq!((partial.scheduled, remaining.scheduled), (0, 0));
    assert!(store.calls.lock().unwrap().is_empty());
    assert!(audit.records.lock().unwrap().is_empty());
}

#[tokio::test]
async fn later_outcome_audit_failure_keeps_only_confirmed_counts_and_previous_cursor() {
    let (service, store, _, audit, caller) = fixture();
    *audit.fail_after.lock().unwrap() = Some(3);
    let partial = service
        .batch(&caller, PageRequest::new(3, None).unwrap(), true)
        .await
        .unwrap();
    assert_eq!(
        (partial.scheduled, partial.failed, partial.next.as_deref()),
        (1, 1, Some("a"))
    );
    let calls = store.calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .map(|request| request.sandbox_id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    let records = audit.records.lock().unwrap();
    assert_eq!(records.len(), 3);
    assert_eq!(records[2].detail.get(DetailKey::Status), Some("requested"));
    assert_eq!(records[2].at, calls[1].requested_at);
}

#[tokio::test]
async fn preview_reports_each_refusal_with_the_sandbox_it_evaluated() {
    let (service, store, _, audit, caller) = fixture();
    *store.show_refusals.lock().unwrap() = true;
    let batch = service
        .batch(&caller, PageRequest::new(2, None).unwrap(), false)
        .await
        .unwrap();
    assert_eq!(batch.candidates, 0);
    assert_eq!(batch.failed, 0);
    assert_eq!(batch.next.as_deref(), Some("b"));
    assert_eq!(
        batch.refusals,
        vec![
            AutoPauseSandboxRefusal {
                sandbox_id: SandboxId("a".into()),
                reason: InactivityPauseRefusal::LegacyLogExpectedRows,
            },
            AutoPauseSandboxRefusal {
                sandbox_id: SandboxId("b".into()),
                reason: InactivityPauseRefusal::RecentAgentActivity,
            },
        ]
    );
    assert!(store.calls.lock().unwrap().is_empty());
    assert!(audit.records.lock().unwrap().is_empty());
}

#[tokio::test]
async fn malformed_page_cannot_schedule_or_audit_partial_work() {
    let (service, store, _, audit, caller) = fixture();
    for limit in [0, AUTO_PAUSE_MAX_BATCH + 1] {
        assert!(matches!(
            service
                .batch(
                    &caller,
                    PageRequest {
                        limit,
                        cursor: None
                    },
                    true
                )
                .await,
            Err(OperatorError::InvalidRequest(_))
        ));
    }

    for (limit, cursor, ids) in [
        (1, None, ["b", "c"]),
        (2, None, ["b", "b"]),
        (2, Some("b".into()), ["b", "c"]),
        (2, None, ["c", "b"]),
    ] {
        *store.supplied_page.lock().unwrap() = Some(Page {
            items: ids
                .into_iter()
                .map(|id| crate::SandboxId(id.into()))
                .collect(),
            next: None,
        });
        assert!(service
            .batch(&caller, PageRequest { limit, cursor }, true)
            .await
            .is_err());
        assert!(store.calls.lock().unwrap().is_empty());
        assert!(audit.records.lock().unwrap().is_empty());
    }
}
