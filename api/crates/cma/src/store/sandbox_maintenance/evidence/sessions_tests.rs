use super::*;
use mongodb::bson::DateTime;

const DEADLINE: &str = "2026-09-05T00:30:00Z";

#[test]
fn exclusive_expiry_uses_both_clocks_and_checks_timestamp_agreement() {
    let expiry = stamp_ms(DEADLINE).unwrap();
    let sandbox = SandboxId("sbx_synthetic".into());
    let mut row = doc! {
        "_id": "desktop", "sandbox_id": sandbox.as_str(),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "lease_expires_at": DEADLINE, "epoch": 1_i64, "runtime_id": "synthetic",
        "boot_id": Bson::Null, "issued_at_ms": expiry - 15_000,
        "expires_at_ms": expiry, "database_now": DateTime::from_millis(expiry - 1),
    };
    assert_eq!(expired(&row, &sandbox, expiry + 30_000), Some(false));
    row.insert("database_now", DateTime::from_millis(expiry));
    assert_eq!(expired(&row, &sandbox, expiry - 1), Some(false));
    assert_eq!(expired(&row, &sandbox, expiry), Some(true));

    for (field, value) in [
        ("issued_at_ms", Bson::Int64(expiry - 15_001)),
        ("expires_at_ms", Bson::Int64(expiry + 1000)),
        (
            "document_schema_version",
            Bson::Int64(DOCUMENT_SCHEMA_VERSION + 1),
        ),
        ("database_now", Bson::Int64(expiry)),
    ] {
        let mut invalid = row.clone();
        invalid.insert(field, value);
        assert_eq!(expired(&invalid, &sandbox, expiry), None, "{field}");
    }
    row.remove("issued_at_ms");
    assert_eq!(expired(&row, &sandbox, expiry), None);
}
