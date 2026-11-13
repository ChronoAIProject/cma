use super::*;
use layer3_adapter::runtime_ledger_maintenance::{LedgerSourceFormatV1, LEDGER_BINDING_FORMAT};

fn plan() -> LedgerRecoveryPlanV1 {
    LedgerRecoveryPlanV1 {
        format: LEDGER_RECOVERY_PLAN_FORMAT.into(),
        operation_id: "fixture-operation".into(),
        binding: LedgerBindingV1 {
            format: LEDGER_BINDING_FORMAT.into(),
            sandbox_id: "fixture-source-sandbox".into(),
            home_identity: "h".repeat(43),
            generation: 1,
            boot_id: "a".repeat(32),
            roots_sha256: "a".repeat(64),
            source_format: LedgerSourceFormatV1::V1,
            archive_id: uuid::Uuid::from_u128(1).to_string(),
            maintenance_id: "fixture-maintenance".into(),
            maintenance_epoch: 2,
            sandbox_revision: 7,
            candidate_sha256: "b".repeat(64),
            recovery_sha256: "c".repeat(64),
            exclusion_receipt_sha256: "d".repeat(64),
        },
        source_agent_id: AgentId("fixture-source-agent".into()),
        source_agent_revision: 4,
        source_log_head: 0,
        source_command_lane_revision: 1,
        failure_evidence_sha256: "a".repeat(64),
        attempt_evidence_sha256: "b".repeat(64),
        disposition: LedgerRecoveryDispositionV1::MigrateToNewResources,
        backup_sha256: "c".repeat(64),
        salvage_sha256: "d".repeat(64),
        source_revisions_sha256: "e".repeat(64),
        consent_receipt_sha256: "f".repeat(64),
        successor: Some(LedgerRecoverySuccessorV1 {
            sandbox_id: SandboxId("fixture-successor-sandbox".into()),
            agent_id: AgentId("fixture-successor-agent".into()),
        }),
    }
}

fn artifact(
    plan: &LedgerRecoveryPlanV1,
) -> Result<LedgerRecoveryArtifactV1, LedgerRecoveryRefusal> {
    LedgerRecoveryArtifactV1::parse(&serde_json::to_vec(plan).unwrap())
}

#[test]
fn migration_requires_explicit_distinct_successors_and_preserves_original_bytes() {
    let original = plan();
    let bytes = serde_json::to_vec_pretty(&original).unwrap();
    let retained = LedgerRecoveryArtifactV1::parse(&bytes).unwrap();
    assert_eq!(retained.original_bytes(), bytes);
    assert_eq!(retained.plan(), &original);
    for case in 0..4 {
        let mut changed = original.clone();
        match case {
            0 => changed.successor = None,
            1 => changed.successor.as_mut().unwrap().agent_id = original.source_agent_id.clone(),
            2 => {
                changed.successor.as_mut().unwrap().sandbox_id =
                    SandboxId(original.binding.sandbox_id.clone())
            }
            _ => changed.successor.as_mut().unwrap().agent_id = AgentId(String::new()),
        }
        assert!(matches!(
            artifact(&changed),
            Err(LedgerRecoveryRefusal::InvalidEvidence)
        ));
    }
}

#[test]
fn same_agent_retry_and_preservation_cannot_smuggle_successor_or_input_replay() {
    for disposition in [
        LedgerRecoveryDispositionV1::SameAgentRetry,
        LedgerRecoveryDispositionV1::PreserveHeld,
    ] {
        let mut value = plan();
        value.disposition = disposition;
        assert!(artifact(&value).is_err());
        value.successor = None;
        assert!(artifact(&value).is_ok());
        let mut json = serde_json::to_value(value).unwrap();
        json["replay_attempted_input"] = serde_json::json!(true);
        assert!(LedgerRecoveryArtifactV1::parse(&serde_json::to_vec(&json).unwrap()).is_err());
    }
}

#[test]
fn retry_retains_exact_operation_and_bytes_across_authority_takeover() {
    let original = artifact(&plan()).unwrap();
    let restored = LedgerRecoveryArtifactV1::parse(original.original_bytes()).unwrap();
    assert_eq!(restored.validate_retry(&original), Ok(()));
    let pretty =
        LedgerRecoveryArtifactV1::parse(&serde_json::to_vec_pretty(original.plan()).unwrap())
            .unwrap();
    assert_eq!(
        pretty.validate_retry(&original),
        Err(LedgerRecoveryRefusal::RequestChanged)
    );
    let mut changed = plan();
    changed.binding.maintenance_epoch += 1;
    assert_eq!(
        artifact(&changed).unwrap().validate_retry(&original),
        Err(LedgerRecoveryRefusal::RequestChanged)
    );
    let mut receipt = LedgerRecoveryReceiptV1 {
        format: "ledger-recovery-receipt-v1".into(),
        plan_sha256: original.sha256().into(),
        record_revision: 1,
        maintenance_epoch: 3,
        phase: LedgerRecoveryPhaseV1::Accepted,
        outcome_sha256: "a".repeat(64),
    };
    assert_eq!(receipt.validate_for(&original), Ok(()));
    receipt.maintenance_epoch = 1;
    assert_eq!(
        receipt.validate_for(&original),
        Err(LedgerRecoveryRefusal::InvalidEvidence)
    );
    receipt.maintenance_epoch = 3;
    receipt.plan_sha256 = "b".repeat(64);
    assert_eq!(
        receipt.validate_for(&original),
        Err(LedgerRecoveryRefusal::InvalidEvidence)
    );
}

#[test]
fn missing_consent_or_backup_bad_revisions_and_unknown_versions_refuse() {
    let original = serde_json::to_value(plan()).unwrap();
    for field in [
        "consent_receipt_sha256",
        "failure_evidence_sha256",
        "attempt_evidence_sha256",
        "backup_sha256",
        "salvage_sha256",
        "source_revisions_sha256",
        "operation_id",
    ] {
        let mut json = original.clone();
        json[field] = serde_json::json!("");
        assert!(LedgerRecoveryArtifactV1::parse(&serde_json::to_vec(&json).unwrap()).is_err());
    }
    for field in ["source_agent_revision", "source_command_lane_revision"] {
        let mut json = original.clone();
        json[field] = serde_json::json!(0);
        assert!(LedgerRecoveryArtifactV1::parse(&serde_json::to_vec(&json).unwrap()).is_err());
    }
    let mut json = original;
    json["format"] = serde_json::json!("ledger-recovery-plan-v2");
    assert!(LedgerRecoveryArtifactV1::parse(&serde_json::to_vec(&json).unwrap()).is_err());
    assert!(matches!(
        LedgerRecoveryArtifactV1::parse(&vec![b' '; MAX_LEDGER_RECOVERY_PLAN_BYTES + 1]),
        Err(LedgerRecoveryRefusal::Oversized)
    ));
}

#[test]
fn debug_cannot_emit_source_successor_or_operation_identifiers() {
    let original = plan();
    let retained = artifact(&original).unwrap();
    let debug = format!("{original:?} {retained:?}");
    for private in [
        original.operation_id.as_str(),
        original.source_agent_id.as_str(),
        original.binding.sandbox_id.as_str(),
        original.binding.maintenance_id.as_str(),
        original.successor.as_ref().unwrap().sandbox_id.as_str(),
        original.successor.as_ref().unwrap().agent_id.as_str(),
    ] {
        assert!(!debug.contains(private));
    }
    assert!(debug.contains(retained.sha256()));
}
