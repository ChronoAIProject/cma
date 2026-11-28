//! Optional persistent exact-thread write fences; ordinary Gateway schema/startup never requires it.
use super::{failure, MongoRecords, RecordError, MIGRATIONS, THREAD_FENCES};
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, Document},
    Database, IndexModel,
};
pub fn role(database: &str, migration: bool) -> Document {
    let mut privileges = vec![
        doc! {"resource":{"db":database,"collection":THREAD_FENCES},"actions":if migration {
            vec!["find","insert","update","createCollection","collMod","createIndex","listIndexes"]
        } else {vec!["find","insert","update","listIndexes"]}},
    ];
    if migration {
        privileges.push(doc! {"resource":{"db":database,"collection":MIGRATIONS},"actions":["find","insert","update","collMod"]});
    }
    doc! {"createRole":if migration {"cma_eg_thread_fences_migrator"} else {"cma_eg_thread_fences_runtime"},"privileges":privileges,"roles":[]}
}
pub(super) fn indexes_valid(indexes: &[IndexModel]) -> bool {
    indexes.len() == 2
        && indexes.iter().all(|i| {
            i.options.as_ref().is_none_or(|o| {
                o.expire_after.is_none()
                    && o.partial_filter_expression.is_none()
                    && o.sparse != Some(true)
            })
        })
        && indexes.iter().any(|i| i.keys == doc! {"_id":1})
        && indexes.iter().any(|i| {
            i.keys == doc! {"owner":1,"_id":1}
                && i.options.as_ref().is_none_or(|o| o.unique != Some(true))
        })
}
impl MongoRecords {
    pub(crate) async fn thread_fences_ready(&self) -> Result<(), RecordError> {
        let invalid = || {
            RecordError::Unavailable("persistent Discord thread fence storage is not ready".into())
        };
        let status = self
            .db
            .run_command(doc! {"connectionStatus":1,"showPrivileges":true})
            .await
            .map_err(failure)?;
        cma_service_auth::mongodb_authority::verify_shared_schema(
            &status,
            self.db.name(),
            &role(self.db.name(), false),
        )
        .map_err(|_| invalid())?;
        if self
            .db
            .collection::<Document>(MIGRATIONS)
            .find_one(doc! {"_id":"thread_fences","version":1})
            .await
            .map_err(failure)?
            .is_none()
        {
            return Err(invalid());
        }
        let indexes: Vec<IndexModel> = self
            .db
            .collection::<Document>(THREAD_FENCES)
            .list_indexes()
            .await
            .map_err(failure)?
            .try_collect()
            .await
            .map_err(failure)?;
        if !indexes_valid(&indexes) {
            return Err(invalid());
        }
        Ok(())
    }
}
pub async fn migrate(db: &Database) -> Result<(), RecordError> {
    cma_service_migrations::marker(db, MIGRATIONS, "thread_fences", "version", &[1], 1)
        .await
        .map_err(failure)?;
    // A separate optional marker preserves the base schema version and existing startup.
    let validator = doc! {"$jsonSchema":{"bsonType":"object","additionalProperties":false,"required":["_id","owner","blob","version","schema_version"],"properties":{
    "_id":{"bsonType":"string"},"owner":{"bsonType":"string","minLength":1,"maxLength":512},"blob":{"bsonType":"binData"},"version":{"bsonType":"long","minimum":1},"schema_version":{"enum":[1]},"guard_revision":{"bsonType":"long","minimum":0}}}};
    // The published schema before 1f86a1b6 omitted only the optional guard revision.
    let mut predecessor = validator.clone();
    predecessor
        .get_document_mut("$jsonSchema")
        .unwrap()
        .get_document_mut("properties")
        .unwrap()
        .remove("guard_revision");
    cma_service_migrations::collection_with_predecessors(
        db,
        THREAD_FENCES,
        validator,
        true,
        vec![predecessor],
    )
    .await
    .map_err(failure)?;
    cma_service_migrations::indexes(
        db,
        THREAD_FENCES,
        vec![IndexModel::builder().keys(doc! {"owner":1,"_id":1}).build()],
    )
    .await
    .map_err(failure)?;
    cma_service_migrations::exact_indexes(
        db,
        THREAD_FENCES,
        vec![IndexModel::builder().keys(doc! {"owner":1,"_id":1}).build()],
        indexes_valid,
    )
    .await
    .map_err(failure)?;
    cma_service_migrations::collection_with_predecessors(
        db,
        MIGRATIONS,
        super::schema::migration_validator_for(db)
            .await
            .map_err(failure)?,
        false,
        crate::records::mongo::migration_predecessors(),
    )
    .await
    .map_err(failure)?;
    cma_service_migrations::update(
        db,
        MIGRATIONS,
        doc! {"_id":"thread_fences"},
        doc! {"$set":{"version":1}},
        true,
        false,
    )
    .await
    .map_err(failure)?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn persistent_storage_requires_exact_indexes_without_expiry_or_partial_visibility() {
        let valid = vec![
            IndexModel::builder().keys(doc! {"_id":1}).build(),
            IndexModel::builder().keys(doc! {"owner":1,"_id":1}).build(),
        ];
        assert!(indexes_valid(&valid));
        assert!(!indexes_valid(&valid[..1]));
        for options in [
            mongodb::options::IndexOptions::builder()
                .expire_after(std::time::Duration::ZERO)
                .build(),
            mongodb::options::IndexOptions::builder()
                .partial_filter_expression(doc! {"owner":"x"})
                .build(),
            mongodb::options::IndexOptions::builder()
                .sparse(true)
                .build(),
        ] {
            let mut bad = valid.clone();
            bad[1].options = Some(options);
            assert!(!indexes_valid(&bad));
        }
        let role = role("gateway", false);
        let grants = role.get_array("privileges").unwrap();
        assert_eq!(grants.len(), 1);
        assert!(!role.to_string().contains("remove"));
    }
}
