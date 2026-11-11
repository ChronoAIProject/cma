use super::*;

#[test]
fn progress_schema_refuses_unknown_fields_invalid_identity_and_unreconciled_counts() {
    let valid = Control::initial().document().unwrap();
    Control::decode(valid.clone()).unwrap();
    for (field, value) in [
        ("protocol", mongodb::bson::Bson::Int64(2)),
        ("document_schema_version", mongodb::bson::Bson::Int64(2)),
        ("revision", mongodb::bson::Bson::Int64(-1)),
        ("fence", mongodb::bson::Bson::Int64(-1)),
        ("run", mongodb::bson::Bson::String("invalid".into())),
        ("future_contract", mongodb::bson::Bson::Boolean(true)),
    ] {
        let mut row = valid.clone();
        row.insert(field, value);
        assert!(Control::decode(row).is_err(), "accepted {field}");
    }
    for values in [
        doc! {"scanned":1_i64,"recorded":0_i64,"retained":0_i64,"skipped":0_i64,"errors":0_i64},
        doc! {"scanned":0_i64,"recorded":-1_i64,"retained":1_i64,"skipped":0_i64,"errors":0_i64},
    ] {
        let mut row = valid.clone();
        row.insert("counts", values);
        assert!(Control::decode(row).is_err());
    }
}

#[test]
fn counters_partition_observations_and_refuse_overflow() {
    let mut counts = Counts::default();
    for outcome in ["recorded", "retained", "missing", "corrupt"] {
        counts.observe(outcome).unwrap();
        assert!(counts.valid());
    }
    assert_eq!(counts.scanned, 4);
    assert!(counts.observe("unknown").is_err());
    let mut full = Counts {
        scanned: i64::MAX,
        recorded: i64::MAX,
        ..Counts::default()
    };
    assert!(full.observe("recorded").is_err());
}
