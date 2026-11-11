//! Explicit additive DDL. Startup only verifies; partial execution is restartable.
use futures_util::TryStreamExt;
use layer2_core::StoreError;
use mongodb::{
    bson::{doc, Document},
    options::IndexOptions,
    Database, IndexModel,
};

use super::{AUDITS, COLLECTIONS, EVIDENCE, HEADS, PROJECTIONS, PUBLICATIONS, SEGMENTS};

mod controls;
mod index_contract;
mod recovery;
mod validators;
use validators::validator;

fn failed() -> StoreError {
    StoreError::Malformed("Agent log segment DDL is absent, partial or incompatible; run the reviewed additive migration".into())
}

fn unavailable(error: mongodb::error::Error) -> StoreError {
    StoreError::Unavailable(format!("Agent log segment schema: {error}"))
}

fn indexes(name: &str) -> Vec<IndexModel> {
    if name == super::RECOVERY_WORK {
        return recovery::indexes();
    }
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
    let mut result: Vec<_> = fields
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
        .collect();
    if name == EVIDENCE {
        result.push(
            IndexModel::builder()
                .keys(doc! {"agent_id":1,"incarnation":1,"evidence_generation":1,
                "kind":1,"turn_sequence":1,"change_ordinal":1})
                .options(
                    IndexOptions::builder()
                        .name("agent_log_output_change_seek".to_owned())
                        .unique(true)
                        .partial_filter_expression(doc! {"kind":"output_change"})
                        .build(),
                )
                .build(),
        );
    }
    if name == super::writer_admission::COLLECTION {
        result.push(
            IndexModel::builder()
                .keys(doc! {"agent_id":1,"incarnation":1,"publication_key":1})
                .options(
                    IndexOptions::builder()
                        .name("agent_log_admission_key".to_owned())
                        .unique(true)
                        .build(),
                )
                .build(),
        );
    }
    result
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
        let expected = indexes(name);
        // A complete inventory is part of the contract: an unrelated index can
        // reject otherwise valid writes or change a consumer's query plan.
        if actual
            .iter()
            .filter(|index| index_contract::primary_matches(index))
            .count()
            != 1
            || actual.iter().any(|index| {
                !index_contract::primary_matches(index)
                    && !expected
                        .iter()
                        .any(|expected| index_contract::matches(index, expected))
            })
        {
            return Err(failed());
        }
        for expected in expected {
            let expected_name = expected.options.as_ref().and_then(|o| o.name.as_ref());
            let mut matching = actual.iter().filter(|index| {
                index.keys.iter().eq(expected.keys.iter())
                    || index.options.as_ref().and_then(|o| o.name.as_ref()) == expected_name
            });
            match matching.next() {
                None => pending.push(Step::Index(name, Box::new(expected))),
                Some(index) => {
                    if matching.next().is_some() || !index_contract::matches(index, &expected) {
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
