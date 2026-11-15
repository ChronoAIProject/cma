use std::sync::Arc;

use super::*;
use crate::{Authorization, Caller, Capability, CoreError, SubjectId};

fn operator() -> Caller {
    Caller::new(
        SubjectId::new("operator-a"),
        Authorization::new([Capability::ConfigurePlatform]),
    )
}

#[tokio::test]
async fn control_writes_default_closed_while_reads_remain_available() {
    let store = Arc::new(MemoryAdmissionControlStore::default());
    let service = AdmissionControlService::new(store.clone());
    assert!(!service.writes_enabled());
    assert_eq!(
        service.read(&operator()).await.unwrap(),
        AdmissionControl::default()
    );
    for result in [
        service.stop(&operator(), 0, "contain incident").await,
        service.resume(&operator(), 0, "restore admission").await,
    ] {
        assert_eq!(result, Err(CoreError::AdmissionControlWritesUnavailable));
    }
    assert!(store.audit_records().unwrap().is_empty());
}

#[tokio::test]
async fn configuring_platform_is_the_authority_for_every_control_entry() {
    let store = Arc::new(MemoryAdmissionControlStore::default());
    let service = AdmissionControlService::new(store.clone()).with_writes_enabled(true);
    let caller = Caller::new(SubjectId::new("owner-a"), Authorization::none());
    assert_eq!(service.read(&caller).await, Err(CoreError::Forbidden));
    assert_eq!(
        service.read_with_history(&caller).await,
        Err(CoreError::Forbidden)
    );
    assert_eq!(
        service.stop(&caller, 0, "reason").await,
        Err(CoreError::Forbidden)
    );
    assert_eq!(
        service.resume(&caller, 0, "reason").await,
        Err(CoreError::Forbidden)
    );
    assert!(store.audit_records().unwrap().is_empty());
}

async fn immutable_history_contract(store: &impl AdmissionControlStore) {
    let initial = store.read_with_history().await.unwrap();
    assert_eq!(initial.control, AdmissionControl::default());
    assert!(initial.history.is_empty());
    for revision in 0..26 {
        let actor = SubjectId::new(format!("operator-{revision}"));
        let state = store
            .change(
                revision,
                &actor,
                &format!("reason-{revision}"),
                revision % 2 == 0,
                100 + revision,
            )
            .await
            .unwrap();
        let snapshot = store.read_with_history().await.unwrap();
        assert_eq!(snapshot.control, state);
        assert_eq!(snapshot.history.first(), state.last_change.as_ref());
        assert_eq!(
            snapshot.history.len(),
            ((revision + 1) as usize).min(ADMISSION_HISTORY_LIMIT)
        );
        assert_eq!(
            snapshot
                .history
                .iter()
                .map(|change| change.revision)
                .collect::<Vec<_>>(),
            ((revision + 1).saturating_sub(19).max(1)..=revision + 1)
                .rev()
                .collect::<Vec<_>>()
        );
        assert_eq!(snapshot.history[0].actor, actor);
        let noop_revision = if state.operator_stop.is_some() {
            0
        } else {
            state.revision
        };
        store
            .change(
                noop_revision,
                &SubjectId::new("other"),
                "no-op",
                state.operator_stop.is_some(),
                200 + revision,
            )
            .await
            .unwrap();
        assert_eq!(store.read_with_history().await.unwrap(), snapshot);
    }
}

#[tokio::test]
async fn both_memory_adapters_keep_bounded_newest_first_history_without_noop_records() {
    immutable_history_contract(&MemoryAdmissionControlStore::default()).await;
    immutable_history_contract(&crate::sandbox::InMemorySandboxStore::default()).await;
}

#[test]
fn admission_history_refuses_missing_changed_or_out_of_order_evidence() {
    let first = AdmissionControl::default()
        .changed(0, &SubjectId::new("operator-a"), "incident", true, 100)
        .unwrap()
        .unwrap();
    let second = first
        .changed(1, &SubjectId::new("operator-b"), "resolved", false, 101)
        .unwrap()
        .unwrap();
    let history = vec![
        second.last_change.clone().unwrap(),
        first.last_change.unwrap(),
    ];
    let valid = AdmissionControlRead {
        control: second,
        history,
    };
    valid.validate().unwrap();
    let mut missing = valid.clone();
    missing.history.pop();
    assert!(missing.validate().is_err());
    let mut reordered = valid.clone();
    reordered.history.reverse();
    assert!(reordered.validate().is_err());
    let mut changed = valid;
    changed.history[0].reason = "rewritten".into();
    assert!(changed.validate().is_err());
}

#[tokio::test]
async fn first_stop_wins_and_every_change_has_one_atomic_audit() {
    let store = MemoryAdmissionControlStore::default();
    let actor = SubjectId::new("operator-a");
    let stopped = store
        .change(0, &actor, "contain incident", true, 100)
        .await
        .unwrap();
    assert_eq!(stopped.revision, 1);
    let stop = stopped.operator_stop.as_ref().unwrap();
    assert_eq!((stop.since, stop.revision), (100, 1));
    assert_eq!(stop.reason, "contain incident");
    assert_eq!(
        store
            .change(0, &SubjectId::new("operator-b"), "second", true, 101)
            .await
            .unwrap(),
        stopped
    );
    assert_eq!(store.audit_records().unwrap().len(), 1);
    let resumed = store
        .change(1, &actor, "resolved", false, 102)
        .await
        .unwrap();
    assert_eq!(resumed.revision, 2);
    assert!(resumed.operator_stop.is_none());
    let change = resumed.last_change.as_ref().unwrap();
    assert_eq!(change.old_stop, stopped.operator_stop);
    assert_eq!(change.new_stop, None);
    assert_eq!(change.reason, "resolved");
    assert_eq!(store.audit_records().unwrap().len(), 2);
    assert_eq!(
        store.change(2, &actor, "repeat", false, 103).await.unwrap(),
        resumed
    );
    assert_eq!(store.audit_records().unwrap().len(), 2);
}

#[tokio::test]
async fn stale_resume_cannot_lift_a_new_stop() {
    let store = MemoryAdmissionControlStore::default();
    let actor = SubjectId::new("operator-a");
    store.change(0, &actor, "first", true, 100).await.unwrap();
    store
        .change(1, &actor, "resolved", false, 101)
        .await
        .unwrap();
    let stopped = store.change(2, &actor, "second", true, 102).await.unwrap();
    assert_eq!(
        store.change(1, &actor, "stale", false, 103).await,
        Err(crate::StoreError::RevisionConflict {
            expected: 1,
            actual: 3
        })
    );
    assert_eq!(store.read().await.unwrap(), stopped);
    assert_eq!(store.audit_records().unwrap().len(), 3);
}

#[tokio::test]
async fn reasons_are_bounded_and_nonempty_before_persistence() {
    let store = Arc::new(MemoryAdmissionControlStore::default());
    let service = AdmissionControlService::new(store.clone()).with_writes_enabled(true);
    for reason in [
        "".to_owned(),
        "   ".to_owned(),
        "bad\nreason".to_owned(),
        "a".repeat(513),
    ] {
        assert!(matches!(
            service.stop(&operator(), 0, &reason).await,
            Err(CoreError::InvalidRequest(_))
        ));
    }
    assert_eq!(store.read().await.unwrap().revision, 0);
}

#[test]
fn admission_refusal_keeps_budget_and_operator_causes_distinct() {
    assert_eq!(AdmissionRefusal::Paused.code(), "admission_paused");
    assert_eq!(
        AdmissionRefusal::Paused.guidance(),
        ADMISSION_PAUSED_GUIDANCE
    );
    let budget = crate::budget::BudgetError::PlatformExhausted;
    assert_eq!(AdmissionRefusal::from(budget).code(), budget.code());
}

#[test]
fn stored_control_refuses_inconsistent_change_evidence() {
    let stopped = AdmissionControl::default()
        .changed(0, &SubjectId::new("operator-a"), "incident", true, 100)
        .unwrap()
        .unwrap();
    let mut invalid = stopped.clone();
    invalid.last_change.as_mut().unwrap().old_stop = stopped.operator_stop.clone();
    assert!(invalid.validate().is_err());
    let resumed = stopped
        .changed(1, &SubjectId::new("operator-a"), "resolved", false, 101)
        .unwrap()
        .unwrap();
    let mut invalid = resumed;
    invalid
        .last_change
        .as_mut()
        .unwrap()
        .old_stop
        .as_mut()
        .unwrap()
        .revision = 0;
    assert!(invalid.validate().is_err());
}
