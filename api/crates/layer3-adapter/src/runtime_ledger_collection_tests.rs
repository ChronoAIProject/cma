use super::*;

fn request() -> CollectionRequest {
    serde_json::from_value(serde_json::json!({
        "format":COLLECTION_FORMAT,"operation_id":uuid::Uuid::from_u128(2).to_string(),
        "binding":{"format":"runtime-ledger-binding-v1","sandbox_id":"fixture-sandbox",
            "home_identity":"a".repeat(43),"generation":1,"boot_id":"b".repeat(32),
            "roots_sha256":"b".repeat(64),"source_format":"v1","archive_id":uuid::Uuid::from_u128(1).to_string(),
            "maintenance_id":"fixture-maintenance","maintenance_epoch":1,"sandbox_revision":1,
            "candidate_sha256":"c".repeat(64),"recovery_sha256":"d".repeat(64),
            "exclusion_receipt_sha256":"e".repeat(64)},
        "active_revision":1,"page_sha256":"f".repeat(64),
        "groups":[{"kind":"agent","agent_id":"fixture-agent"}],
    }))
    .unwrap()
}

#[test]
fn bounded_collection_wire_roundtrip_refuses_future_duplicate_and_oversized_requests() {
    let value = request();
    assert!(value.validate());
    let bytes = serde_json::to_vec(&value).unwrap();
    assert_eq!(
        CollectionRequest::decode(&bytes).unwrap().fingerprint(),
        value.fingerprint()
    );
    for case in 0..6 {
        let mut changed = request();
        match case {
            0 => changed.groups.clear(),
            1 => changed.groups.push(changed.groups[0].clone()),
            2 => changed.groups = vec![changed.groups[0].clone(); 65],
            3 => changed.binding.maintenance_epoch = 0,
            4 => changed.active_revision = 0,
            _ => changed.operation_id = uuid::Uuid::nil().to_string(),
        }
        assert!(CollectionRequest::decode(&serde_json::to_vec(&changed).unwrap()).is_none());
    }
    let mut json = serde_json::to_value(value).unwrap();
    json["apply_authorized"] = true.into();
    assert!(CollectionRequest::decode(&serde_json::to_vec(&json).unwrap()).is_none());
    assert!(CollectionRequest::decode(&vec![b' '; MAX_COLLECTION_REQUEST_BYTES + 1]).is_none());
}

#[test]
fn bounded_collection_request_fingerprint_binds_authority_source_and_original_operation() {
    let value = request();
    for case in 0..7 {
        let mut changed = value.clone();
        match case {
            0 => changed.operation_id = uuid::Uuid::new_v4().to_string(),
            1 => changed.binding.maintenance_epoch += 1,
            2 => changed.binding.sandbox_revision += 1,
            3 => changed.binding.generation += 1,
            4 => changed.active_revision += 1,
            5 => changed.binding.home_identity = "z".repeat(43),
            _ => changed.page_sha256 = "1".repeat(64),
        }
        assert!(changed.validate());
        assert_ne!(changed.fingerprint(), value.fingerprint());
    }
}

#[test]
fn candidate_page_requires_explicit_end_or_changed_continuation() {
    let value = request();
    let mut page = LedgerCandidatePageV1 {
        format: "runtime-ledger-candidates-v1".into(),
        binding: value.binding,
        active_revision: 1,
        cursor: None,
        next_cursor: Some("page-64".into()),
        groups: vec![],
        complete: false,
    };
    assert!(page.validate());
    assert!(!page.complete);
    page.next_cursor = None;
    assert!(!page.validate());
    page.complete = true;
    assert!(page.validate());
    page.active_revision = 0;
    assert!(!page.validate());
}

fn observation(request: &CollectionRequest) -> CollectionObservation {
    let evidence = LedgerGroupEvidenceV1 {
        format: "runtime-ledger-group-evidence-v1".into(),
        request_sha256: request.fingerprint().unwrap(),
        binding_sha256: request.binding.binding_sha256().unwrap(),
        group_key: request.groups[0].key(),
        sandbox_revision: request.binding.sandbox_revision,
        resource_revisions: vec![LedgerResourceRevisionV1 {
            resource_sha256: "1".repeat(64),
            revision: 1,
        }],
        source_sha256: "2".repeat(64),
        observed_at: "2026-11-11T00:00:00Z".into(),
        decision: LedgerGroupDecisionV1::SettledDeleted,
    };
    let bytes = serde_json::to_vec_pretty(&evidence).unwrap();
    let hash = format!("{:x}", Sha256::digest(&bytes));
    CollectionObservation {
        format: "runtime-ledger-authority-batch-v1".into(),
        request_sha256: request.fingerprint().unwrap(),
        groups: vec![LedgerGroupAuthorityV1 {
            format: "runtime-ledger-group-authority-v1".into(),
            binding: request.binding.clone(),
            group_key: evidence.group_key,
            sandbox_revision: evidence.sandbox_revision,
            resource_revisions: evidence.resource_revisions,
            observed_at: evidence.observed_at,
            observation_sha256: hash.clone(),
            decision: evidence.decision,
            evidence_reference: format!("sha256:{hash}"),
        }],
        evidence: vec![bytes],
        dependencies_read: 2,
        source_bytes: 512,
    }
}

#[test]
fn collection_authority_requires_original_bytes_complete_evidence_and_exact_bindings() {
    let request = request();
    let value = observation(&request);
    assert!(value.validate_for(&request));
    for case in 0..10 {
        let mut changed = value.clone();
        match case {
            0 => changed.groups[0].binding.maintenance_epoch += 1,
            1 => changed.groups[0].binding.boot_id.push_str("changed"),
            2 => changed.groups[0].sandbox_revision += 1,
            3 => changed.groups[0].decision = LedgerGroupDecisionV1::RetainLive,
            4 => changed.groups[0].resource_revisions[0].revision += 1,
            5 => changed.evidence[0].push(b' '),
            6 => changed.groups.push(changed.groups[0].clone()),
            7 => changed.dependencies_read = MAX_COLLECTION_DEPENDENCIES as u64 + 1,
            8 => changed.source_bytes = 16 * 1024 * 1024 + 1,
            _ => changed.request_sha256 = "3".repeat(64),
        }
        assert!(!changed.validate_for(&request), "case {case}");
    }
    let mut reparsed: LedgerGroupEvidenceV1 = serde_json::from_slice(&value.evidence[0]).unwrap();
    reparsed.resource_revisions.clear();
    let mut empty = value;
    empty.groups[0].resource_revisions.clear();
    empty.evidence[0] = serde_json::to_vec(&reparsed).unwrap();
    let hash = format!("{:x}", Sha256::digest(&empty.evidence[0]));
    empty.groups[0].observation_sha256 = hash.clone();
    empty.groups[0].evidence_reference = format!("sha256:{hash}");
    assert!(!empty.validate_for(&request));
}

#[test]
fn collection_uses_the_shared_binding_identity_for_both_source_formats() {
    use crate::runtime_ledger_maintenance::LedgerSourceFormatV1;
    for format in [LedgerSourceFormatV1::V1, LedgerSourceFormatV1::V2] {
        let mut request = request();
        request.binding.source_format = format;
        assert_eq!(request.binding.validate(), Ok(()));
        let observation = observation(&request);
        assert!(observation.validate_for(&request));
        let evidence: LedgerGroupEvidenceV1 =
            serde_json::from_slice(&observation.evidence[0]).unwrap();
        assert_eq!(
            evidence.binding_sha256,
            request.binding.binding_sha256().unwrap()
        );
        let mut other = request.clone();
        other.binding.source_format = match format {
            LedgerSourceFormatV1::V1 => LedgerSourceFormatV1::V2,
            LedgerSourceFormatV1::V2 => LedgerSourceFormatV1::V1,
        };
        assert_ne!(request.fingerprint(), other.fingerprint());
        assert!(!observation.validate_for(&other));
    }
}

#[test]
fn collection_entry_points_reject_invalid_in_process_binding_mutations() {
    let accepted = request();
    let evidence = observation(&accepted);
    for case in 0..9 {
        let mut invalid = accepted.clone();
        match case {
            0 => invalid.binding.boot_id = "fixture-boot".into(),
            1 => invalid.binding.boot_id = "B".repeat(32),
            2 => invalid.binding.archive_id = "fixture-archive".into(),
            3 => invalid.binding.archive_id = uuid::Uuid::nil().to_string(),
            4 => invalid.binding.archive_id = uuid::Uuid::from_u128(1).simple().to_string(),
            5 => invalid.binding.sandbox_id.push(' '),
            6 => invalid.binding.maintenance_id.insert(0, ' '),
            7 => invalid.binding.format = "runtime-ledger-binding-v2".into(),
            _ => invalid.binding.roots_sha256 = "A".repeat(64),
        }
        assert!(invalid.binding.validate().is_err(), "case {case}");
        assert!(!invalid.validate(), "case {case}");
        assert!(invalid.fingerprint().is_none(), "case {case}");
        assert!(!evidence.validate_for(&invalid), "case {case}");
        let page = LedgerCandidatePageV1 {
            format: "runtime-ledger-candidates-v1".into(),
            binding: invalid.binding.clone(),
            active_revision: 1,
            cursor: None,
            next_cursor: None,
            groups: invalid.groups,
            complete: true,
        };
        assert!(!page.validate(), "case {case}");
        let mut invalid_authority = evidence.clone();
        invalid_authority.groups[0].binding = invalid.binding;
        assert!(!invalid_authority.validate_for(&accepted), "case {case}");
    }
}
