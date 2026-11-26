//! Installation of the Channel first-insert writer floor after draining old writers.
use super::{create_fences::CREATE_FENCES, failure, schema, RecordError, MIGRATIONS, RECORDS};
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, Document},
    options::{Acknowledgment, ReadConcern, ValidationAction, ValidationLevel, WriteConcern},
    Database,
};

pub(super) const MARKER: &str = "channel_create_fences";
pub(super) const VALIDATOR_CHANGE: &str = "channel_validator_change";
const IDLE: i32 = 1;
const LEGACY_UPGRADE: i32 = 2;
const FENCED_WRITERS: i32 = 3;

async fn validator_phase(db: &Database) -> Result<Option<i32>, RecordError> {
    let row = db
        .collection::<Document>(MIGRATIONS)
        .find_one(doc! {"_id": VALIDATOR_CHANGE})
        .read_concern(ReadConcern::majority())
        .await
        .map_err(failure)?;
    row.map(|row| {
        row.get_i32("version").map_err(|_| {
            RecordError::Unavailable("Channel validator migration phase is unreadable".into())
        })
    })
    .transpose()
}

async fn begin_validator_change(db: &Database, next: i32) -> Result<bool, RecordError> {
    let filter = doc! {"_id": VALIDATOR_CHANGE, "version": IDLE};
    let change = doc! {"$set": {"version": next}};
    cma_service_migrations::update(
        db,
        MIGRATIONS,
        doc! {"_id": VALIDATOR_CHANGE},
        doc! {"$setOnInsert": {"version": IDLE}},
        true,
        false,
    )
    .await
    .map_err(failure)?;
    if cma_service_migrations::describing() {
        cma_service_migrations::update(db, MIGRATIONS, filter, change, false, false)
            .await
            .map_err(failure)?;
        return Ok(true);
    }
    let rows = db.collection::<Document>(MIGRATIONS);
    let result = cma_service_migrations::effect(
        db,
        doc! {"update":MIGRATIONS,"updates":[{"q":&filter,"u":&change,"upsert":false,"multi":false}]},
        async {
            rows.update_one(filter, change)
                .write_concern(WriteConcern::builder().w(Acknowledgment::Majority).build())
                .await
        },
    )
    .await
    .map_err(failure)?;
    if result.matched_count == 1 {
        return Ok(true);
    }
    if validator_phase(db).await? == Some(FENCED_WRITERS) {
        return Ok(next == FENCED_WRITERS);
    }
    Err(RecordError::Unavailable(
        "Channel validator migration is in progress or unresolved".into(),
    ))
}

pub(super) async fn upgrade_legacy_validator(
    db: &Database,
    validator: Document,
) -> Result<(), RecordError> {
    if !begin_validator_change(db, LEGACY_UPGRADE).await? {
        return Ok(());
    }
    let result = set_channel_validator(db, validator).await;
    // A lost DDL acknowledgement can leave the old operation in flight. Its
    // durable phase must then prevent installation of the stronger validator.
    if result.is_ok()
        || matches!(&result, Err(error)
        if matches!(error.kind.as_ref(), mongodb::error::ErrorKind::Command(command)
            if matches!(command.code, 2 | 13 | 26)))
    {
        cma_service_migrations::update(
            db,
            MIGRATIONS,
            doc! {"_id": VALIDATOR_CHANGE, "version": LEGACY_UPGRADE},
            doc! {"$set": {"version": IDLE}},
            false,
            false,
        )
        .await
        .map_err(failure)?;
    }
    result.map_err(failure)?;
    Ok(())
}

async fn set_channel_validator(
    db: &Database,
    validator: Document,
) -> mongodb::error::Result<Document> {
    cma_service_migrations::command(
        db,
        doc! {
            "collMod": RECORDS, "validator": validator,
            "validationLevel": "strict", "validationAction": "error",
            "writeConcern": {"w": "majority"}
        },
    )
    .await
}

pub fn role(database: &str, migration: bool) -> Document {
    let actions = if migration {
        vec![
            "find",
            "insert",
            "update",
            "createCollection",
            "collMod",
            "listIndexes",
        ]
    } else {
        vec!["find", "insert", "update", "listIndexes"]
    };
    let mut privileges = vec![
        doc! {
            "resource": {"db": database, "collection": CREATE_FENCES}, "actions": actions
        },
        doc! {
            "resource": {"db": database, "collection": ""}, "actions": ["listCollections"]
        },
    ];
    if migration {
        privileges.extend([
            doc! {"resource": {"db": database, "collection": RECORDS}, "actions": ["find", "update", "collMod"]},
            doc! {"resource": {"db": database, "collection": MIGRATIONS}, "actions": ["find", "insert", "update", "collMod"]},
        ]);
    }
    doc! {
        "createRole": if migration {"cma_eg_create_fence_migrator"} else {"cma_eg_create_fence_runtime"},
        "privileges": privileges, "roles": []
    }
}

pub(crate) async fn verify_runtime(db: &Database, issuer: &str) -> Result<bool, RecordError> {
    if !verify(db, issuer).await? {
        return Ok(false);
    }
    let status = db
        .run_command(doc! {"connectionStatus": 1, "showPrivileges": true})
        .await
        .map_err(failure)?;
    cma_service_auth::mongodb_authority::verify_shared_schema(
        &status,
        db.name(),
        &role(db.name(), false),
    )
    .map_err(|_| {
        RecordError::Unavailable("Gateway Channel create fence permissions are missing".into())
    })?;
    Ok(true)
}

pub(super) async fn installed(db: &Database) -> Result<bool, RecordError> {
    Ok(db
        .collection::<Document>(MIGRATIONS)
        .find_one(doc! {"_id": MARKER, "version": 1})
        .read_concern(ReadConcern::majority())
        .await
        .map_err(failure)?
        .is_some())
}

pub(crate) async fn verify(db: &Database, issuer: &str) -> Result<bool, RecordError> {
    if !installed(db).await? {
        return Ok(false);
    }
    let fences = db.collection::<Document>(CREATE_FENCES);
    let row = fences
        .find_one(doc! {"_id": "writer_floor"})
        .read_concern(ReadConcern::majority())
        .await
        .map_err(failure)?;
    let indexes = fences
        .list_indexes()
        .await
        .map_err(failure)?
        .try_collect::<Vec<_>>()
        .await
        .map_err(failure)?;
    if validator_phase(db).await? != Some(FENCED_WRITERS)
        || !row.is_some_and(|row| {
            row.get_str("issuer").ok() == Some(issuer) && row.get_i32("version").ok() == Some(1)
        })
        || indexes.len() != 1
        || indexes[0].keys != doc! {"_id": 1}
        || indexes[0]
            .options
            .as_ref()
            .is_some_and(|options| options.expire_after.is_some())
        || !channel_validator_is_strict(db).await?
    {
        return Err(RecordError::Unavailable(
            "Channel create fence writer floor is incompatible".into(),
        ));
    }
    Ok(true)
}

async fn channel_validator_is_strict(db: &Database) -> Result<bool, RecordError> {
    let collection = db
        .list_collections()
        .filter(doc! {"name": RECORDS})
        .await
        .map_err(failure)?
        .try_next()
        .await
        .map_err(failure)?;
    Ok(collection.is_some_and(|collection| {
        matches!(
            collection.options.validation_level,
            Some(ValidationLevel::Strict)
        ) && matches!(
            collection.options.validation_action,
            Some(ValidationAction::Error)
        ) && collection.options.validator == Some(schema::record_validator(true))
    }))
}

fn fence_validator() -> Document {
    doc! {"$or": [
        {"$and": [{"_id": "writer_floor"}, {"version": 1}, {"issuer": {"$type": "string"}}]},
        {"$jsonSchema": {"bsonType": "object", "required": ["_id", "revision", "cancelled"],
            "additionalProperties": false,
            "properties": {
                "_id": {"bsonType": "object", "required": ["issuer", "kind", "subject", "record"],
                    "additionalProperties": false,
                    "properties": {
                        "issuer": {"bsonType": "string"}, "kind": {"enum": ["human", "organization"]},
                        "subject": {"bsonType": "string"}, "record": {"bsonType": "string"}
                    }
                },
                "revision": {"bsonType": "long", "minimum": 1}, "cancelled": {"bsonType": "bool"}
            }
        }}
    ]}
}

async fn marker_catalog(db: &Database) -> Result<(), RecordError> {
    cma_service_migrations::collection_with_predecessors(
        db,
        MIGRATIONS,
        schema::migration_validator(),
        false,
        schema::migration_predecessors(),
    )
    .await
    .map_err(failure)
}

async fn channel_catalog(db: &Database) -> Result<(), RecordError> {
    cma_service_migrations::collection_with_guarded_replacement(
        db,
        RECORDS,
        schema::record_validator(true),
        false,
        vec![schema::record_validator(false)],
        None,
        |validator| async move { set_channel_validator(db, validator).await.map(|_| ()) },
    )
    .await
    .map_err(failure)
}

pub(crate) async fn inspect_migration(db: &Database, issuer: &str) -> Result<bool, RecordError> {
    let catalog_ready = cma_service_migrations::inspect(async {
        marker_catalog(db)
            .await
            .map_err(|_| "Channel migration-marker validator unavailable")?;
        cma_service_migrations::collection(db, CREATE_FENCES, fence_validator(), false)
            .await
            .map_err(|_| "Channel create fence validator unavailable")?;
        channel_catalog(db)
            .await
            .map_err(|_| "Channel record validator unavailable")
    })
    .await
    .map_err(|reason| RecordError::Unavailable(reason.into()))?;
    if installed(db).await? {
        if !catalog_ready {
            return Err(RecordError::Unavailable(
                "Channel create fence catalog drift".into(),
            ));
        }
        return verify(db, issuer).await;
    }

    if !matches!(
        validator_phase(db).await?,
        None | Some(IDLE | FENCED_WRITERS)
    ) {
        return Err(RecordError::Unavailable(
            "Channel validator migration is in progress or unresolved".into(),
        ));
    }
    let retained = db
        .collection::<Document>(CREATE_FENCES)
        .find_one(doc! {"_id": "writer_floor"})
        .read_concern(ReadConcern::majority())
        .await
        .map_err(failure)?;
    if retained.is_some_and(|row| {
        row.get_str("issuer").ok() != Some(issuer) || row.get_i32("version").ok() != Some(1)
    }) {
        return Err(RecordError::Unavailable(
            "Channel create fence writer floor is incompatible".into(),
        ));
    }
    Ok(false)
}

pub async fn migrate(db: &Database, issuer: &str) -> Result<(), RecordError> {
    if !cma_service_migrations::describing() && inspect_migration(db, issuer).await? {
        return Ok(());
    }
    let url = url::Url::parse(issuer)
        .map_err(|_| RecordError::Rejected("invalid Channel create fence issuer".into()))?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if !(url.scheme() == "https" || url.scheme() == "http" && loopback)
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || issuer.ends_with('/')
    {
        return Err(RecordError::Rejected(
            "invalid Channel create fence issuer".into(),
        ));
    }
    marker_catalog(db).await?;
    begin_validator_change(db, FENCED_WRITERS).await?;
    cma_service_migrations::collection(db, CREATE_FENCES, fence_validator(), true)
        .await
        .map_err(failure)?;
    cma_service_migrations::update(
        db,
        CREATE_FENCES,
        doc! {"_id": "writer_floor", "issuer": issuer, "version": 1},
        doc! {"$setOnInsert": {"issuer": issuer, "version": 1}},
        true,
        false,
    )
    .await
    .map_err(failure)?;
    channel_catalog(db).await?;
    let without_floor =
        doc! {"$or": [{"guard_revision": {"$exists": false}}, {"guard_revision": 0}]};
    cma_service_migrations::update(
        db,
        RECORDS,
        without_floor,
        doc! {"$set": {"guard_revision": 1_i64}},
        false,
        true,
    )
    .await
    .map_err(failure)?;
    cma_service_migrations::update(
        db,
        MIGRATIONS,
        doc! {"_id": MARKER},
        doc! {"$set": {"version": 1}},
        true,
        false,
    )
    .await
    .map_err(failure)?;
    if cma_service_migrations::describing() {
        return Ok(());
    }
    require_installed(db, issuer).await
}

async fn require_installed(db: &Database, issuer: &str) -> Result<(), RecordError> {
    if inspect_migration(db, issuer).await? {
        Ok(())
    } else {
        Err(RecordError::Unavailable(
            "Gateway Channel create fence floor postcondition unavailable".into(),
        ))
    }
}
