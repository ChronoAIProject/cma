//! Installed floor inspection. Ordinary startup never creates or edits schemas.
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, Document},
    Database,
};

pub(super) const POOLS: &str = "native_warm_pool_targets";

pub(super) fn pool_validator() -> Document {
    doc! {"$or": [
        {"document_schema_version": 3_i64, "machine": false},
        {"document_schema_version": 4_i64, "machine": true}
    ], "$and": [
        {"document_schema_version": {"$type": ["int", "long"]}},
        {"document_schema_version": {"$not": {"$type": "array"}}},
        {"machine": {"$type": "bool"}}, {"machine": {"$not": {"$type": "array"}}}
    ]}
}

pub(crate) async fn verify_floor(database: &Database) -> Result<(), String> {
    let mut cursor = database
        .list_collections()
        .filter(doc! {"name": POOLS})
        .await
        .map_err(|_| "native pool migration floor cannot be inspected")?;
    let collection = cursor
        .try_next()
        .await
        .map_err(|_| "native pool migration floor is unavailable")?
        .ok_or("native pool collection is absent")?;
    if collection.options.validator.as_ref() != Some(&pool_validator())
        || collection.options.validation_level != Some(mongodb::options::ValidationLevel::Strict)
        || collection.options.validation_action != Some(mongodb::options::ValidationAction::Error)
    {
        return Err(
            "native pool reader/writer floor is not the exact strict schema3/4 validator".into(),
        );
    }
    let unknown = doc! {"$nor": [
        {"document_schema_version": 3_i64, "machine": false},
        {"document_schema_version": 4_i64, "machine": true}
    ]};
    if database
        .collection::<Document>(POOLS)
        .find_one(unknown)
        .await
        .map_err(|_| "native pool schema inventory failed")?
        .is_some()
    {
        return Err("native pool migration has unresolved legacy or malformed rows".into());
    }
    Ok(())
}

pub(super) fn plan() -> serde_json::Value {
    serde_json::json!({
        "format": "runtime-release-floor-plan-v1", "apply_authority": false,
        "legacy_finality_verified": false,
        "requires": ["actual legacy process and restart-source closure", "provider/controller/receiver finality", "exact original row snapshot and per-row CAS", "qualified original positive Ready pools", "separate authorized migration executor"],
        "collections": crate::release_authority::COLLECTIONS,
        "pool_floor": {"collection": POOLS, "validator": pool_validator(), "validationLevel": "strict", "validationAction": "error"},
        "activation_history": "preserve original activated_at, revisions, target identities and tombstones",
        "status": "PLAN_ONLY_MIGRATION_EXECUTOR_NOT_IMPLEMENTED"
    })
}

/// Refuse partial migration before any ordinary store/profile/routing effects.
/// Missing head is not sufficient evidence that the protocol is uninstalled.
pub(super) async fn verify_uninstalled(config: &crate::config::Config) -> Result<(), String> {
    let client = super::connect(config).await?;
    let database = client.database(&config.mongodb_database);
    let mut names = crate::release_authority::COLLECTIONS.to_vec();
    names.push(POOLS);
    let mut cursor = database
        .list_collections()
        .filter(doc! {"name": {"$in": names}})
        .await
        .map_err(|_| "runtime release installation floor cannot be inspected")?;
    while let Some(collection) = cursor
        .try_next()
        .await
        .map_err(|_| "runtime release floor inventory failed")?
    {
        if collection.name != POOLS {
            return Err("runtime release protocol is installed or partially migrated; disabled composition refused".into());
        }
        if collection.options.validator.is_some() {
            // Legacy initializer installed no validator. Unknown validators
            // need an explicit compatibility decision; never infer absence.
            return Err("native pool validator requires managed release startup".into());
        }
    }
    if database
        .collection::<Document>(POOLS)
        .find_one(doc! {"document_schema_version": {"$nin": [1_i64, 2]}})
        .await
        .map_err(|_| "native pool reader floor cannot be inspected")?
        .is_some()
    {
        return Err("native pool rows require managed release startup".into());
    }
    Ok(())
}
