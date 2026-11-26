use super::*;
use layer2_core::billing_authority::AuthorityObservation;
use mongodb::bson::Bson;

#[test]
fn strict_pair_distinguishes_absence_from_partial_and_malformed_authority() {
    assert_eq!(
        format_v1::decode_pair(vec![]).unwrap(),
        AuthorityObservation::Absent
    );
    let rows = format_v1::proposed_documents();
    assert!(format_v1::decode_pair(rows.to_vec())
        .unwrap()
        .require_legacy_compatible()
        .is_ok());
    for row in &rows {
        assert!(format_v1::decode_pair(vec![row.clone()]).is_err());
        assert!(format_v1::decode_pair(vec![row.clone(), row.clone()]).is_err());
    }
    for index in 0..2 {
        for key in rows[index].keys() {
            let mut changed = rows.clone();
            changed[index].remove(key);
            assert!(
                format_v1::decode_pair(changed.to_vec()).is_err(),
                "missing {key}"
            );
            let mut changed = rows.clone();
            changed[index].insert(key, Bson::Null);
            assert!(
                format_v1::decode_pair(changed.to_vec()).is_err(),
                "null {key}"
            );
        }
        let mut changed = rows.clone();
        changed[index].insert("extra", true);
        assert!(format_v1::decode_pair(changed.to_vec()).is_err());
        for version in [
            Bson::Int64(1),
            Bson::Int64(3),
            Bson::Int32(2),
            Bson::Double(2.0),
            Bson::Array(vec![2_i64.into()]),
        ] {
            let mut changed = rows.clone();
            changed[index].insert("document_schema_version", version);
            assert!(format_v1::decode_pair(changed.to_vec()).is_err());
        }
    }
    for epoch in [
        Bson::Int64(-1),
        Bson::Int32(0),
        Bson::Double(0.0),
        Bson::String("0".into()),
        Bson::Array(vec![0_i64.into()]),
    ] {
        let mut changed = rows.clone();
        changed[1].insert("epoch", epoch);
        assert!(format_v1::decode_pair(changed.to_vec()).is_err());
    }
    for revision in [0_i64, -1] {
        let mut changed = rows.clone();
        changed[1].insert("revision", revision);
        assert!(format_v1::decode_pair(changed.to_vec()).is_err());
    }
    let mut changed = rows.clone();
    changed[1].insert("mode", "unknown");
    assert!(format_v1::decode_pair(changed.to_vec()).is_err());
}

#[test]
fn readable_active_or_rollback_epochs_do_not_authorize_legacy_execution() {
    for (epoch, mode) in [
        (0_i64, "nyxid"),
        (1, "nyxid"),
        (1, "legacy"),
        (i64::MAX, "legacy"),
    ] {
        let mut rows = format_v1::proposed_documents();
        rows[1].insert("epoch", epoch);
        rows[1].insert("mode", mode);
        assert!(format_v1::decode_pair(rows.to_vec())
            .unwrap()
            .require_legacy_compatible()
            .is_err());
    }
}
