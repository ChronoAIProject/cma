#[path = "schema_predecessors.rs"]
mod predecessors;
use mongodb::{
    bson::{doc, Document},
    options::ReadConcern,
    Database, IndexModel,
};

use super::Error;

pub(super) const COLLECTION: &str = "eg_automation_records";

pub fn runtime_role(database: &str) -> Document {
    role(database, false)
}
pub fn migration_role(database: &str) -> Document {
    crate::migrations::migration_role(role(database, true), database)
}

fn role(database: &str, migration: bool) -> Document {
    let mut privileges = vec![
        doc! {"resource":{"db":database,"collection":COLLECTION},"actions":if migration {vec!["find","insert","update","createCollection","collMod","createIndex","listIndexes"]} else {vec!["find","insert","update","listIndexes"]}},
    ];
    if migration {
        privileges.push(doc! {"resource":{"db":database,"collection":crate::records::mongo::MIGRATIONS},"actions":["find","insert","update","collMod"]});
    }
    doc! {"createRole":if migration {"cma_eg_automation_migrator"} else {"cma_eg_automation_runtime"},"privileges":privileges,"roles":[]}
}

pub(super) async fn migration_required(database: &Database) -> Result<bool, Error> {
    let marker = database
        .collection::<Document>(crate::records::mongo::MIGRATIONS)
        .find_one(doc! {"_id":"github_integrations"})
        .read_concern(ReadConcern::majority())
        .await
        .map_err(|_| Error::Unavailable)?;
    match marker {
        None => Ok(true),
        Some(marker) => match marker
            .get("version")
            .and_then(|value| value.as_i32().map(i64::from).or_else(|| value.as_i64()))
        {
            Some(1..=5) => Ok(true),
            Some(6) => Ok(false),
            _ => Err(Error::Unavailable),
        },
    }
}

pub async fn migrate(database: &Database) -> Result<(), Error> {
    let ready = cma_service_migrations::marker(
        database,
        crate::records::mongo::MIGRATIONS,
        "github_integrations",
        "version",
        &[1, 2, 3, 4, 5, 6],
        6,
    )
    .await
    .map_err(|_| Error::Unavailable)?;
    let needed = !ready;
    let mut validator = doc! {"$jsonSchema":{"bsonType":"object","additionalProperties":false,"required":["_id","owner","kind","version","schema_version","blob"],"properties":{"_id":{"bsonType":"string"},"owner":{"bsonType":"string","minLength":1,"maxLength":512},"kind":{"enum":["binding","binding_operation","binding_approval","delivery","execution_group","app_integration","app_registration","webhook_credential","app_operation","app_identity"]},"version":{"bsonType":"long","minimum":1},"schema_version":{"enum":[1]},"guard_revision":{"bsonType":"long","minimum":0},"lease_until":{"bsonType":"long"},"next_dispatch_at":{"bsonType":"long"},"expires_at":{"bsonType":"date"},"blob":{"bsonType":"binData"}}}};
    validator.insert(
        "$or",
        mongodb::bson::bson!([
            {"kind":"delivery","expires_at":{"$type":"date"}},
            {"kind":{"$ne":"delivery"},"expires_at":{"$exists":false}}
        ]),
    );
    cma_service_migrations::collection_with_predecessors(
        database,
        COLLECTION,
        validator,
        true,
        predecessors::prior_validators(),
    )
    .await
    .map_err(|_| Error::Unavailable)?;
    cma_service_migrations::indexes(
        database,
        COLLECTION,
        vec![
            IndexModel::builder()
                .keys(doc! {"owner":1,"kind":1,"_id":1})
                .build(),
            IndexModel::builder()
                .keys(doc! {"expires_at":1})
                .options(
                    mongodb::options::IndexOptions::builder()
                        .expire_after(std::time::Duration::ZERO)
                        .build(),
                )
                .build(),
            IndexModel::builder()
                .keys(doc! {"owner":1,"kind":1,"next_dispatch_at":1,"_id":1})
                .build(),
        ],
    )
    .await
    .map_err(|error| migration_failure("create_index", error))?;
    let expiry_filter = doc! {"kind":"delivery","expires_at":{"$exists":false}};
    let dispatch_filter = doc! {"kind":"delivery","next_dispatch_at":{"$exists":false}};
    if needed || cma_service_migrations::inspecting() {
        cma_service_migrations::absent(database, COLLECTION, expiry_filter.clone())
            .await
            .map_err(|_| Error::Unavailable)?;
        cma_service_migrations::absent(database, COLLECTION, dispatch_filter.clone())
            .await
            .map_err(|_| Error::Unavailable)?;
        let now = if cma_service_migrations::describing() {
            0
        } else {
            super::now()
        };
        let legacy_until = mongodb::bson::DateTime::from_millis((now + 7 * 86400) * 1000);
        cma_service_migrations::update(
            database,
            COLLECTION,
            expiry_filter,
            doc! {"$set":{"expires_at":legacy_until}},
            false,
            true,
        )
        .await
        .map_err(|error| migration_failure("legacy_receipt_expiry", error))?;
        cma_service_migrations::update(
            database,
            COLLECTION,
            dispatch_filter,
            doc! {"$set":{"next_dispatch_at":i64::MAX}},
            false,
            true,
        )
        .await
        .map_err(|error| migration_failure("legacy_dispatch_deadline", error))?;
    }
    cma_service_migrations::collection_with_predecessors(
        database,
        crate::records::mongo::MIGRATIONS,
        crate::records::mongo::migration_validator_for(database)
            .await
            .map_err(|_| Error::Unavailable)?,
        false,
        crate::records::mongo::migration_predecessors(),
    )
    .await
    .map_err(|_| Error::Unavailable)?;
    if needed {
        cma_service_migrations::update(
            database,
            crate::records::mongo::MIGRATIONS,
            doc! {"_id":"github_integrations","version":{"$in":[1,2,3,4,5]}},
            doc! {"$set":{"version":6}},
            true,
            false,
        )
        .await
        .map_err(|error| migration_failure("commit_marker", error))?;
    }
    Ok(())
}

fn error_code(error: &mongodb::error::Error) -> Option<i32> {
    match error.kind.as_ref() {
        mongodb::error::ErrorKind::Command(command) => Some(command.code),
        mongodb::error::ErrorKind::Write(mongodb::error::WriteFailure::WriteError(write)) => {
            Some(write.code)
        }
        _ => None,
    }
}

fn migration_failure(phase: &'static str, error: mongodb::error::Error) -> Error {
    let code = error_code(&error);
    tracing::warn!(phase, code, "GitHub schema initialization command failed");
    Error::Unavailable
}
