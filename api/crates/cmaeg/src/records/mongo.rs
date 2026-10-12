//! Gateway-owned sealed record persistence. No Agent Manager collections.
use super::{RecordError, RecordHead, RecordValue};
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, spec::BinarySubtype, Binary, Document},
    options::{Acknowledgment, ReadConcern, ReturnDocument, WriteConcern},
    Collection, Database,
};
use serde::{Deserialize, Serialize};

pub const RECORDS: &str = "eg_channel_records";
pub const MIGRATIONS: &str = "eg_schema_migrations";
pub const SESSIONS: &str = "eg_auth_sessions";
#[derive(Clone, Debug)]
pub struct MongoRecords {
    db: Database,
    records: Collection<Record>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    #[serde(rename = "_id")]
    id: String,
    owner: String,
    blob: Binary,
    version: i64,
    schema_version: i32,
}

impl MongoRecords {
    pub async fn open(db: Database) -> Result<Self, RecordError> {
        verify(&db).await?;
        let options = mongodb::options::CollectionOptions::builder()
            .read_concern(ReadConcern::majority())
            .write_concern(WriteConcern::builder().w(Acknowledgment::Majority).build())
            .build();
        Ok(Self {
            records: db.collection_with_options(RECORDS, options),
            db,
        })
    }
    pub async fn reachable(&self) -> bool {
        self.db.run_command(doc! {"ping":1}).await.is_ok()
    }
    pub async fn put(
        &self,
        id: &str,
        owner: &str,
        blob: &[u8],
        expected: Option<u64>,
    ) -> Result<u64, RecordError> {
        if owner.is_empty() || owner.len() > 512 || blob.is_empty() || blob.len() > 65536 {
            return Err(RecordError::Rejected("invalid record".into()));
        }
        let binary = Binary {
            subtype: BinarySubtype::Generic,
            bytes: blob.to_vec(),
        };
        if let Some(version) = expected {
            let version = i64::try_from(version)
                .map_err(|_| RecordError::Rejected("invalid record revision".into()))?;
            let row = self
                .records
                .find_one_and_update(
                    doc! {"_id":id,"owner":owner,"version":version},
                    doc! {"$set":{"blob":binary},"$inc":{"version":1_i64}},
                )
                .return_document(ReturnDocument::After)
                .await
                .map_err(failure)?;
            return match row {
                Some(r) => Ok(r.version as u64),
                None => Err(self.conflict(id, expected.unwrap_or(0)).await?),
            };
        }
        let row = Record {
            id: id.into(),
            owner: owner.into(),
            blob: binary,
            version: 1,
            schema_version: 1,
        };
        match self.records.insert_one(&row).await {
            Ok(_) => Ok(1),
            Err(error) => {
                if self
                    .records
                    .find_one(doc! {"_id":id})
                    .await
                    .map_err(failure)?
                    .is_some()
                {
                    Err(self.conflict(id, 0).await?)
                } else {
                    Err(failure(error))
                }
            }
        }
    }
    pub async fn get(&self, id: &str) -> Result<RecordValue, RecordError> {
        let row = self
            .records
            .find_one(doc! {"_id":id})
            .await
            .map_err(failure)?
            .ok_or(RecordError::NotFound)?;
        Ok(RecordValue {
            blob: row.blob.bytes,
            version: row.version as u64,
        })
    }
    pub async fn delete(&self, id: &str, expected: u64) -> Result<bool, RecordError> {
        let version =
            i64::try_from(expected).map_err(|_| RecordError::Rejected("invalid version".into()))?;
        if self
            .records
            .delete_one(doc! {"_id":id,"version":version})
            .await
            .map_err(failure)?
            .deleted_count
            == 1
        {
            return Ok(true);
        }
        if self
            .records
            .find_one(doc! {"_id":id})
            .await
            .map_err(failure)?
            .is_none()
        {
            return Ok(false);
        }
        Err(self.conflict(id, expected).await?)
    }
    pub async fn list(
        &self,
        owner: Option<&str>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<RecordHead>, RecordError> {
        if !(1..=1000).contains(&limit) {
            return Err(RecordError::Rejected("invalid page size".into()));
        }
        let mut filter = doc! {};
        if let Some(owner) = owner {
            filter.insert("owner", owner);
        }
        if let Some(after) = after {
            filter.insert("_id", doc! {"$gt":after});
        }
        let rows: Vec<Record> = self
            .records
            .find(filter)
            .sort(doc! {"_id":1})
            .limit(limit as i64)
            .await
            .map_err(failure)?
            .try_collect()
            .await
            .map_err(failure)?;
        Ok(rows
            .into_iter()
            .map(|r| RecordHead {
                record_id: r.id,
                owner: r.owner,
                version: r.version as u64,
            })
            .collect())
    }
    async fn conflict(&self, id: &str, expected: u64) -> Result<RecordError, RecordError> {
        let current = self
            .records
            .find_one(doc! {"_id":id})
            .await
            .map_err(failure)?;
        Ok(RecordError::Conflict {
            expected,
            actual: current.map(|r| r.version as u64),
        })
    }
}
fn failure(_: mongodb::error::Error) -> RecordError {
    RecordError::Unavailable("gateway database operation failed".into())
}

pub fn role(database: &str, migration: bool) -> Document {
    let grants:Vec<_>=[RECORDS,MIGRATIONS,SESSIONS].iter().map(|name|doc!{
        "resource":{"db":database,"collection":*name},
        "actions":if migration {vec!["find","insert","update","createCollection","collMod","createIndex","listIndexes"]} else if *name==MIGRATIONS {vec!["find"]} else {vec!["find","insert","update","remove"]}
    }).collect();
    doc! {"createRole":if migration {"cma_eg_migrator"}else{"cma_eg_runtime"},"privileges":grants,"roles":[]}
}
pub async fn verify(db: &Database) -> Result<(), RecordError> {
    let invalid = || RecordError::Rejected("gateway database credential is not isolated".into());
    let status = db
        .run_command(doc! {"connectionStatus":1,"showPrivileges":true})
        .await
        .map_err(failure)?;
    let info = status.get_document("authInfo").map_err(|_| invalid())?;
    if info
        .get_array("authenticatedUsers")
        .map_err(|_| invalid())?
        .len()
        != 1
    {
        return Err(invalid());
    }
    let mut grants =
        std::collections::BTreeMap::<String, std::collections::BTreeSet<String>>::new();
    for grant in info
        .get_array("authenticatedUserPrivileges")
        .map_err(|_| invalid())?
    {
        let grant = grant.as_document().ok_or_else(&invalid)?;
        let resource = grant.get_document("resource").map_err(|_| invalid())?;
        let collection = resource.get_str("collection").map_err(|_| invalid())?;
        if resource.len() != 2
            || resource.get_str("db").ok() != Some(db.name())
            || ![RECORDS, MIGRATIONS, SESSIONS].contains(&collection)
        {
            return Err(invalid());
        }
        for action in grant.get_array("actions").map_err(|_| invalid())? {
            let action = action.as_str().ok_or_else(&invalid)?;
            let allowed = if collection != MIGRATIONS {
                &["find", "insert", "update", "remove"][..]
            } else {
                &["find"][..]
            };
            if !allowed.contains(&action) {
                return Err(invalid());
            }
            grants
                .entry(collection.into())
                .or_default()
                .insert(action.into());
        }
    }
    if !["find", "insert", "update", "remove"]
        .iter()
        .all(|a| grants.get(RECORDS).is_some_and(|s| s.contains(*a)))
        || !grants.get(MIGRATIONS).is_some_and(|s| s.contains("find"))
    {
        return Err(invalid());
    }
    if !["find", "insert", "update", "remove"]
        .iter()
        .all(|a| grants.get(SESSIONS).is_some_and(|s| s.contains(*a)))
    {
        return Err(invalid());
    }
    let marker = db
        .collection::<Document>(MIGRATIONS)
        .find_one(doc! {"_id":"schema","version":1})
        .await
        .map_err(failure)?;
    if marker.is_none() {
        return Err(RecordError::Unavailable(
            "gateway schema migration required".into(),
        ));
    }
    Ok(())
}
pub async fn migrate(db: &Database) -> Result<(), RecordError> {
    cma_service_auth::browser::migrate(db, SESSIONS)
        .await
        .map_err(failure)?;
    let record_validator = doc! {"$jsonSchema":{"bsonType":"object","additionalProperties":false,"required":["_id","owner","blob","version","schema_version"],"properties":{
    "_id":{"bsonType":"string"},"owner":{"bsonType":"string","minLength":1,"maxLength":512},"blob":{"bsonType":"binData"},"version":{"bsonType":"long","minimum":1},"schema_version":{"enum":[1]}}}};
    for (name, validator) in [
        (RECORDS, record_validator),
        (
            MIGRATIONS,
            doc! {"$jsonSchema":{"bsonType":"object","additionalProperties":false,"required":["_id","version"],"properties":{"_id":{"enum":["schema"]},"version":{"enum":[1]}}}},
        ),
    ] {
        match db.run_command(doc!{"create":name,"validator":validator.clone(),"validationLevel":"strict","validationAction":"error"}).await{
            Ok(_)=>{},Err(error) if matches!(error.kind.as_ref(),mongodb::error::ErrorKind::Command(c) if c.code==48)=>{db.run_command(doc!{"collMod":name,"validator":validator,"validationLevel":"strict","validationAction":"error"}).await.map_err(failure)?;},Err(error)=>return Err(failure(error)),
        }
    }
    db.collection::<Document>(RECORDS)
        .create_index(
            mongodb::IndexModel::builder()
                .keys(doc! {"owner":1,"_id":1})
                .build(),
        )
        .await
        .map_err(failure)?;
    db.collection::<Document>(MIGRATIONS)
        .update_one(doc! {"_id":"schema"}, doc! {"$set":{"version":1}})
        .upsert(true)
        .await
        .map_err(failure)?;
    Ok(())
}
