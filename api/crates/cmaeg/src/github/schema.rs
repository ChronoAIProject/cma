use mongodb::{
    bson::{doc, Document},
    options::{Acknowledgment, ReadConcern, WriteConcern},
    Database, IndexModel,
};

use super::Error;

pub(super) const COLLECTION: &str = "eg_automation_records";

pub fn runtime_role(database: &str) -> Document {
    role(database, false)
}
pub fn migration_role(database: &str) -> Document {
    role(database, true)
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
            Some(1..=4) => Ok(true),
            Some(5) => Ok(false),
            _ => Err(Error::Unavailable),
        },
    }
}

pub async fn migrate(database: &Database) -> Result<(), Error> {
    if !migration_required(database).await? {
        return Ok(());
    }
    let mut validator = doc! {"$jsonSchema":{"bsonType":"object","additionalProperties":false,"required":["_id","owner","kind","version","schema_version","blob"],"properties":{"_id":{"bsonType":"string"},"owner":{"bsonType":"string","minLength":1,"maxLength":512},"kind":{"enum":["binding","binding_operation","binding_approval","delivery","execution_group","app_integration","app_registration","webhook_credential","app_operation","app_identity"]},"version":{"bsonType":"long","minimum":1},"schema_version":{"enum":[1]},"guard_revision":{"bsonType":"long","minimum":0},"lease_owner":{"bsonType":"string"},"lease_until":{"bsonType":"long"},"expires_at":{"bsonType":"date"},"blob":{"bsonType":"binData"}}}};
    validator.insert(
        "$or",
        mongodb::bson::bson!([
            {"kind":"delivery","expires_at":{"$type":"date"}},
            {"kind":{"$ne":"delivery"},"expires_at":{"$exists":false}}
        ]),
    );
    match database.run_command(doc! {"create":COLLECTION,"validator":validator.clone(),"validationLevel":"strict","validationAction":"error"}).await {
        Ok(_) => (),
        Err(error) if matches!(error.kind.as_ref(), mongodb::error::ErrorKind::Command(command) if command.code == 48) => { database.run_command(doc! {"collMod":COLLECTION,"validator":validator,"validationLevel":"strict","validationAction":"error"}).await.map_err(|_| Error::Unavailable)?; },
        Err(_) => return Err(Error::Unavailable),
    }
    database
        .collection::<Document>(COLLECTION)
        .create_index(
            IndexModel::builder()
                .keys(doc! {"owner":1,"kind":1,"_id":1})
                .build(),
        )
        .await
        .map_err(|error| migration_failure("create_index", error))?;
    database
        .collection::<Document>(COLLECTION)
        .create_index(
            IndexModel::builder()
                .keys(doc! {"expires_at":1})
                .options(
                    mongodb::options::IndexOptions::builder()
                        .expire_after(std::time::Duration::ZERO)
                        .build(),
                )
                .build(),
        )
        .await
        .map_err(|error| migration_failure("receipt_expiry_index", error))?;
    let legacy_until = mongodb::bson::DateTime::from_millis((super::now() + 7 * 86400) * 1000);
    database
        .collection::<Document>(COLLECTION)
        .update_many(
            doc! {"kind":"delivery","expires_at":{"$exists":false}},
            doc! {"$set":{"expires_at":legacy_until}},
        )
        .write_concern(WriteConcern::builder().w(Acknowledgment::Majority).build())
        .await
        .map_err(|error| migration_failure("legacy_receipt_expiry", error))?;
    database.run_command(doc! {"collMod":crate::records::mongo::MIGRATIONS,"validator":crate::records::mongo::migration_validator(),"validationLevel":"strict","validationAction":"error"}).await.map_err(|_| Error::Unavailable)?;
    let committed = database
        .collection::<Document>(crate::records::mongo::MIGRATIONS)
        .update_one(
            doc! {"_id":"github_integrations","version":{"$in":[1,2,3,4]}},
            doc! {"$set":{"version":5}},
        )
        .upsert(true)
        .write_concern(WriteConcern::builder().w(Acknowledgment::Majority).build())
        .await;
    match committed {
        Ok(_) => Ok(()),
        Err(error)
            if error_code(&error) == Some(11000)
                && matches!(migration_required(database).await, Ok(false)) =>
        {
            Ok(())
        }
        Err(error) => Err(migration_failure("commit_marker", error)),
    }
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
