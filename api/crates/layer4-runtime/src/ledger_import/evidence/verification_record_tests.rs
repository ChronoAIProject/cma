use super::*;

#[test]
fn mutation_and_authority_shapes_cannot_be_mixed_or_given_unrecognized_states() {
    for (kind, key, value) in [
        ("Journal", "status", json!("unknown")),
        ("Journal", "status", Value::Null),
        ("Journal", "fence", json!("1")),
        ("Journal", "fence", Value::Null),
        ("Authority", "status", json!("started")),
        ("Authority", "status", Value::Null),
        ("Authority", "fence", json!("0")),
        ("Authority", "fence", json!("01")),
        ("Authority", "fence", json!(18446744073709551615u64)),
        ("Authority", "fence", json!("18446744073709551616")),
        ("Authority", "resource_sha256", json!("A".repeat(64))),
        ("Authority", "disposition", json!("maybe")),
        ("Authority", "type", json!("mutation")),
        ("Journal", "unknown", json!([])),
    ] {
        let mut artifact = Artifact::with_fixture(mutation_fixture());
        artifact.change(|p| {
            let record = p["records"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|r| r["kind"] == kind)
                .unwrap();
            record["fact"][key] = value;
        });
        assert!(artifact.verify().is_err(), "{kind}: {key}");
    }
}

#[test]
fn operation_identity_and_required_fact_fields_are_checked() {
    for (key, value) in [
        ("operation_id", json!("")),
        ("operation_id", json!("x".repeat(257))),
        ("request_sha256", json!("bad")),
        ("payload_sha256", json!(null)),
        ("subject_sha256", json!("0".repeat(63))),
        ("unknown", json!(true)),
    ] {
        let mut artifact = Artifact::with_fixture(mutation_fixture());
        artifact.change(|p| {
            let r = p["records"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|r| r["kind"] == "Journal")
                .unwrap();
            r["fact"]["operation"][key] = value;
        });
        assert!(artifact.verify().is_err(), "{key}");
    }
    for kind in ["Journal", "Authority"] {
        let mut artifact = Artifact::with_fixture(mutation_fixture());
        artifact.change(|p| {
            let r = p["records"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|r| r["kind"] == kind)
                .unwrap();
            r["fact"] = Value::Null;
        });
        assert!(artifact.verify().is_err());
    }
}

#[test]
fn unpublished_authority_and_partial_journal_observations_remain_admissible_as_bytes() {
    let fixture = mutation_fixture();
    fixture.write("mutation-authority/.authority-partial.tmp", b"unpublished");
    let artifact = Artifact::with_fixture(fixture);
    artifact.verify().unwrap();
    let payload = artifact.payload();
    assert!(payload["records"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["kind"] == "Authority" && r["unpublished"] == true && r["fact"].is_null()));
    assert!(payload["records"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["kind"] == "Journal" && r["fact"]["status"] == "started"));
}

#[test]
fn processing_a_self_consistent_forged_identity_never_authenticates_it() {
    let mut artifact = Artifact::new();
    artifact.change(|p| p["sandbox_id"] = json!("another-claimed-sandbox"));
    let receipt = serde_json::to_value(artifact.verify().unwrap()).unwrap();
    assert_eq!(receipt["source_provenance"], "UNVERIFIED");
    assert_eq!(receipt["restore_admission"], "HELD");
    assert_eq!(receipt["semantic_reconciliation"], "NOT_RUN");
    assert!(!receipt.to_string().contains("another-claimed-sandbox"));
}

#[test]
fn encoded_limits_admit_exactly_full_files_and_refuse_one_byte_more() {
    let mut artifact = Artifact::new();
    let mut payload = std::fs::read(artifact.path.join("evidence.json")).unwrap();
    payload.resize(16 * 1024 * 1024, b' ');
    artifact.replace_payload(&payload);
    let seal_path = artifact.path.join("sealed.json");
    let mut seal = std::fs::read(&seal_path).unwrap();
    seal.resize(4096, b' ');
    write(&seal_path, &seal);
    artifact.hash = hash(&seal);
    artifact.verify().unwrap();
    seal.push(b' ');
    write(&seal_path, &seal);
    artifact.hash = hash(&seal);
    assert!(artifact.verify().is_err());
}
