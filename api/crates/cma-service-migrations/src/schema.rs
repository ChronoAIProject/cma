//! Shared schema operations. Inspection follows the same definitions as execution.
use super::*;
use futures_util::TryStreamExt;
use mongodb::{options::IndexOptions, IndexModel};
use std::cell::{Cell, RefCell};

#[path = "schema_collections.rs"]
mod collections;
pub use collections::{
    collection, collection_with_guarded_replacement, collection_with_predecessors,
};

tokio::task_local! { static INSPECTION: Cell<bool>; }
tokio::task_local! { static DEFINITION: RefCell<Sha256>; }

/// Discover the ordered schema definitions using the same migration body,
/// without contacting MongoDB. The descriptor hash participates in the
/// immutable manifest checksum, together with its ordered prerequisites.
pub async fn describe(future: impl std::future::Future<Output = Result<()>>) -> Result<[u8; 32]> {
    DEFINITION
        .scope(RefCell::new(Sha256::new()), async {
            future.await?;
            Ok(DEFINITION.with(|digest| digest.borrow().clone().finalize().into()))
        })
        .await
}
pub fn describing() -> bool {
    DEFINITION.try_with(|_| ()).is_ok()
}
fn record(descriptor: Document) -> mongodb::error::Result<()> {
    let bytes =
        mongodb::bson::serialize_to_vec(&descriptor).map_err(mongodb::error::Error::custom)?;
    DEFINITION.with(|digest| {
        let mut digest = digest.borrow_mut();
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
    });
    Ok(())
}

pub fn inspecting() -> bool {
    describing() || INSPECTION.try_with(|_| ()).is_ok()
}
fn missing() {
    INSPECTION.with(|ready| ready.set(false));
}
pub async fn inspect(future: impl std::future::Future<Output = Result<()>>) -> Result<bool> {
    INSPECTION
        .scope(Cell::new(true), async {
            future.await?;
            Ok(INSPECTION.with(Cell::get))
        })
        .await
}
fn invalid(message: &'static str) -> mongodb::error::Error {
    mongodb::error::Error::custom(message)
}

fn matches(expected: &IndexModel, present: &IndexModel) -> bool {
    let expected_options = expected.options.clone().unwrap_or_default();
    let options = present.options.clone().unwrap_or_default();
    expected.keys.iter().eq(present.keys.iter())
        && expected_options
            .name
            .as_ref()
            .is_none_or(|name| options.name.as_ref() == Some(name))
        && expected_options.unique.unwrap_or(false) == options.unique.unwrap_or(false)
        && expected_options.sparse.unwrap_or(false) == options.sparse.unwrap_or(false)
        && expected_options.hidden.unwrap_or(false) == options.hidden.unwrap_or(false)
        && expected_options.expire_after == options.expire_after
        && expected_options.partial_filter_expression == options.partial_filter_expression
        && expected_options.collation.is_none()
        && options.collation.is_none()
}
fn normalize_names(models: &mut [IndexModel]) {
    for index in models {
        let options = index.options.get_or_insert_with(IndexOptions::default);
        if options.name.is_none() {
            options.name = Some(
                index
                    .keys
                    .iter()
                    .map(|(name, direction)| match direction {
                        Bson::String(direction) => format!("{name}_{direction}"),
                        _ => format!("{name}_{direction}"),
                    })
                    .collect::<Vec<_>>()
                    .join("_"),
            );
        }
    }
}
pub async fn indexes(
    db: &Database,
    name: &str,
    models: Vec<IndexModel>,
) -> mongodb::error::Result<()> {
    if models.is_empty() {
        return Ok(());
    }
    let mut models = models;
    normalize_names(&mut models);
    let descriptor = doc! {"createIndexes":name,"indexes":mongodb::bson::serialize_to_bson(&models).map_err(mongodb::error::Error::custom)?};
    if describing() {
        return record(descriptor);
    }
    let rows = db.collection::<Document>(name);
    if inspecting() {
        let actual: Vec<IndexModel> = match rows.list_indexes().max_time(IO).await {
            Ok(cursor) => cursor.try_collect().await?,
            Err(error) if matches!(error.kind.as_ref(),mongodb::error::ErrorKind::Command(command) if command.code==26) =>
            {
                missing();
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        for index in models {
            if actual.iter().any(|present| matches(&index, present)) {
                continue;
            }
            let name = index.options.as_ref().and_then(|o| o.name.as_ref());
            if actual.iter().any(|present| {
                present.keys.iter().eq(index.keys.iter())
                    || (name.is_some()
                        && name == present.options.as_ref().and_then(|o| o.name.as_ref()))
            }) {
                return Err(invalid("migration index definition drift"));
            }
            missing();
        }
        return Ok(());
    }
    effect(db, descriptor, async {
        rows.create_indexes(models.clone())
            .write_concern(majority())
            .await
    })
    .await?;
    if effects::active() {
        let actual: Vec<IndexModel> = rows
            .list_indexes()
            .max_time(IO)
            .await?
            .try_collect()
            .await?;
        if !models
            .iter()
            .all(|expected| actual.iter().any(|present| matches(expected, present)))
        {
            return Err(invalid("migration index postcondition unavailable"));
        }
        effects::check_fence().await?;
    }
    Ok(())
}

fn majority() -> WriteConcern {
    WriteConcern::builder().w(Acknowledgment::Majority).build()
}
/// Check a version marker before schema effects. Older recognized versions may
/// be converged; malformed and unsupported versions refuse before dispatch.
pub async fn marker(
    db: &Database,
    collection: &str,
    id: &str,
    field: &str,
    versions: &[i32],
    current: i32,
) -> mongodb::error::Result<bool> {
    marker_with_fields(db, collection, id, field, versions, current, doc! {}).await
}
pub async fn marker_with_fields(
    db: &Database,
    collection: &str,
    id: &str,
    field: &str,
    versions: &[i32],
    current: i32,
    fields: Document,
) -> mongodb::error::Result<bool> {
    marker_impl(db, collection, id, field, versions, current, fields, false).await
}
pub async fn marker_int32(
    db: &Database,
    collection: &str,
    id: &str,
    field: &str,
    versions: &[i32],
    current: i32,
) -> mongodb::error::Result<bool> {
    marker_impl(db, collection, id, field, versions, current, doc! {}, true).await
}
#[allow(clippy::too_many_arguments)]
async fn marker_impl(
    db: &Database,
    collection: &str,
    id: &str,
    field: &str,
    versions: &[i32],
    current: i32,
    fields: Document,
    strict_int32: bool,
) -> mongodb::error::Result<bool> {
    if describing() {
        record(
            doc! {"schema_marker":collection,"id":id,"field":field,"accepted":versions.to_vec(),"current":current,"fields":fields,"strict_int32":strict_int32},
        )?;
        return Ok(false);
    }
    let row = db
        .collection::<Document>(collection)
        .find_one(doc! {"_id":id})
        .read_concern(ReadConcern::majority())
        .max_time(IO)
        .await?;
    let ready = match row {
        None => false,
        Some(row) => {
            if fields
                .iter()
                .any(|(key, value)| row.get(key) != Some(value))
            {
                return Err(invalid("migration marker fields malformed"));
            }
            let version = row
                .get_i32(field)
                .ok()
                .or_else(|| {
                    if strict_int32 {
                        None
                    } else {
                        row.get_i64(field)
                            .ok()
                            .and_then(|value| i32::try_from(value).ok())
                    }
                })
                .ok_or_else(|| invalid("migration marker malformed"))?;
            if !versions.contains(&version) {
                return Err(invalid("unsupported prior schema version"));
            }
            version == current
        }
    };
    if inspecting() && !ready {
        missing();
    }
    Ok(ready)
}
/// The caller supplies the exact update and its postcondition independently.
pub async fn update(
    db: &Database,
    collection: &str,
    filter: Document,
    change: Document,
    upsert: bool,
    many: bool,
) -> mongodb::error::Result<()> {
    let descriptor = doc! {"update":collection,"updates":[{"q":filter.clone(),"u":change.clone(),"upsert":upsert,"multi":many}]};
    if describing() {
        return record(descriptor);
    }
    if inspecting() {
        return Ok(());
    }
    let rows = db.collection::<Document>(collection);
    effect(db, descriptor, async {
        if many {
            rows.update_many(filter, change)
                .upsert(upsert)
                .write_concern(majority())
                .await
        } else {
            rows.update_one(filter, change)
                .upsert(upsert)
                .write_concern(majority())
                .await
        }
    })
    .await?;
    Ok(())
}
pub async fn absent(
    db: &Database,
    collection: &str,
    filter: Document,
) -> mongodb::error::Result<()> {
    if describing() {
        return record(doc! {"absent":collection,"filter":filter});
    }
    if inspecting()
        && db
            .collection::<Document>(collection)
            .find_one(filter)
            .read_concern(ReadConcern::majority())
            .max_time(IO)
            .await?
            .is_some()
    {
        missing();
    }
    Ok(())
}

pub async fn exact_indexes(
    db: &Database,
    name: &str,
    mut required: Vec<IndexModel>,
    valid: impl Fn(&[IndexModel]) -> bool,
) -> mongodb::error::Result<()> {
    normalize_names(&mut required);
    if describing() {
        return record(
            doc! {"exact_indexes":name,"required":mongodb::bson::serialize_to_bson(&required).map_err(mongodb::error::Error::custom)?},
        );
    }
    let actual: Vec<IndexModel> = match db
        .collection::<Document>(name)
        .list_indexes()
        .max_time(IO)
        .await
    {
        Ok(cursor) => cursor.try_collect().await?,
        Err(error)
            if inspecting()
                && matches!(error.kind.as_ref(), mongodb::error::ErrorKind::Command(command) if command.code==26) =>
        {
            missing();
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    if !valid(&actual) {
        if inspecting() {
            let mut completed = actual.clone();
            for index in required {
                if !actual
                    .iter()
                    .any(|present| present.keys.iter().eq(index.keys.iter()))
                {
                    completed.push(index);
                }
            }
            if valid(&completed) {
                missing();
                return Ok(());
            }
        }
        return Err(invalid("migration index inventory drift"));
    }
    Ok(())
}

/// Reject legacy schema markers from a newer or different schema before
/// installing any shared marker validator.
pub async fn marker_history(
    db: &Database,
    collection: &str,
    allowed: Vec<Document>,
) -> mongodb::error::Result<()> {
    let filter = doc! {"$nor":allowed};
    if describing() {
        return record(doc! {"marker_history":collection,"filter":filter});
    }
    if db
        .collection::<Document>(collection)
        .find_one(filter)
        .read_concern(ReadConcern::majority())
        .max_time(IO)
        .await?
        .is_some()
    {
        return Err(invalid("unsupported legacy migration history"));
    }
    Ok(())
}
