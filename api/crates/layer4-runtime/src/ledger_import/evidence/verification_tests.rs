use super::*;
use crate::ledger_import::evidence::{self, verification};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

#[path = "verification_io_tests.rs"]
mod io;
#[path = "verification_record_tests.rs"]
mod records;

struct Artifact {
    fixture: Fixture,
    path: PathBuf,
    hash: String,
}

impl Artifact {
    fn new() -> Self {
        Self::with_fixture(semantic::fixture())
    }
    fn with_fixture(fixture: Fixture) -> Self {
        let path = fixture.root.join("evidence");
        let receipt = evidence::create(
            &fixture.config,
            &path,
            &mut Budget::new(Limits::default()),
            16 * 1024 * 1024,
            |_: &File| Ok(()),
            |_| Ok(()),
        )
        .unwrap();
        let receipt = serde_json::to_value(receipt).unwrap();
        Self {
            fixture,
            path,
            hash: receipt["sealed_sha256"].as_str().unwrap().into(),
        }
    }
    fn verify(&self) -> Result<verification::Receipt> {
        verification::verify(
            &self.path,
            &self.hash,
            &mut Budget::new(limits()),
            || Ok(()),
        )
    }
    fn payload(&self) -> Value {
        serde_json::from_slice(&std::fs::read(self.path.join("evidence.json")).unwrap()).unwrap()
    }
    fn change(&mut self, change: impl FnOnce(&mut Value)) {
        let mut payload = self.payload();
        change(&mut payload);
        self.replace_payload(&serde_json::to_vec(&payload).unwrap());
    }
    fn replace_payload(&mut self, bytes: &[u8]) {
        write(&self.path.join("evidence.json"), bytes);
        let mut seal: Value =
            serde_json::from_slice(&std::fs::read(self.path.join("sealed.json")).unwrap()).unwrap();
        seal["payload_bytes"] = json!(bytes.len());
        seal["payload_sha256"] = json!(hash(bytes));
        let seal = serde_json::to_vec(&seal).unwrap();
        write(&self.path.join("sealed.json"), &seal);
        self.hash = hash(&seal);
    }
    fn snapshot(&self) -> Vec<(String, Vec<u8>, u32, u64)> {
        ["evidence.json", "sealed.json"]
            .into_iter()
            .map(|name| {
                let path = self.path.join(name);
                let m = std::fs::metadata(&path).unwrap();
                (name.into(), std::fs::read(path).unwrap(), m.mode(), m.ino())
            })
            .collect()
    }
}

fn limits() -> Limits {
    Limits {
        entries: 8,
        bytes: 2 * (16 * 1024 * 1024 + 4096),
        metadata_bytes: 16 * 1024,
        deadline: Duration::from_secs(30),
    }
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o400)).unwrap();
}

fn hash(bytes: &[u8]) -> String {
    crate::command::hex(&Sha256::digest(bytes))
}

fn mutation_fixture() -> Fixture {
    let fixture = semantic::fixture();
    semantic::journal(
        &fixture.config,
        &crate::mutation_journal::Record::started(&fixture.config, &semantic::add()).unwrap(),
    );
    semantic::authority(&fixture.config, u64::MAX, &semantic::add());
    fixture
}

#[test]
fn real_export_round_trip_preserves_artifacts_and_keeps_all_authority_unverified() {
    let artifact = Artifact::with_fixture(mutation_fixture());
    let before = artifact.snapshot();
    let receipt = serde_json::to_value(artifact.verify().unwrap()).unwrap();
    assert_eq!(receipt["artifact_integrity"], "VALIDATED");
    assert_eq!(receipt["source_provenance"], "UNVERIFIED");
    assert_eq!(receipt["producer_acknowledgement"], "UNVERIFIED");
    assert_eq!(receipt["restore_admission"], "HELD");
    assert_eq!(receipt["semantic_reconciliation"], "NOT_RUN");
    assert_eq!(
        receipt["source_records"],
        artifact.payload()["source_records"]
    );
    assert_eq!(receipt["sealed_sha256"], artifact.hash);
    assert_eq!(
        receipt["payload_sha256"],
        hash(&std::fs::read(artifact.path.join("evidence.json")).unwrap())
    );
    let safe = receipt.to_string();
    for private in [
        &artifact.fixture.config.sandbox_id,
        artifact.fixture.config.home_identity.as_ref().unwrap(),
        &semantic::add().operation_id,
    ] {
        assert!(!safe.contains(private));
    }
    assert!(!safe.contains(artifact.fixture.root.to_str().unwrap()));
    assert!(safe.len() < 4096);
    assert_eq!(artifact.snapshot(), before);
}

#[test]
fn expected_seal_digest_is_required_even_for_otherwise_valid_artifacts() {
    let artifact = Artifact::new();
    for expected in ["", "A", &"A".repeat(64), &"f".repeat(64)] {
        assert!(
            verification::verify(&artifact.path, expected, &mut Budget::new(limits()), || Ok(
                ()
            ))
            .is_err()
        );
    }
    artifact.verify().unwrap();
}

#[test]
fn altered_payload_requires_the_sealed_length_and_digest() {
    for changed in [b" ".as_slice(), b"\n".as_slice()] {
        let artifact = Artifact::new();
        let path = artifact.path.join("evidence.json");
        let mut bytes = std::fs::read(&path).unwrap();
        if changed == b" " {
            bytes.push(b' ');
        } else {
            *bytes.last_mut().unwrap() = b' ';
        }
        write(&path, &bytes);
        let before = artifact.snapshot();
        assert!(artifact.verify().is_err());
        assert_eq!(artifact.snapshot(), before);
    }
}

#[test]
fn seal_shape_counts_and_inventory_binding_are_checked_even_with_a_matching_pin() {
    for (key, value) in [
        ("format", json!("future")),
        ("restore_admission", json!("READY")),
        ("semantic_reconciliation", json!("COMPLETE")),
        ("payload_bytes", json!(0)),
        ("payload_sha256", json!("f".repeat(64))),
        ("source_records", json!(123)),
        ("inventory_sha256", json!("f".repeat(64))),
        ("unexpected", json!(true)),
    ] {
        let mut artifact = Artifact::new();
        let path = artifact.path.join("sealed.json");
        let mut seal: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        seal[key] = value;
        let bytes = serde_json::to_vec(&seal).unwrap();
        write(&path, &bytes);
        artifact.hash = hash(&bytes);
        assert!(artifact.verify().is_err(), "{key}");
    }
}

#[test]
fn resealed_payload_cannot_claim_complete_or_trusted_domains_outside_its_format() {
    for (key, value) in [
        ("format", json!("future")),
        ("scope", json!("full_home")),
        ("enumeration_complete", json!(false)),
        ("restore_admission", json!("READY")),
        ("semantic_reconciliation", json!("COMPLETE")),
        ("native_command_correlation", json!("KNOWN")),
        ("native_session_inventory", json!("VALIDATED")),
        ("runtime_output_boundary", json!("KNOWN")),
        ("retirement_inventory", json!("VALIDATED")),
        ("full_home_inventory", json!("VALIDATED")),
        ("provider_identity", json!("VERIFIED")),
        ("writer_exclusion", json!("VERIFIED")),
        ("external_immutability", json!("VERIFIED")),
        ("mutation_histories", json!("UNKNOWN")),
        ("auxiliary_recovery", json!("QUALIFIED")),
        ("source_records", json!(123)),
        ("source_directories", json!(0)),
        ("sandbox_id", json!(" ")),
        ("home_identity", json!("invalid")),
        ("generation", json!(7)),
        ("inventory_sha256", json!("A".repeat(64))),
        ("unknown", json!(true)),
    ] {
        let mut artifact = Artifact::new();
        artifact.change(|p| p[key] = value);
        let before = artifact.snapshot();
        assert!(artifact.verify().is_err(), "{key}");
        assert_eq!(artifact.snapshot(), before);
    }
}

#[test]
fn missing_duplicate_and_trailing_json_fields_are_refused() {
    for key in [
        "home_identity",
        "generation",
        "records",
        "enumeration_complete",
    ] {
        let mut artifact = Artifact::new();
        artifact.change(|p| {
            p.as_object_mut().unwrap().remove(key);
        });
        assert!(artifact.verify().is_err(), "{key}");
    }
    for suffix in [",\"generation\":\"1\"}", "}{}", "} trailing"] {
        let mut artifact = Artifact::new();
        let mut encoded = serde_json::to_string(&artifact.payload()).unwrap();
        encoded.pop();
        encoded.push_str(suffix);
        artifact.replace_payload(encoded.as_bytes());
        assert!(artifact.verify().is_err(), "{suffix}");
    }
    let mut artifact = Artifact::new();
    artifact.change(|p| {
        p["records"][0].as_object_mut().unwrap().remove("fact");
    });
    assert!(artifact.verify().is_err());
}

#[test]
fn canonical_decimal_identities_include_u64_max_and_reject_lossy_alternatives() {
    let mut fixture = semantic::fixture();
    fixture.config.generation = u64::MAX;
    let binding = fixture.config.state_root.join("home-binding/binding.json");
    let mut value: Value = serde_json::from_slice(&std::fs::read(&binding).unwrap()).unwrap();
    value["runtime_generation"] = json!(u64::MAX);
    std::fs::write(binding, serde_json::to_vec(&value).unwrap()).unwrap();
    let mut artifact = Artifact::with_fixture(fixture);
    artifact.verify().unwrap();
    for value in ["0", "01", "+1", "18446744073709551616", "1.0", "1e0", "-1"] {
        artifact.change(|p| p["generation"] = json!(value));
        assert!(artifact.verify().is_err(), "{value}");
    }
}

#[test]
fn record_references_kinds_and_unpublished_facts_are_strict() {
    for (key, value) in [
        ("source_index", json!(1)),
        ("source_sha256", json!("bad")),
        ("source_bytes", json!(u64::MAX)),
        ("kind", json!("V2")),
        (
            "scope",
            json!({"kind":"process_generation","generation":"0"}),
        ),
        ("scope", json!({"kind":"legacy_global","generation":"7"})),
        ("unpublished", json!("true")),
        ("unknown", json!(1)),
    ] {
        let mut artifact = Artifact::new();
        artifact.change(|p| p["records"][0][key] = value);
        assert!(artifact.verify().is_err(), "{key}");
    }
    let mut artifact = Artifact::with_fixture(mutation_fixture());
    artifact.change(|p| {
        let records = p["records"].as_array_mut().unwrap();
        let authority = records
            .iter_mut()
            .find(|r| r["kind"] == "Authority")
            .unwrap();
        authority["unpublished"] = json!(true);
    });
    assert!(artifact.verify().is_err());
}

#[test]
fn duplicate_physical_references_do_not_become_an_extra_valid_record() {
    let mut artifact = Artifact::new();
    artifact.change(|p| {
        let copy = p["records"][0].clone();
        p["records"].as_array_mut().unwrap().push(copy);
        p["source_records"] = json!(p["records"].as_array().unwrap().len());
    });
    let path = artifact.path.join("sealed.json");
    let mut seal: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    seal["source_records"] = artifact.payload()["source_records"].clone();
    let bytes = serde_json::to_vec(&seal).unwrap();
    write(&path, &bytes);
    artifact.hash = hash(&bytes);
    assert!(artifact.verify().is_err());
}
