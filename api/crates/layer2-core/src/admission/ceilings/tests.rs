use super::*;
use crate::{admission::AdmissionRefusal, SubjectId};

#[test]
fn defaults_are_unlimited_and_zero_is_a_real_ceiling() {
    let mut ceilings = AdmissionCeilings::default();
    assert_eq!(ceilings.registrations, None);
    assert_eq!(ceilings.effective_retained_homes(0), None);
    ceilings.retained_homes = Some(0);
    assert_eq!(ceilings.effective_retained_homes(10), Some(0));
    ceilings.retained_homes = Some(20);
    assert_eq!(ceilings.effective_retained_homes(10), Some(10));
    assert_eq!(ceilings.effective_retained_homes(0), Some(20));
}

#[test]
fn reserves_cannot_exceed_their_ceiling() {
    let mut ceilings = AdmissionCeilings {
        retained_homes: Some(5),
        active_sessions: Some(3),
        ..Default::default()
    };
    ceilings.existing_reserve.retained_homes = Some(6);
    assert!(ceilings.validate().is_err());
    ceilings.existing_reserve.retained_homes = Some(5);
    ceilings.existing_reserve.active_sessions = Some(4);
    assert!(ceilings.validate().is_err());
    ceilings.existing_reserve.active_sessions = Some(3);
    assert!(ceilings.validate().is_ok());
}

#[test]
fn replacement_uses_an_independent_cas_and_preserves_evidence() {
    let current = AdmissionCeilingsState::default();
    let actor = SubjectId::new("operator-a");
    let ceilings = AdmissionCeilings {
        registrations: Some(2),
        ..Default::default()
    };
    let changed = current
        .changed(0, &actor, "bound new registrations", ceilings.clone(), 100)
        .unwrap()
        .unwrap();
    assert_eq!(changed.revision, 1);
    changed.validate().unwrap();
    assert!(changed
        .changed(0, &actor, "stale writer", ceilings.clone(), 101)
        .is_err());
    assert!(changed
        .changed(1, &actor, "same limits", ceilings, 101)
        .unwrap()
        .is_none());
    assert_eq!(
        changed.last_change.as_ref().unwrap().old_ceilings,
        AdmissionCeilings::default()
    );
    let mut broken = changed.clone();
    broken
        .last_change
        .as_mut()
        .unwrap()
        .new_ceilings
        .registrations = Some(9);
    assert!(broken.validate().is_err());
}

#[test]
fn session_refusal_contains_no_capacity_values() {
    let refusal = AdmissionRefusal::Capacity(CapacityKind::ActiveSessions);
    assert_eq!(refusal.code(), "session_capacity_unavailable");
    assert!(!refusal.guidance().chars().any(|c| c.is_ascii_digit()));
}

#[tokio::test]
async fn ceiling_service_requires_authority_and_its_independent_write_gate() {
    use crate::{Authorization, Caller, Capability, CoreError};
    use std::sync::Arc;
    let store = Arc::new(MemoryAdmissionCeilingsStore::default());
    let service = AdmissionCeilingsService::new(store.clone());
    let mut caller = Caller {
        subject: SubjectId::new("operator"),
        authorization: Authorization::new([]),
    };
    assert_eq!(service.read(&caller).await, Err(CoreError::Forbidden));
    caller.authorization = Authorization::new([Capability::ConfigurePlatform]);
    assert_eq!(
        service
            .replace(
                &caller,
                0,
                "bound registrations",
                AdmissionCeilings {
                    registrations: Some(1),
                    ..Default::default()
                }
            )
            .await,
        Err(CoreError::AdmissionCeilingsWritesUnavailable)
    );
    assert!(store.audit_records().unwrap().is_empty());
    let service = service.with_writes_enabled(true);
    let saved = service
        .replace(
            &caller,
            0,
            "bound registrations",
            AdmissionCeilings {
                registrations: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(saved.revision, 1);
    let history = service.read_with_history(&caller).await.unwrap();
    assert_eq!(history.history.len(), 1);
    assert_eq!(store.audit_records().unwrap().len(), 1);
    service
        .replace(&caller, 1, "same policy", saved.ceilings)
        .await
        .unwrap();
    assert_eq!(store.audit_records().unwrap().len(), 1);
}

#[test]
fn ceiling_evidence_rejects_unknown_fields_invalid_reasons_and_revision_overflow() {
    let actor = SubjectId::new("operator");
    let current = AdmissionCeilingsState::default();
    for reason in ["", " ", "contains\ncontrol"] {
        assert!(current
            .changed(
                0,
                &actor,
                reason,
                AdmissionCeilings {
                    registrations: Some(1),
                    ..Default::default()
                },
                100
            )
            .is_err());
    }
    let value = serde_json::json!({"registrations":1,"unknown":false});
    assert!(serde_json::from_value::<AdmissionCeilings>(value).is_err());
    let mut state = current
        .changed(
            0,
            &actor,
            "bounded",
            AdmissionCeilings {
                registrations: Some(1),
                ..Default::default()
            },
            100,
        )
        .unwrap()
        .unwrap();
    state.revision = i64::MAX as u64;
    state.last_change.as_mut().unwrap().revision = state.revision;
    assert!(state
        .changed(
            state.revision,
            &actor,
            "cannot overflow",
            AdmissionCeilings::default(),
            101
        )
        .is_err());
}
