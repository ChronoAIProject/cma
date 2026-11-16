use super::*;

#[test]
fn snapshot_projection_accepts_matching_canonical_subject_hashes() {
    let hash = "0123456789abcdef".repeat(4);
    let row = doc! {"subject_hash": &hash, "active_subject_hash": &hash};
    assert_eq!(records::active_hash(&row).unwrap(), hash);
}

#[test]
fn snapshot_projection_rejects_mismatched_active_hash() {
    let row = doc! {"subject_hash": "a".repeat(64), "active_subject_hash": "e".repeat(64)};
    assert!(records::active_hash(&row).is_err());
}

#[test]
fn snapshot_projection_rejects_missing_identity_fields() {
    for row in [
        doc! {},
        doc! {"subject_hash": "a".repeat(64)},
        doc! {"active_subject_hash": "a".repeat(64)},
    ] {
        assert!(records::active_hash(&row).is_err());
    }
}

#[test]
fn snapshot_projection_rejects_noncanonical_hashes() {
    for hash in [
        "A".repeat(64),
        "g".repeat(64),
        "a".repeat(63),
        "a".repeat(65),
        String::new(),
    ] {
        let row = doc! {"subject_hash": &hash, "active_subject_hash": &hash};
        assert!(records::active_hash(&row).is_err(), "{hash}");
    }
}

#[test]
fn snapshot_projection_rejects_non_string_identities() {
    for row in [
        doc! {"subject_hash": 1, "active_subject_hash": "a".repeat(64)},
        doc! {"subject_hash": "a".repeat(64), "active_subject_hash": 1},
        doc! {"subject_hash": "a".repeat(64), "active_subject_hash": null},
        doc! {"subject_hash": "a".repeat(64), "active_subject_hash": ["a".repeat(64)]},
    ] {
        assert!(records::active_hash(&row).is_err());
    }
}
