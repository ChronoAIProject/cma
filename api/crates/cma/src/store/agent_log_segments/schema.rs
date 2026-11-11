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
                "agent_log_evidence_generation",
                doc! {"agent_id":1,"incarnation":1,"evidence_generation":1,"_id":1},
            ),
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
    doc! {"$jsonSchema":{"bsonType":"object","required":required,"additionalProperties":false,"properties":properties}}
}

#[cfg(test)]
pub(in crate::store) async fn migrate(database: &Database) -> Result<(), StoreError> {
    while apply_next(database).await? {}
    verify(database).await
}

/// Startup verifies only. An entirely absent inventory is supported before
/// activation; partial inventory never authorizes fallback or startup DDL.
pub(in crate::store) async fn verify(database: &Database) -> Result<(), StoreError> {
    let names = database
        .list_collection_names()
        .await
        .map_err(unavailable)?;
    let present = COLLECTIONS
        .iter()
        .filter(|name| names.iter().any(|n| n == **name))
        .count();
    if present == 0 {
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
    if present != COLLECTIONS.len() || !pending(database).await?.is_empty() {
        return Err(failed());
    }
    Ok(())
}

enum Step {
    Collection(&'static str),
    Index(&'static str, Box<IndexModel>),
}

/// Inspect the existing contract before any additive operation. Partial
/// progress must not conceal incompatible earlier DDL.
async fn pending(database: &Database) -> Result<Vec<Step>, StoreError> {
    let mut cursor = database.list_collections().await.map_err(unavailable)?;
    let mut actual = std::collections::HashMap::new();
    while let Some(spec) = cursor.try_next().await.map_err(unavailable)? {
        if COLLECTIONS.contains(&spec.name.as_str()) {
            actual.insert(spec.name.clone(), spec);
        }
    }
    let mut pending = Vec::new();
    for name in COLLECTIONS {
        let Some(spec) = actual.get(*name) else {
            pending.push(Step::Collection(name));
            pending.extend(
                indexes(name)
                    .into_iter()
                    .map(|index| Step::Index(name, Box::new(index))),
            );
            continue;
        };
        if spec.options.validator != Some(validator(name))
            || spec.options.validation_level != Some(mongodb::options::ValidationLevel::Strict)
            || spec.options.validation_action != Some(mongodb::options::ValidationAction::Error)
            || spec.options.collation.is_some()
            || spec.options.capped == Some(true)
        {
            return Err(failed());
        }
        let actual: Vec<_> = database
            .collection::<Document>(name)
            .list_indexes()
            .await
            .map_err(unavailable)?
            .try_collect()
            .await
            .map_err(unavailable)?;
        if actual.iter().any(|index| {
            index
                .options
                .as_ref()
                .is_some_and(|o| o.expire_after.is_some())
        }) {
            return Err(failed());
        }
        for expected in indexes(name) {
            let expected_name = expected.options.as_ref().and_then(|o| o.name.as_ref());
            let mut matching = actual.iter().filter(|index| {
                index.keys.iter().eq(expected.keys.iter())
                    || index.options.as_ref().and_then(|o| o.name.as_ref()) == expected_name
            });
            match matching.next() {
                None => pending.push(Step::Index(name, Box::new(expected))),
                Some(index) => {
                    if matching.next().is_some()
                        || !index.keys.iter().eq(expected.keys.iter())
                        || !index.options.as_ref().is_some_and(|o| {
                            o.unique == Some(true)
                                && o.name.as_ref() == expected_name
                                && o.partial_filter_expression.is_none()
                                && o.sparse != Some(true)
                                && o.hidden != Some(true)
                                && o.collation.is_none()
                        })
                    {
                        return Err(failed());
                    }
                }
            }
        }
    }
    Ok(pending)
}

pub(in crate::store) async fn pending_count(database: &Database) -> Result<usize, StoreError> {
    Ok(pending(database).await?.len())
}

/// One majority-acknowledged unit. The shared runner owns authority, source
/// identity, checksum, lease renewal and the durable completion receipt.
pub(in crate::store) async fn apply_next(database: &Database) -> Result<bool, StoreError> {
    let Some(step) = pending(database).await?.into_iter().next() else {
        return Ok(false);
    };
    match step {
        Step::Collection(name) => {
            database
                .create_collection(name)
                .validator(validator(name))
                .validation_level(mongodb::options::ValidationLevel::Strict)
                .validation_action(mongodb::options::ValidationAction::Error)
                .write_concern(mongodb::options::WriteConcern::majority())
                .await
                .map_err(unavailable)?;
        }
        Step::Index(name, index) => {
            database
                .collection::<Document>(name)
                .create_index(*index)
                .write_concern(mongodb::options::WriteConcern::majority())
                .await
                .map_err(unavailable)?;
        }
    }
    pending(database).await?;
    Ok(true)
}
