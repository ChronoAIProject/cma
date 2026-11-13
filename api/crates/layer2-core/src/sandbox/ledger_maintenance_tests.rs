use super::*;
use layer3_adapter::runtime_ledger_maintenance::{LedgerSourceFormatV1, LEDGER_BINDING_FORMAT};

const WRITERS: [LedgerWriterClass; 18] = [
    LedgerWriterClass::OwnerInput,
    LedgerWriterClass::OwnerControl,
    LedgerWriterClass::Creation,
    LedgerWriterClass::AgentWorker,
    LedgerWriterClass::WorkspaceWorker,
    LedgerWriterClass::Recorder,
    LedgerWriterClass::Checkpoint,
    LedgerWriterClass::Output,
    LedgerWriterClass::Reconciliation,
    LedgerWriterClass::Recovery,
    LedgerWriterClass::Rotation,
    LedgerWriterClass::Revival,
    LedgerWriterClass::Credentials,
    LedgerWriterClass::Retirement,
    LedgerWriterClass::HomeCleanup,
    LedgerWriterClass::Purge,
    LedgerWriterClass::Administration,
    LedgerWriterClass::Provider,
];
const PHASES: [LedgerMaintenancePhaseV1; 9] = [
    LedgerMaintenancePhaseV1::Requested,
    LedgerMaintenancePhaseV1::Draining,
    LedgerMaintenancePhaseV1::Excluded,
    LedgerMaintenancePhaseV1::BackedUp,
    LedgerMaintenancePhaseV1::Applying,
    LedgerMaintenancePhaseV1::HeldVerified,
    LedgerMaintenancePhaseV1::Released,
    LedgerMaintenancePhaseV1::RestoreHeld,
    LedgerMaintenancePhaseV1::AbortedHeld,
];

fn record(phase: LedgerMaintenancePhaseV1) -> LedgerMaintenanceRecordV1 {
    LedgerMaintenanceRecordV1 {
        format: LEDGER_MAINTENANCE_FORMAT.into(),
        binding: LedgerBindingV1 {
            format: LEDGER_BINDING_FORMAT.into(),
            sandbox_id: "fixture".into(),
            home_identity: "h".repeat(43),
            generation: 1,
            boot_id: "a".repeat(32),
            roots_sha256: "a".repeat(64),
            source_format: LedgerSourceFormatV1::V2,
            archive_id: uuid::Uuid::from_u128(1).to_string(),
            maintenance_id: "operation".into(),
            maintenance_epoch: 2,
            sandbox_revision: 7,
            candidate_sha256: "b".repeat(64),
            recovery_sha256: "c".repeat(64),
            exclusion_receipt_sha256: "d".repeat(64),
        },
        plan_sha256: "e".repeat(64),
        record_revision: 1,
        phase,
        authority_receipt_sha256: Some("f".repeat(64)),
        backup_receipt_sha256: Some("a".repeat(64)),
        release_receipt_sha256: (phase == LedgerMaintenancePhaseV1::Released)
            .then(|| "b".repeat(64)),
    }
}

#[test]
fn every_writer_holds_in_every_nonserving_phase_after_serialized_restart() {
    for phase in PHASES {
        let original = record(phase);
        let restored: LedgerMaintenanceRecordV1 =
            serde_json::from_slice(&serde_json::to_vec(&original).unwrap()).unwrap();
        for class in WRITERS {
            let result = check_ledger_writer(
                LedgerMaintenanceObservation::Present(&restored),
                class,
                Some(LedgerWriterFence {
                    binding: &restored.binding,
                }),
            );
            assert_eq!(
                result,
                if phase == LedgerMaintenancePhaseV1::Released {
                    Ok(())
                } else {
                    Err(LedgerMaintenanceRefusal::Held)
                }
            );
        }
    }
}

#[test]
fn every_writer_refuses_unknown_missing_and_old_release_fences() {
    let current = record(LedgerMaintenancePhaseV1::Released);
    let mut old = current.binding.clone();
    old.maintenance_epoch -= 1;
    for class in WRITERS {
        assert_eq!(
            check_ledger_writer(LedgerMaintenanceObservation::Unknown, class, None),
            Err(LedgerMaintenanceRefusal::UnknownAuthority)
        );
        assert_eq!(
            check_ledger_writer(LedgerMaintenanceObservation::KnownAbsent, class, None),
            Ok(())
        );
        assert_eq!(
            check_ledger_writer(LedgerMaintenanceObservation::Present(&current), class, None),
            Err(LedgerMaintenanceRefusal::StaleWriter)
        );
        assert_eq!(
            check_ledger_writer(
                LedgerMaintenanceObservation::Present(&current),
                class,
                Some(LedgerWriterFence { binding: &old })
            ),
            Err(LedgerMaintenanceRefusal::StaleWriter)
        );
    }
}

#[test]
fn malformed_or_missing_release_evidence_never_opens_any_writer() {
    for field in [
        "format",
        "plan_sha256",
        "authority_receipt_sha256",
        "backup_receipt_sha256",
        "release_receipt_sha256",
    ] {
        let mut value = serde_json::to_value(record(LedgerMaintenancePhaseV1::Released)).unwrap();
        value[field] = serde_json::json!("invalid");
        let invalid: LedgerMaintenanceRecordV1 = serde_json::from_value(value).unwrap();
        for class in WRITERS {
            assert_eq!(
                check_ledger_writer(
                    LedgerMaintenanceObservation::Present(&invalid),
                    class,
                    Some(LedgerWriterFence {
                        binding: &invalid.binding
                    })
                ),
                Err(LedgerMaintenanceRefusal::InvalidEvidence)
            );
        }
    }
    for field in [
        "authority_receipt_sha256",
        "backup_receipt_sha256",
        "release_receipt_sha256",
    ] {
        let mut value = serde_json::to_value(record(LedgerMaintenancePhaseV1::Released)).unwrap();
        value[field] = serde_json::Value::Null;
        let invalid: LedgerMaintenanceRecordV1 = serde_json::from_value(value).unwrap();
        assert_eq!(
            invalid.validate(),
            Err(LedgerMaintenanceRefusal::InvalidEvidence)
        );
    }
}

#[test]
fn clock_expiry_cannot_be_deserialized_as_a_release() {
    let mut value = serde_json::to_value(record(LedgerMaintenancePhaseV1::Requested)).unwrap();
    value["lease_expired"] = serde_json::json!(true);
    assert!(serde_json::from_value::<LedgerMaintenanceRecordV1>(value).is_err());
    let mut invalid = record(LedgerMaintenancePhaseV1::Requested);
    invalid.release_receipt_sha256 = Some("f".repeat(64));
    assert_eq!(
        invalid.validate(),
        Err(LedgerMaintenanceRefusal::InvalidEvidence)
    );
}

#[test]
fn phase_grammar_cannot_skip_barriers_or_release_a_restored_aborted_home() {
    use LedgerMaintenancePhaseV1::*;
    for pair in [
        Requested,
        Draining,
        Excluded,
        BackedUp,
        Applying,
        HeldVerified,
        Released,
    ]
    .windows(2)
    {
        assert!(pair[0].accepts_successor(pair[1]));
        assert!(!pair[1].accepts_successor(pair[0]));
    }
    for phase in PHASES {
        assert!(!phase.accepts_successor(phase));
        if phase != HeldVerified {
            assert!(!phase.accepts_successor(Released));
        }
        assert!(!Released.accepts_successor(phase));
        assert!(!AbortedHeld.accepts_successor(phase));
    }
    assert!(Applying.accepts_successor(RestoreHeld));
    assert!(RestoreHeld.accepts_successor(AbortedHeld));
    assert!(!RestoreHeld.accepts_successor(HeldVerified));
}
