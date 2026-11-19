use super::*;
use crate::sandbox::SandboxHome;
use layer3_adapter::{HomeVolumePolicy, HomeVolumeReceipt};

fn retained_home(value: &mut crate::sandbox::Sandbox) {
    let mut home = SandboxHome::reserve(
        &value.id,
        HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "test-home".into(),
            capacity_gib: 10,
            template_name: "test-seed".into(),
            template_generation: "test-generation".into(),
        },
    )
    .unwrap();
    home.bind(HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "test-home-volume".into(),
        storage_generation: "test-home-generation".into(),
    })
    .unwrap();
    value.lifecycle.home = Some(Box::new(home));
}

#[test]
fn terminal_failure_closes_compute_at_failure_and_keeps_retained_disk() {
    let mut value = sandbox();
    retained_home(&mut value);
    let runtime = value.runtime.clone();
    let (active, _) = observe(None, &value, 1, &at(0), false).unwrap().unwrap();
    value.status = SandboxStatus::Failed;
    value.revision += 1;
    let (failed, event) = observe(Some(&active), &value, 2, &at(60), false)
        .unwrap()
        .unwrap();
    assert_eq!(event.kind, UsageEventKind::Terminated);
    assert_eq!(event.quantity.uptime_seconds, 60);
    assert_eq!(event.quantity.cpu_millis_seconds, 60_000);
    assert_eq!(event.quantity.memory_mib_seconds, 122_880);
    assert_eq!(
        failed.allocation,
        Allocation {
            disk_gib: 10,
            ..Default::default()
        }
    );
    assert_eq!(value.runtime, runtime);
    let (checkpoint, interval) = observe(Some(&failed), &value, 2, &at(120), true)
        .unwrap()
        .unwrap();
    assert_eq!(
        interval.quantity,
        UsageTotals {
            disk_gib_seconds: 600,
            ..Default::default()
        }
    );
    assert_eq!(
        checkpoint.totals_at(timestamp(&at(180)).unwrap()).unwrap(),
        UsageTotals {
            disk_gib_seconds: 1_800,
            ..failed.totals.clone()
        }
    );
    assert!(observe(Some(&checkpoint), &value, 2, &at(120), true)
        .unwrap()
        .is_none());
    value.lifecycle.home.as_mut().unwrap().released = true;
    value.revision += 1;
    let (released, _) = observe(Some(&checkpoint), &value, 3, &at(180), false)
        .unwrap()
        .unwrap();
    assert_eq!(released.allocation, Allocation::default());
    assert_eq!(
        released.totals_at(timestamp(&at(240)).unwrap()).unwrap(),
        released.totals
    );
}

#[test]
fn existing_terminal_allocation_converges_without_new_source_revision_or_historical_rewrite() {
    let mut value = sandbox();
    retained_home(&mut value);
    let (active, _) = observe(None, &value, 1, &at(0), false).unwrap().unwrap();
    let (mut legacy, _) = observe(Some(&active), &value, 1, &at(60), true)
        .unwrap()
        .unwrap();
    value.status = SandboxStatus::Failed;
    legacy.status = "failed".into();
    let confirmed = legacy.totals.clone();
    let mut renamed = value.clone();
    renamed.name = "changed-without-a-revision".into();
    assert_eq!(
        observe(Some(&legacy), &renamed, 1, &at(120), true),
        Err(UsageError::Conflict)
    );
    let mut released = value.clone();
    released.lifecycle.home.as_mut().unwrap().released = true;
    assert_eq!(
        observe(Some(&legacy), &released, 1, &at(120), true),
        Err(UsageError::Conflict)
    );
    let (settled, event) = observe(Some(&legacy), &value, 1, &at(120), true)
        .unwrap()
        .unwrap();
    assert_eq!(
        event.quantity,
        UsageTotals {
            disk_gib_seconds: 600,
            ..Default::default()
        }
    );
    assert_eq!(settled.totals.uptime_seconds, confirmed.uptime_seconds);
    assert_eq!(
        settled.totals.cpu_millis_seconds,
        confirmed.cpu_millis_seconds
    );
    assert_eq!(
        settled.totals.memory_mib_seconds,
        confirmed.memory_mib_seconds
    );
    assert_eq!(settled.source_revision, legacy.source_revision);
    assert_eq!(settled.source_seq, legacy.source_seq);
    assert_eq!(settled.revision, legacy.revision + 1);
    assert!(!settled.allocation.running);
    assert!(observe(Some(&settled), &value, 1, &at(120), true)
        .unwrap()
        .is_none());
}

#[test]
fn legacy_terminal_live_tail_and_report_preserve_the_historical_failure_interval() {
    let mut value = sandbox();
    retained_home(&mut value);
    let (active, _) = observe(None, &value, 1, &at(0), false).unwrap().unwrap();
    let (mut legacy, mut event) = observe(Some(&active), &value, 2, &at(60), false)
        .unwrap()
        .unwrap();
    legacy.status = "failed".into();
    event.status = "failed".into();
    event.kind = UsageEventKind::StateChanged;
    let totals = legacy.totals_at(timestamp(&at(120)).unwrap()).unwrap();
    assert_eq!(
        totals,
        UsageTotals {
            disk_gib_seconds: 1_200,
            ..legacy.totals.clone()
        }
    );
    let report = crate::sandbox_metrics::allocation::period(
        &legacy,
        &[event],
        crate::sandbox_metrics::Range {
            from: timestamp(&at(0)).unwrap(),
            to: timestamp(&at(120)).unwrap(),
        },
    );
    assert!(report.complete);
    assert_eq!(report.uptime_seconds, 60.0);
    assert_eq!(report.cpu_core_hours, 60.0 / 3_600.0);
    assert_eq!(report.memory_gib_hours, 120.0 / 3_600.0);
    assert_eq!(report.disk_gib_hours, 1_200.0 / 3_600.0);
}

#[test]
fn retirement_after_failure_cannot_restart_compute_while_binding_is_retained() {
    let mut value = sandbox();
    value.status = SandboxStatus::Failed;
    let (failed, _) = observe(None, &value, 1, &at(0), false).unwrap().unwrap();
    value.status = SandboxStatus::Retiring;
    value.revision += 1;
    let (retiring, _) = observe(Some(&failed), &value, 2, &at(60), false)
        .unwrap()
        .unwrap();
    assert!(!retiring.allocation.running);
    assert_eq!(
        retiring.totals_at(timestamp(&at(120)).unwrap()).unwrap(),
        UsageTotals::default()
    );
    assert!(observe(Some(&retiring), &value, 2, &at(120), true)
        .unwrap()
        .is_none());
    value.revision += 1;
    let (saved, _) = observe(Some(&retiring), &value, 2, &at(180), false)
        .unwrap()
        .unwrap();
    assert!(!saved.allocation.running);
}

#[test]
fn terminal_filter_preserves_nonterminal_allocations_and_unallocated_failures() {
    let mut value = sandbox();
    for status in [
        SandboxStatus::Active,
        SandboxStatus::Installing,
        SandboxStatus::Pausing,
        SandboxStatus::Reviving,
        SandboxStatus::Rotating,
        SandboxStatus::CredentialRotating,
        SandboxStatus::Retiring,
    ] {
        value.status = status;
        assert!(
            Allocation::from_sandbox(&value).running,
            "{}",
            status.name()
        );
    }
    for status in [SandboxStatus::Failed, SandboxStatus::Retired] {
        value.status = status;
        assert_eq!(Allocation::from_sandbox(&value), Allocation::default());
    }
    value.runtime = None;
    value.status = SandboxStatus::Failed;
    let (failed, _) = observe(None, &value, 1, &at(0), false).unwrap().unwrap();
    assert!(!failed.has_started);
    assert_eq!(
        failed.totals_at(timestamp(&at(120)).unwrap()).unwrap(),
        UsageTotals::default()
    );
}
