//! Explicit additive DDL. Startup only verifies; partial execution is restartable.
use futures_util::TryStreamExt;
use layer2_core::StoreError;
use mongodb::{
    bson::{doc, Document},
    options::IndexOptions,
    Database, IndexModel,
};

use super::{AUDITS, COLLECTIONS, EVIDENCE, HEADS, PROJECTIONS, PUBLICATIONS, SEGMENTS};

fn failed() -> StoreError {
    StoreError::Malformed("Agent log segment DDL is absent, partial or incompatible; run the reviewed additive migration".into())
}

fn unavailable(error: mongodb::error::Error) -> StoreError {
    StoreError::Unavailable(format!("Agent log segment schema: {error}"))
}

fn indexes(name: &str) -> Vec<IndexModel> {
    let fields = match name {
        SEGMENTS => vec![
            (
                "agent_log_segment_ordinal",
                doc! {"agent_id":1,"incarnation":1,"ordinal":1},
            ),
            (
                "agent_log_segment_start",
                doc! {"agent_id":1,"incarnation":1,"start_seq":1},
            ),
        ],
        PUBLICATIONS => vec![(
            "agent_log_publication_key",
            doc! {"agent_id":1,"incarnation":1,"publication_key":1},
        )],
        EVIDENCE => vec![
            (
                "agent_log_evidence_key",
                doc! {"agent_id":1,"incarnation":1,"evidence_generation":1,"kind":1,"entity_key":1},
            ),
            (
                "agent_log_evidence_turn",
                doc! {"agent_id":1,"incarnation":1,"evidence_generation":1,"kind":1,"record.facts.turn_sequence":1,"entity_key":1},
            ),
            (
                "agent_log_evidence_approval",
                doc! {"agent_id":1,"incarnation":1,"evidence_generation":1,"kind":1,"record.epoch":1,"record.request_sequence":1,"entity_key":1},
            ),
        ],
        AUDITS => vec![
            ("agent_log_audit_key", doc! {"agent_id":1,"audit_id":1}),
            (
                "agent_log_audit_status",
                doc! {"agent_id":1,"incarnation":1,"status":1,"audit_id":1},
            ),
        ],
        _ => vec![],
    };
    fields
        .into_iter()
        .map(|(name, keys)| {
            IndexModel::builder()
                .keys(keys)
                .options(
                    IndexOptions::builder()
                        .name(name.to_string())
                        .unique(true)
                        .build(),
                )
                .build()
        })
        .collect()
}

fn validator(name: &str) -> Document {
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
            &["agent_id", "incarnation", "publication_key"],
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
            &["agent_id", "incarnation", "audit_id", "holder", "status"],
            &[
                "fence",
                "expires_at_ms",
                "through_seq",
                "head_revision",
                "next_ordinal",
                "checked_entries",
                "checked_bytes",
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
        properties.insert("semantic", doc! {"bsonType":"object"});
        properties.insert("previous_seal", doc! {"bsonType":["null","binData"]});
    }
    let required: Vec<_> = properties.keys().cloned().collect();
    doc! {"$jsonSchema":{"bsonType":"object","required":required,"additionalProperties":false,"properties":properties}}
}

pub(crate) async fn migrate(database: &Database) -> Result<(), StoreError> {
    let existing = database
        .list_collection_names()
        .await
        .map_err(unavailable)?;
    for name in COLLECTIONS {
        if !existing.iter().any(|n| n == name) {
            database
                .create_collection(*name)
                .validator(validator(name))
                .validation_level(mongodb::options::ValidationLevel::Strict)
                .validation_action(mongodb::options::ValidationAction::Error)
                .await
                .map_err(unavailable)?;
        }
        for index in indexes(name) {
            database
                .collection::<Document>(name)
                .create_index(index)
                .await
                .map_err(unavailable)?;
        }
    }
    verify(database).await
}

pub(in crate::store) async fn verify(database: &Database) -> Result<(), StoreError> {
    let existing = database
        .list_collection_names()
        .await
        .map_err(unavailable)?;
    let present = COLLECTIONS
        .iter()
        .filter(|name| existing.iter().any(|n| n == **name))
        .count();
    if present == 0 {
        // Readers may ship before additive DDL. No activated Agent can do so.
        if database
            .collection::<Document>("agents")
            .find_one(doc! {"document_schema_version":2_i64})
            .projection(doc! {"_id":1})
            .await
            .map_err(unavailable)?
            .is_none()
        {
            return Ok(());
        }
        return Err(failed());
    }
    if present != COLLECTIONS.len() {
        return Err(failed());
    }
    for name in COLLECTIONS {
        let mut specs = database
            .list_collections()
            .filter(doc! {"name":*name})
            .await
            .map_err(unavailable)?;
        let spec = specs
            .try_next()
            .await
            .map_err(unavailable)?
            .ok_or_else(failed)?;
        if spec.options.validator != Some(validator(name))
            || spec.options.validation_level != Some(mongodb::options::ValidationLevel::Strict)
            || spec.options.validation_action != Some(mongodb::options::ValidationAction::Error)
        {
            return Err(failed());
        }
        let collection = database.collection::<Document>(name);
        let actual: Vec<_> = collection
            .list_indexes()
            .await
            .map_err(unavailable)?
            .try_collect()
            .await
            .map_err(unavailable)?;
        if actual
            .iter()
            .any(|i| i.options.as_ref().is_some_and(|o| o.expire_after.is_some()))
        {
            return Err(failed());
        }
        for expected in indexes(name) {
            if !actual.iter().any(|index| {
                index.keys == expected.keys
                    && index.options.as_ref().is_some_and(|o| {
                        o.unique == Some(true)
                            && o.name == expected.options.as_ref().and_then(|o| o.name.clone())
                            && o.partial_filter_expression.is_none()
                            && o.sparse != Some(true)
                            && o.collation.is_none()
                    })
            }) {
                return Err(failed());
            }
        }
    }
    Ok(())
}
