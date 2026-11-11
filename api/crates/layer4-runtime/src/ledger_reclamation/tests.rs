use super::*;
use layer3_adapter::runtime_ledger_collection::{CollectionGroup, LedgerGroupEvidenceV1};

fn fixture() -> (RuntimeConfig, LedgerBindingV1, String, [Vec<u8>; 3]) {
    let mut config = RuntimeConfig::for_test("/tmp/collection-evidence-fixture".into());
    config.home_identity = Some("a".repeat(43));
    let group = CollectionGroup::Agent {
        agent_id: "retained-fixture".into(),
    };
    let key = group.key();
    let (binding, originals) = crate::retirement_archive::collection_probe::artifacts(
        &config,
        &uuid::Uuid::new_v4().to_string(),
        7,
        group,
    )
    .unwrap();
    (config, binding, key, originals)
}

fn encode(value: &impl serde::Serialize) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

fn rebind_request(originals: &mut [Vec<u8>; 3]) {
    let mut request: CollectionRequest = serde_json::from_slice(&originals[1]).unwrap();
    request.page_sha256 = hash(&originals[0]);
    originals[1] = encode(&request);
    let mut observed: CollectionObservation = serde_json::from_slice(&originals[2]).unwrap();
    observed.request_sha256 = request.fingerprint().unwrap();
    for (authority, bytes) in observed.groups.iter_mut().zip(&mut observed.evidence) {
        let mut evidence: LedgerGroupEvidenceV1 = serde_json::from_slice(bytes).unwrap();
        evidence.request_sha256 = observed.request_sha256.clone();
        *bytes = encode(&evidence);
        authority.observation_sha256 = hash(bytes);
        authority.evidence_reference = format!("sha256:{}", hash(bytes));
    }
    assert!(observed.validate_for(&request));
    originals[2] = encode(&observed);
}

#[test]
fn original_artifact_bytes_and_operation_survive_without_recanonicalization() {
    let (_, binding, key, originals) = fixture();
    let parsed = CollectionEvidence::parse(&binding, &key, originals.clone()).unwrap();
    assert_eq!(parsed.originals(), originals.each_ref().map(Vec::as_slice));
    let request: CollectionRequest = serde_json::from_slice(&originals[1]).unwrap();
    assert_eq!(parsed.operation_id(), request.operation_id);
    let mut respelled = originals.clone();
    respelled[1] = encode(&request);
    let second = CollectionEvidence::parse(&binding, &key, respelled).unwrap();
    assert_eq!(parsed.operation_id(), second.operation_id());
    assert_ne!(parsed.fingerprint(), second.fingerprint());
    let debug = format!("{parsed:?}");
    for private in [
        &binding.sandbox_id,
        &binding.home_identity,
        &binding.maintenance_id,
    ] {
        assert!(!debug.contains(private));
    }
    assert!(!debug.contains("retained-fixture"));
}

#[test]
fn changed_accepted_binding_and_artifact_bounds_refuse() {
    let (_, binding, key, originals) = fixture();
    for case in 0..6 {
        let mut accepted = binding.clone();
        match case {
            0 => accepted.maintenance_epoch += 1,
            1 => accepted.boot_id = "c".repeat(32),
            2 => accepted.archive_id = uuid::Uuid::new_v4().to_string(),
            3 => accepted.roots_sha256 = "a".repeat(64),
            4 => accepted.recovery_sha256 = "b".repeat(64),
            _ => accepted.source_format = LedgerSourceFormatV1::V1,
        }
        assert!(CollectionEvidence::parse(&accepted, &key, originals.clone()).is_err());
    }
    for (index, bound) in [
        2 * 1024 * 1024,
        MAX_COLLECTION_REQUEST_BYTES,
        MAX_COLLECTION_OBSERVATION_BYTES,
    ]
    .into_iter()
    .enumerate()
    {
        let mut changed = originals.clone();
        changed[index] = vec![b' '; bound + 1];
        assert!(CollectionEvidence::parse(&binding, &key, changed).is_err());
        let mut changed = originals.clone();
        let mut value: serde_json::Value = serde_json::from_slice(&changed[index]).unwrap();
        value["future"] = true.into();
        changed[index] = encode(&value);
        assert!(CollectionEvidence::parse(&binding, &key, changed).is_err());
    }
}

#[test]
fn candidate_membership_and_exact_page_digest_are_independent_requirements() {
    let (_, binding, key, originals) = fixture();
    let mut changed = originals.clone();
    changed[0].push(b' ');
    assert!(CollectionEvidence::parse(&binding, &key, changed).is_err());
    let mut changed = originals;
    let mut page: LedgerCandidatePageV1 = serde_json::from_slice(&changed[0]).unwrap();
    page.groups = vec![CollectionGroup::Agent {
        agent_id: "another-fixture".into(),
    }];
    assert!(page.validate());
    changed[0] = encode(&page);
    rebind_request(&mut changed);
    assert!(CollectionEvidence::parse(&binding, &key, changed).is_err());
}

#[test]
fn valid_live_and_unknown_decisions_remain_retained() {
    let (_, binding, key, originals) = fixture();
    for decision in [
        LedgerGroupDecisionV1::RetainLive,
        LedgerGroupDecisionV1::RetainUnknown,
    ] {
        let mut changed = originals.clone();
        let mut observed: CollectionObservation = serde_json::from_slice(&changed[2]).unwrap();
        observed.groups[0].decision = decision;
        let mut evidence: LedgerGroupEvidenceV1 =
            serde_json::from_slice(&observed.evidence[0]).unwrap();
        evidence.decision = decision;
        observed.evidence[0] = encode(&evidence);
        changed[2] = encode(&observed);
        rebind_request(&mut changed);
        assert_eq!(
            CollectionEvidence::parse(&binding, &key, changed)
                .unwrap_err()
                .code,
            "ledger_collection_resource_retained"
        );
    }
}

#[test]
fn source_revision_format_identity_and_group_observation_must_match() {
    let (config, binding, key, originals) = fixture();
    let evidence = CollectionEvidence::parse(&binding, &key, originals).unwrap();
    let proof = GroupProof {
        key,
        source_sha256: "2".repeat(64),
        observation_sha256: evidence.observation_sha256().into(),
        records: 2,
    };
    evidence.validate_source(&config, &proof, 7).unwrap();
    for case in 0..5 {
        let mut config = config.clone();
        let mut proof = proof.clone();
        match case {
            0 => config.generation += 1,
            1 => config.sandbox_id.push('x'),
            2 => config.home_identity = Some("b".repeat(43)),
            3 => proof.key = "3".repeat(64),
            _ => proof.observation_sha256 = "4".repeat(64),
        }
        assert!(evidence.validate_source(&config, &proof, 7).is_err());
    }
    assert!(evidence.validate_source(&config, &proof, 8).is_err());
    let mut legacy = evidence;
    legacy.binding.source_format = LedgerSourceFormatV1::V1;
    assert!(legacy.validate_source(&config, &proof, 7).is_err());
}
