use super::*;
use crate::install_state::InstallRecord;
use crate::persistent_home::Binding;
use layer3_adapter::{RestoreBootstrapReceipt, RestoreBootstrapRequest, RestoreRequest};

const OPERATION: &str = "inspection-inherited-installation";
const BINDING: &str = "home-binding/binding.json";
const INSTALLATION: &str = "runtime-state/install-v2.json";
const MATCHING: (&str, &str) = ("RECOGNIZED", "binding_matches");

fn checkpoint(content: &[u8]) -> RestoreRequest {
    let sha256 = sha256(content);
    let checkpoint = RestoreRequest {
        object_reference: format!("home-{sha256}.cma"),
        sha256,
    };
    assert!(checkpoint.has_valid_reference());
    checkpoint
}

fn inherited(combined: bool) -> Fixture {
    let fixture = Fixture::new();
    if combined {
        fixture.v2();
        fixture.retirement(3);
    }
    let request = RestoreBootstrapRequest::new(
        OPERATION,
        &fixture.config.sandbox_id,
        checkpoint(b"synthetic inherited home"),
    );
    assert!(request.has_valid_sha256());
    let restore_request_sha256 = request.checkpoint.computed_sha256();
    let receipt = RestoreBootstrapReceipt {
        operation_id: request.operation_id,
        request_sha256: request.request_sha256,
        restore_request_sha256: restore_request_sha256.clone(),
        installation_inherited: true,
    };
    fixture.write(
        io::TRANSFER,
        INSTALLATION,
        &serde_json::to_vec(&InstallRecord::inherited(&fixture.config, receipt)).unwrap(),
    );
    fixture.edit(io::STATE, BINDING, |value| {
        value["restore"] = json!({
            "request_sha256": restore_request_sha256,
            "completed": true,
        });
    });
    independently_valid(&fixture);
    fixture
}

fn independently_valid(fixture: &Fixture) {
    let binding = std::fs::read(fixture.roots[io::STATE].join(BINDING)).unwrap();
    Binding::decode_inventory(&binding, &fixture.config).unwrap();
    let record = std::fs::read(fixture.roots[io::TRANSFER].join(INSTALLATION)).unwrap();
    let record = serde_json::from_slice::<InstallRecord>(&record).unwrap();
    crate::install_state_validation::validate_record(&fixture.config, &record).unwrap();
}

fn check(fixture: &Fixture, combined: bool, binding: (&str, &str), installation: (&str, &str)) {
    let before = fixture.snapshot();
    let report = fixture.report();
    assert_eq!(before, fixture.snapshot());
    held(&report);
    for (component, (status, reason)) in [
        ("binding", binding),
        ("installation", installation),
        (
            "mutation",
            (
                "RECOGNIZED",
                if combined {
                    "mutation_v2_metadata"
                } else {
                    "mutation_v1_layout"
                },
            ),
        ),
        (
            "retirement",
            if combined {
                ("RECOGNIZED", "retirement_metadata_transition_unassessed")
            } else {
                ("ABSENT", "retirement_not_activated")
            },
        ),
    ] {
        assert_eq!(report[component]["status"], status, "{component}: {report}");
        assert_eq!(report[component]["reason"], reason, "{component}: {report}");
    }
    let encoded = serde_json::to_string(&report).unwrap();
    for private in [
        OPERATION,
        fixture.config.sandbox_id.as_str(),
        fixture.config.home_identity.as_deref().unwrap(),
        fixture.root.to_str().unwrap(),
        "/home/cma",
        "inspection-install",
        "user-content",
        "database contents unassessed",
        "invalid-operation",
        "invalid-bootstrap-digest",
        "invalid-restore-digest",
        "invalid-marker-digest",
        "unexpected-receipt-payload",
    ] {
        assert!(!encoded.contains(private), "assessment exposed source data");
    }
}

#[test]
fn completed_inherited_installation_is_recognized_in_both_layouts() {
    for combined in [false, true] {
        check(
            &inherited(combined),
            combined,
            MATCHING,
            ("RECOGNIZED", "installation_completed_metadata"),
        );
    }
}

#[test]
fn pending_matching_marker_never_completes_inherited_installation() {
    for combined in [false, true] {
        let fixture = inherited(combined);
        fixture.edit(io::STATE, BINDING, |value| {
            value["restore"]["completed"] = json!(false);
        });
        independently_valid(&fixture);
        check(
            &fixture,
            combined,
            MATCHING,
            ("INCONSISTENT", "installation_restore_pending"),
        );
    }
}

#[test]
fn independently_valid_restore_digests_must_match() {
    for combined in [false, true] {
        let fixture = inherited(combined);
        fixture.edit(io::STATE, BINDING, |value| {
            let other = checkpoint(b"another synthetic home").computed_sha256();
            assert_ne!(value["restore"]["request_sha256"], other);
            value["restore"]["request_sha256"] = json!(other);
        });
        independently_valid(&fixture);
        check(
            &fixture,
            combined,
            MATCHING,
            ("INCONSISTENT", "installation_binding_mismatch"),
        );
    }
}

#[test]
fn inherited_receipt_without_marker_stays_pending() {
    let fixture = inherited(false);
    fixture.edit(io::STATE, BINDING, |value| value["restore"] = Value::Null);
    independently_valid(&fixture);
    check(
        &fixture,
        false,
        MATCHING,
        ("INCONSISTENT", "installation_restore_pending"),
    );
}

#[test]
fn normal_completed_installation_cannot_supply_inherited_authority() {
    let fixture = inherited(false);
    fixture.install(false);
    independently_valid(&fixture);
    check(
        &fixture,
        false,
        MATCHING,
        ("INCONSISTENT", "installation_binding_mismatch"),
    );
}

fn invalid_receipt(field: &str, value: Value) {
    let fixture = inherited(false);
    fixture.edit(io::TRANSFER, INSTALLATION, |record| {
        record["receipt"][field] = value;
    });
    check(
        &fixture,
        false,
        MATCHING,
        ("INCONSISTENT", "installation_invalid"),
    );
}

#[test]
fn matching_marker_cannot_override_false_inherited_flag() {
    invalid_receipt("installation_inherited", json!(false));
}

#[test]
fn malformed_operation_is_invalid_and_not_exposed() {
    invalid_receipt("operation_id", json!("invalid-operation\n"));
}

#[test]
fn malformed_bootstrap_digest_is_invalid_and_not_exposed() {
    invalid_receipt("request_sha256", json!("invalid-bootstrap-digest"));
}

#[test]
fn malformed_restore_digest_is_invalid_and_not_exposed() {
    invalid_receipt("restore_request_sha256", json!("invalid-restore-digest"));
}

#[test]
fn unknown_receipt_field_is_invalid_and_not_exposed() {
    invalid_receipt("unknown", json!("unexpected-receipt-payload"));
}

#[test]
fn malformed_marker_invalidates_binding_before_installation_join() {
    let fixture = inherited(false);
    fixture.edit(io::STATE, BINDING, |value| {
        value["restore"]["request_sha256"] = json!("invalid-marker-digest");
    });
    check(
        &fixture,
        false,
        ("INCONSISTENT", "binding_invalid"),
        ("INCONSISTENT", "installation_binding_missing_or_invalid"),
    );
}
