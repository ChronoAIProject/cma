use super::*;
use mongodb::bson::{self, doc};

fn seal() -> Seal {
    let raw = [12, 0, 0, 0, 16, b'x', 0, 1, 0, 0, 0, 0];
    let incarnation = "0".repeat(64);
    let leaf = digest::leaf("agt_fixture", &incarnation, 1, &raw);
    let mut seal = Seal {
        id: Seal::identity("agt_fixture", &incarnation, 0),
        format_version: 1,
        agent_id: "agt_fixture".into(),
        incarnation,
        ordinal: 0,
        start_seq: 1,
        end_seq: 1,
        entry_count: 1,
        encoded_bytes: 12,
        previous_seal: None,
        prefix_count: 1,
        prefix_bytes: 12,
        leaf_digests: leaf,
        seal_digest: digest::binary(vec![]),
        digest_version: digest::VERSION.into(),
        sealing_operation: "fixture-operation".into(),
        source_format: "legacy_v1".into(),
        storage_kind: "mongo".into(),
    };
    seal.seal_digest = seal.digest();
    seal
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|n| format!("{n:02x}")).collect()
}

#[test]
fn raw_bson_and_manifest_have_independent_golden_digests() {
    let seal = seal();
    assert_eq!(
        hex(&seal.leaf_digests.bytes),
        "6e360a739f42e45beee7d249c73fee8d73e059aa2eee485c97c3f49a9860c0f3"
    );
    assert_eq!(
        hex(&seal.seal_digest.bytes),
        "11dc607aa2bf286a9f7e3a657d102ecd36c1ea15d53cb80bc866537a3a2f1289"
    );
    seal.validate().unwrap();
    let raw32 = bson::serialize_to_vec(&doc! {"x":1_i32}).unwrap();
    let raw64 = bson::serialize_to_vec(&doc! {"x":1_i64}).unwrap();
    assert_eq!(
        digest::leaf(&seal.agent_id, &seal.incarnation, 1, &raw32),
        seal.leaf_digests
    );
    assert_ne!(
        digest::leaf(&seal.agent_id, &seal.incarnation, 1, &raw64),
        seal.leaf_digests
    );
    assert_ne!(
        digest::leaf("other-agent", &seal.incarnation, 1, &raw32),
        seal.leaf_digests
    );
    assert_ne!(
        digest::leaf(&seal.agent_id, &seal.incarnation, 2, &raw32),
        seal.leaf_digests
    );
}

#[test]
fn maximum_manifest_is_below_one_mib_with_binary_leaves() {
    let mut seal = seal();
    seal.end_seq = MAX_ENTRIES;
    seal.entry_count = MAX_ENTRIES;
    seal.prefix_count = MAX_ENTRIES;
    seal.encoded_bytes = MAX_BYTES;
    seal.prefix_bytes = MAX_BYTES;
    seal.leaf_digests = digest::binary(vec![0; MAX_ENTRIES as usize * 32]);
    seal.sealing_operation = "x".repeat(512);
    seal.seal_digest = seal.digest();
    seal.validate().unwrap();
    let document = bson::serialize_to_document(&seal).unwrap();
    assert!(bson::serialize_to_vec(&document).unwrap().len() < MAX_SEAL_BYTES);
    assert_eq!(Seal::read(document).unwrap(), seal);
}

#[test]
fn manifests_reject_unknown_fields_gaps_wrong_counters_and_bad_hashes() {
    let base = seal();
    let mut missing = bson::serialize_to_document(&base).unwrap();
    missing.remove("previous_seal");
    assert!(Seal::read(missing).is_err());
    let mut document = bson::serialize_to_document(&base).unwrap();
    document.insert("future", true);
    assert!(Seal::read(document).is_err());
    for field in [
        "format_version",
        "start_seq",
        "end_seq",
        "entry_count",
        "encoded_bytes",
        "prefix_count",
        "prefix_bytes",
        "ordinal",
    ] {
        let mut document = bson::serialize_to_document(&base).unwrap();
        document.insert(field, -1_i64);
        assert!(Seal::read(document).is_err(), "{field}");
    }
    let mut changed = base.clone();
    changed.leaf_digests.bytes[0] ^= 1;
    assert!(changed.validate().is_err());
    let mut next = base.clone();
    next.ordinal = 1;
    next.start_seq = 2;
    next.end_seq = 2;
    next.prefix_count = 2;
    next.prefix_bytes = 24;
    next.previous_seal = Some(base.seal_digest.clone());
    next.id = Seal::identity(&next.agent_id, &next.incarnation, 1);
    next.seal_digest = next.digest();
    next.validate().unwrap();
    next.follows(&base).unwrap();
    next.start_seq = 3;
    assert!(next.follows(&base).is_err());
}

#[test]
fn envelope_is_an_old_reader_tripwire_without_changing_other_collections() {
    let mut document = doc! {"document_schema_version":1_i64};
    assert_eq!(read_format(&document).unwrap(), None);
    document.insert("agent_log_format", FORMAT);
    assert!(read_format(&document).is_err());
    document.insert("document_schema_version", 2_i64);
    document.insert("agent_log_incarnation", "0".repeat(64));
    document.insert("agent_log_head_revision", 1_i64);
    assert!(read_format(&document).unwrap().is_some());
    assert!(crate::store::sandbox_mongo::require_version(&document).is_err());
    for version in [
        bson::Bson::Int64(3),
        bson::Bson::String("2".into()),
        bson::Bson::Array(vec![2_i64.into()]),
    ] {
        document.insert("document_schema_version", version);
        assert!(read_format(&document).is_err());
    }
}

#[test]
fn head_refuses_unsafe_public_numbers_and_missing_projection_coverage() {
    let mut head = Head {
        agent_id: "agt_fixture".into(),
        format_version: 1,
        incarnation: "0".repeat(64),
        revision: 1,
        active_ordinal: 1,
        active_start_seq: MAX_SEQUENCE,
        last_seq: MAX_SEQUENCE,
        active_count: 1,
        active_bytes: 12,
        previous_seal: Some(digest::binary(vec![0; 32])),
        lifetime_count: MAX_SEQUENCE,
        lifetime_bytes: 100,
        projection_format: 1,
        evidence_generation: "initial".into(),
        projection_through: MAX_SEQUENCE,
        migration_id: MIGRATION.into(),
        required_reader: READER.into(),
    };
    head.validate().unwrap();
    head.projection_through -= 1;
    assert!(head.validate().is_err());
    head.last_seq += 1;
    head.projection_through = head.last_seq;
    head.lifetime_count = head.last_seq;
    head.active_count = 2;
    assert!(head.validate().is_err());
}

#[test]
fn projection_requires_every_nullable_reference_even_when_null() {
    use layer2_core::sandbox::AgentLogProjectionState;
    let record = bson::serialize_to_document(&AgentLogProjectionState::default()).unwrap();
    for (field, value) in &record {
        if value == &bson::Bson::Null {
            let mut missing = record.clone();
            missing.remove(field);
            assert!(
                bson::deserialize_from_document::<AgentLogProjectionState>(missing).is_err(),
                "{field}"
            );
        }
    }
    assert!(bson::deserialize_from_document::<AgentLogProjectionState>(record).is_ok());
}
