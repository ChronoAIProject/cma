use super::*;
use layer2_core::{
    ActivityQuery, ActivityRecord, ActivityService, ActivityStore, Page, PlatformSettings,
    PlatformSettingsService, PlatformSettingsStore, StoreError, StoredPlatformSettings,
};
use std::sync::{Arc, Mutex};

#[path = "reconciliation_explicit_update_policy_tests.rs"]
mod explicit_updates;

struct Settings(Mutex<Option<StoredPlatformSettings>>);

#[async_trait::async_trait]
impl PlatformSettingsStore for Settings {
    async fn read(&self) -> Result<StoredPlatformSettings, StoreError> {
        self.0
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| StoreError::Unavailable("fixture".into()))
    }

    async fn replace(
        &self,
        _: u64,
        _: &PlatformSettings,
    ) -> Result<StoredPlatformSettings, StoreError> {
        unreachable!("reconciliation only reads settings")
    }
}

struct Activity;

#[async_trait::async_trait]
impl ActivityStore for Activity {
    async fn append(&self, _: &ActivityRecord) -> Result<(), StoreError> {
        unreachable!("reconciliation only reads settings")
    }

    async fn list(&self, _: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
        unreachable!("reconciliation does not read activity content")
    }
}

fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_788_566_400)
}

fn enable(settings: &Settings, at: SystemTime, threshold: u64) {
    let mut stored = settings.0.lock().unwrap();
    let inactivity = &mut stored.as_mut().unwrap().settings.inactivity;
    inactivity.auto_pause_enabled = true;
    inactivity.grace_started_at = Some(rfc3339_at(at));
    inactivity.pause_after_seconds = threshold;
}

async fn live() -> (Arc<fixtures::Store>, SandboxReconciler, Arc<Settings>) {
    let (store, mut reconciler) = setup(3, Duration::ZERO).await;
    let settings = Arc::new(Settings(Mutex::new(Some(StoredPlatformSettings {
        changes: Default::default(),
        revision: 1,
        settings: PlatformSettings::default(),
    }))));
    reconciler.policy = Arc::new(crate::sandbox::policy::LiveSandboxPolicy::new(
        Arc::new(PlatformSettingsService::new(
            settings.clone(),
            ActivityService::new(Arc::new(Activity)),
        )),
        vec![],
    ));
    (store, reconciler, settings)
}

#[tokio::test(start_paused = true)]
async fn disabled_and_grace_sweeps_finish_without_scanning_pause_candidates() {
    let (store, reconciler, settings) = live().await;
    let at = now();
    assert!(reconciler.run_due(at).await.unwrap());
    assert!(!store.keys("authority").is_empty());
    assert!(store.keys("candidates").is_empty());
    assert!(store.keys("pause").is_empty());
    enable(&settings, at, 900);
    assert!(reconciler
        .run_due(at + Duration::from_secs(899))
        .await
        .unwrap());
    assert!(store.keys("candidates").is_empty());
    assert!(reconciler.progress.lock().await.is_none());

    let eligible_at = at + Duration::from_secs(900);
    assert!(reconciler.run_due(eligible_at).await.unwrap());
    let pauses = store.pauses.lock().unwrap();
    assert_eq!(pauses.len(), 3);
    assert!(pauses.iter().all(|request| request.auto_pause_enabled
        && request.pause_after_seconds == 900
        && request.requested_at == rfc3339_at(eligible_at)));
}

#[tokio::test(start_paused = true)]
async fn resumed_idle_phase_honors_disabling_and_restarted_grace() {
    for disable in [true, false] {
        let (store, reconciler, settings) = live().await;
        let at = now();
        enable(&settings, at - Duration::from_secs(900), 900);
        store.fail("pause", "sbx_001", Failure::Deadline);
        assert!(reconciler.run_due(at).await.is_err());
        assert_eq!(store.keys("pause"), ["sbx_000", "sbx_001"]);
        if disable {
            settings
                .0
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .settings
                .inactivity
                .auto_pause_enabled = false;
        } else {
            enable(&settings, at, 1_800);
        }
        assert!(reconciler
            .run_due(at + Duration::from_secs(15))
            .await
            .unwrap());
        assert_eq!(store.keys("pause"), ["sbx_000", "sbx_001"]);
        assert_eq!(store.pauses.lock().unwrap().len(), 1);
        assert!(reconciler.progress.lock().await.is_none());
    }
}

#[tokio::test(start_paused = true)]
async fn resumed_idle_phase_reads_new_threshold_and_retries_unavailable_policy() {
    let (store, reconciler, settings) = live().await;
    let at = now();
    enable(&settings, at - Duration::from_secs(900), 900);
    store.fail("pause", "sbx_001", Failure::Deadline);
    assert!(reconciler.run_due(at).await.is_err());
    let retained = settings.0.lock().unwrap().take().unwrap();
    assert!(reconciler.run_due(at).await.is_err());
    assert_eq!(store.keys("pause"), ["sbx_000", "sbx_001"]);
    assert_eq!(
        reconciler
            .progress
            .lock()
            .await
            .as_ref()
            .unwrap()
            .sweep
            .phase,
        Phase::IdlePause
    );

    *settings.0.lock().unwrap() = Some(retained);
    enable(&settings, at, 1_800);
    let resumed_at = at + Duration::from_secs(1_800);
    assert!(reconciler.run_due(resumed_at).await.unwrap());
    // Each retained pause attempt refreshes bindings first, including the one
    // whose policy is unavailable. Neither refresh repeats a pause admission.
    assert_eq!(store.keys("authority"), ["start", "sbx_001"].repeat(3));
    assert_eq!(
        store.keys("pause"),
        ["sbx_000", "sbx_001", "sbx_001", "sbx_002"]
    );
    let pauses = store.pauses.lock().unwrap();
    assert_eq!(pauses.len(), 3);
    assert_eq!(pauses[0].pause_after_seconds, 900);
    assert!(pauses[1..].iter().all(|request| request.auto_pause_enabled
        && request.pause_after_seconds == 1_800
        && request.requested_at == rfc3339_at(resumed_at)));
}

struct Platform(Option<bool>);

#[async_trait::async_trait]
impl crate::sandbox::reconciliation::platform::PlatformSpendSweep for Platform {
    async fn evaluate_platform_spend(
        &self,
        _: u64,
    ) -> Result<Option<bool>, layer2_core::budget::BudgetError> {
        Ok(self.0)
    }
}

#[tokio::test(start_paused = true)]
async fn the_platform_spend_phase_drives_a_pause_of_quiet_sandboxes_and_leaves_busy_ones() {
    use layer2_core::sandbox::store::InactivityPauseCause;
    // Automatic inactivity pause stays disabled throughout.
    let (store, reconciler, _) = live().await;
    let reconciler = reconciler.with_platform_spend(Arc::new(Platform(Some(false))));
    assert!(reconciler.run_due(now()).await.unwrap());
    assert!(store.keys("pause").is_empty());
    let (store, reconciler, _) = live().await;
    let reconciler = reconciler.with_platform_spend(Arc::new(Platform(Some(true))));
    assert!(reconciler.run_due(now()).await.unwrap());
    // Eligibility (quiet Agents, no active turn or queued input) stays the
    // store's decision; this fixture records every request it is offered.
    let pauses = store.pauses.lock().unwrap();
    assert_eq!(pauses.len(), 3);
    assert!(pauses.iter().all(
        |request| request.cause == InactivityPauseCause::PlatformSpendStop
            && request.pause_after_seconds == 0
            && request.auto_pause_enabled
            && request.requested_at == rfc3339_at(now())
    ));
}

#[derive(Default)]
struct Counting(std::sync::atomic::AtomicUsize);

#[async_trait::async_trait]
impl crate::sandbox::reconciliation::platform::PlatformSpendSweep for Counting {
    async fn evaluate_platform_spend(
        &self,
        _: u64,
    ) -> Result<Option<bool>, layer2_core::budget::BudgetError> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Some(false))
    }
}

#[tokio::test(start_paused = true)]
async fn platform_spend_is_evaluated_even_when_an_earlier_sweep_phase_keeps_failing() {
    let (store, reconciler, _) = live().await;
    let platform = Arc::new(Counting::default());
    let reconciler = reconciler.with_platform_spend(platform.clone());
    let evaluations = || platform.0.load(std::sync::atomic::Ordering::SeqCst);
    let at = now();
    for (offset, expected) in [(0, 1), (5, 1), (15, 2), (31, 3)] {
        store.fail("authority", "start", Failure::Corrupt);
        assert!(reconciler
            .run_due(at + Duration::from_secs(offset))
            .await
            .is_err());
        assert_eq!(evaluations(), expected, "after {offset}s");
    }
}

/// An Active, bound copy of a fixture candidate, valid for the pause store.
fn active(index: usize, always_on: bool) -> layer2_core::sandbox::Sandbox {
    use layer2_core::sandbox::{
        RuntimeBinding, SandboxCredentialAuthority, SandboxStatus, SandboxWorkState,
        SandboxWorkStep,
    };
    let mut value = fixtures::sandbox(index);
    let at = value.created_at.clone();
    value.status = SandboxStatus::Active;
    value.profile.always_on = always_on.then_some(true);
    value.runtime = Some(RuntimeBinding {
        boot_id: None,
        runtime_id: format!("runtime-{index}"),
        runtime_version: "image-v1".into(),
        provision_generation: Some(1),
        bound_at: at.clone(),
    });
    value.lifecycle.runtime_generation_high_water = 1;
    value.credential.key_generation = 1;
    value.credential.delivered = true;
    value.credential.current = Some(SandboxCredentialAuthority {
        generation: 1,
        provider_key_id: "provider-key".into(),
        provider_state_version: 1,
        issued_at: at.clone(),
        expires_at: "2026-10-05T00:00:00Z".into(),
        grant_fingerprint: "a".repeat(64),
    });
    value.work = SandboxWorkState::new(SandboxWorkStep::WaitForCredentialRotation, at);
    value
}

#[tokio::test(start_paused = true)]
async fn the_platform_spend_phase_pauses_keep_alive_and_agentless_sandboxes() {
    use layer2_core::sandbox::store::{InMemorySandboxStore, SandboxStore};
    use layer2_core::sandbox::{SandboxStatus, PLATFORM_SPEND_PAUSE_NOTICE};
    for stopped in [false, true] {
        let (store, reconciler, settings) = live().await;
        let decisions = InMemorySandboxStore::default();
        // sbx_000 keeps itself alive; none of the three has a live Agent.
        for index in 0..3 {
            decisions
                .create_sandbox(active(index, index == 0))
                .await
                .unwrap();
        }
        assert!(store.pause_decisions.set(decisions.clone()).is_ok());
        let reconciler = reconciler.with_platform_spend(Arc::new(Platform(Some(stopped))));
        // Without the stop, automatic inactivity pause is enabled and due.
        enable(&settings, now() - Duration::from_secs(1_800), 900);
        assert!(reconciler.run_due(now()).await.unwrap());
        for index in 0..3 {
            let sandbox = decisions
                .get_sandbox(&fixtures::sandbox(index).id)
                .await
                .unwrap()
                .unwrap();
            if stopped {
                assert_eq!(sandbox.status, SandboxStatus::Pausing, "sbx_{index:03}");
                assert_eq!(
                    sandbox.lifecycle.paused_notice.as_deref(),
                    Some(PLATFORM_SPEND_PAUSE_NOTICE)
                );
            } else {
                assert_eq!(sandbox.status, SandboxStatus::Active, "sbx_{index:03}");
            }
            assert_eq!(sandbox.profile.always_on, (index == 0).then_some(true));
        }
        assert_eq!(store.pauses.lock().unwrap().len(), 3);
    }
}

#[tokio::test(start_paused = true)]
async fn a_platform_stop_defers_managed_runtime_updates_so_no_paused_sandbox_is_restarted() {
    for stopped in [false, true] {
        let (store, reconciler, settings) = live().await;
        let reconciler = reconciler
            .with_runtime_version("fixture-image".into())
            .with_platform_spend(Arc::new(Platform(Some(stopped))));
        enable(&settings, now() - Duration::from_secs(1_800), 900);
        assert!(reconciler.run_due(now()).await.unwrap());
        // One binding read per pass; the runtime-update phase reads every
        // candidate once more, and would start a rotation (a runtime, even
        // on a paused Sandbox) unless the stop defers it to after the lift.
        for index in 0..3 {
            let id = fixtures::sandbox(index).id.to_string();
            let reads = store.keys("get").iter().filter(|key| **key == id).count();
            assert_eq!(reads, if stopped { 1 } else { 2 }, "{id} stopped={stopped}");
        }
        assert_eq!(store.keys("pause").len(), 3);
    }
}
