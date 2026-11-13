use super::*;

pub(super) fn validator(name: &str) -> Document {
    if name == super::super::RECOVERY_WORK {
        return super::recovery::validator();
    }
    if name == super::super::writer_gate::COLLECTION {
        return controls::gate();
    }
    if name == super::super::writer_admission::COLLECTION {
        return controls::admission();
    }
    let mut properties =
        doc! {"_id":{"bsonType":"string"},"format_version":{"bsonType":"long","enum":[1_i64]}};
    let (strings, numbers, binaries): (&[&str], &[&str], &[&str]) = match name {
        HEADS => (
            &[
                "incarnation",
                "migration_id",
                "required_reader",
                "evidence_generation",
            ],
            &[
                "revision",
                "active_ordinal",
                "active_start_seq",
                "last_seq",
                "active_count",
                "active_bytes",
                "lifetime_count",
                "lifetime_bytes",
                "projection_format",
                "projection_through",
            ],
            &[],
        ),
        SEGMENTS => (
            &[
                "agent_id",
                "incarnation",
                "digest_version",
                "sealing_operation",
                "source_format",
                "storage_kind",
            ],
            &[
                "ordinal",
                "start_seq",
                "end_seq",
                "entry_count",
                "encoded_bytes",
                "prefix_count",
                "prefix_bytes",
            ],
            &["leaf_digests", "seal_digest"],
        ),
        PUBLICATIONS => (
            &["agent_id", "incarnation", "publication_key", "appended_at"],
            &[
                "expected_revision",
                "expected_seq",
                "first_seq",
                "last_seq",
                "result_revision",
                "head_revision",
                "active_ordinal",
            ],
            &["fingerprint", "result_digest"],
        ),
        PROJECTIONS => (
            &["incarnation", "evidence_generation"],
            &["through_seq", "head_revision"],
            &[],
        ),
        EVIDENCE => (
            &[
                "agent_id",
                "incarnation",
                "kind",
                "entity_key",
                "evidence_generation",
            ],
            &["through_seq", "head_revision"],
            &[],
        ),
        AUDITS => (
            &[
                "agent_id",
                "incarnation",
                "audit_id",
                "holder",
                "status",
                "result_generation",
            ],
            &[
                "fence",
                "expires_at_ms",
                "through_seq",
                "head_revision",
                "step",
            ],
            &[],
        ),
        _ => unreachable!("fixed segment collection inventory"),
    };
    for field in strings {
        properties.insert(*field, doc! {"bsonType":"string"});
    }
    for field in numbers {
        properties.insert(*field, doc! {"bsonType":"long","minimum":0_i64});
    }
    for field in binaries {
        properties.insert(*field, doc! {"bsonType":"binData"});
    }
    if matches!(name, HEADS | SEGMENTS | PUBLICATIONS) {
        properties.insert("previous_seal", doc! {"bsonType":["null","binData"]});
    }
    if matches!(name, PROJECTIONS | EVIDENCE) {
        properties.insert("record", doc! {"bsonType":"object"});
    }
    if name == AUDITS {
        properties.insert("captured_head", doc! {"bsonType":"object"});
        properties.insert("captured_agent", doc! {"bsonType":"object"});
        properties.insert("format_version", doc! {"bsonType":"long","enum":[2_i64]});
        properties.insert("completed", doc! {"bsonType":"object"});
        properties.insert("stage", doc! {"bsonType":["null","object"]});
    }
    let required: Vec<_> = properties.keys().cloned().collect();
    if name == PUBLICATIONS {
        properties.insert(
            "format_version",
            doc! {"bsonType":"long","enum":[1_i64,2_i64]},
        );
        properties.insert("agent_digest", doc! {"bsonType":"binData"});
    }
    if name == EVIDENCE {
        properties.insert(
            "turn_sequence",
            doc! {"bsonType":"long","minimum":1_i64,"maximum":super::super::MAX_SEQUENCE},
        );
        properties.insert(
            "change_ordinal",
            doc! {"bsonType":"long","minimum":0_i64,"maximum":super::super::MAX_SEQUENCE},
        );
    }
    let mut result = doc! {"$jsonSchema":{"bsonType":"object","required":required,"additionalProperties":false,"properties":properties}};
    if name == PUBLICATIONS {
        result.insert(
            "$or",
            vec![
                doc! {"format_version":1_i64,"agent_digest":{"$exists":false}},
                doc! {"format_version":2_i64,"agent_digest":{"$exists":true}},
            ],
        );
    }
    if name == EVIDENCE {
        result.insert(
            "$or",
            vec![
                doc! {"kind":{"$ne":"output_change"},
                "turn_sequence":{"$exists":false},"change_ordinal":{"$exists":false}},
                doc! {"kind":"output_change","turn_sequence":{"$exists":true},
                "change_ordinal":{"$exists":true},"record.type":"output_change",
                "record.change.turn_sequence":{"$type":"long"},
                "record.change.ordinal":{"$type":"long"},
                "$expr":{"$and":[
                    {"$eq":["$turn_sequence","$record.change.turn_sequence"]},
                    {"$eq":["$change_ordinal","$record.change.ordinal"]}
                ]}},
            ],
        );
    }
    result
}
