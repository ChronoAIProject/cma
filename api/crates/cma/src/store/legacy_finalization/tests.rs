use super::*;
use mongodb::bson::{doc, Bson};
use serde_json::json;

fn fixture() -> (Document, Sandbox) {
    let mut sandbox = crate::store::sandbox_mongo_integration_tests::sandbox("sbx_retained");
    sandbox.status = SandboxStatus::Retired;
    sandbox.credential.current = None;
    sandbox.retired_at = Some(sandbox.created_at.clone());
    sandbox.work.step = SandboxWorkStep::None;
    let history = json!({
        "schema_version":1,
        "policy":{"schema_version":1,"allocation_id":"allocation", "allocation_sha256":"a".repeat(64),"disposition":"retain","pinned_at_ms":1_700_000_000_000_i64},
        "sandbox_id":sandbox.id.as_str(),"sandbox_created_at":sandbox.created_at,
        "tree_fingerprint":"a".repeat(64),"storage_retention_sha256":"b".repeat(64),
        "reference_count_at_seal":1,"references_sha256_at_seal":"c".repeat(64),
        "reference_ids_at_seal":["chr_retained"],
        "material":{"agent_conversation_sha256":"d".repeat(64),"log_count":0,"log_sha256":"e".repeat(64),"commands":[],"outbox":[],"ancillary":[]},
        "sealed_at":sandbox.retired_at
    });
    let receipt = json!({
        "schema_version":1,"allocation_id":"allocation","allocation_operation_id":"operation",
        "sandbox_sha256":content_digest(&("channel-retirement-finalization-v1",&sandbox)).unwrap(),
        "provider_finality_sha256":"a".repeat(64),"retirement_state_sha256":"b".repeat(64),
        "storage_audit_id":"audit","storage_audit_sha256":"c".repeat(64),
        "history":history,"retired_at":sandbox.retired_at
    });
    let document = doc! {
        "channel_retirement_finalization":mongodb::bson::serialize_to_bson(&receipt).unwrap(),
        "channel_retirement_finalization_sha256":content_digest(&("channel-retirement-finalization-v1",&receipt)).unwrap(),
        "channel_history_seal":content_digest(&("channel-retained-history-seal-v1",&history)).unwrap(),
    };
    (document, sandbox)
}

#[test]
fn historical_receipt_round_trip_retains_canonical_digests() {
    let (document, sandbox) = fixture();
    assert_eq!(validate_document(&document, &sandbox), Ok(()));
    let mut with_empty_controls = document.clone();
    with_empty_controls
        .get_document_mut("channel_retirement_finalization")
        .unwrap()
        .get_document_mut("history")
        .unwrap()
        .get_document_mut("material")
        .unwrap()
        .insert("controls", Bson::Array(vec![]));
    // Earlier readers omitted empty controls when checking the stored digest.
    assert_eq!(validate_document(&with_empty_controls, &sandbox), Ok(()));
}

#[test]
fn corrupt_or_unsealed_retirement_never_becomes_an_ordinary_sandbox() {
    let (document, sandbox) = fixture();
    for key in [
        "channel_retirement_finalization",
        "channel_retirement_finalization_sha256",
        "channel_history_seal",
    ] {
        let mut broken = document.clone();
        broken.remove(key);
        assert_eq!(validate_document(&broken, &sandbox), Err(()), "{key}");
    }
    let mut wrong_state = sandbox.clone();
    wrong_state.status = SandboxStatus::Active;
    assert_eq!(validate_document(&document, &wrong_state), Err(()));
    let mut unknown = document.clone();
    unknown
        .get_document_mut("channel_retirement_finalization")
        .unwrap()
        .get_document_mut("history")
        .unwrap()
        .insert("new_authority", true);
    assert_eq!(validate_document(&unknown, &sandbox), Err(()));
    assert_eq!(validate_document(&Document::new(), &wrong_state), Ok(()));
}
