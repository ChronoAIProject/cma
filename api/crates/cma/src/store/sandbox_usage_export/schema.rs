//! Versioned additive schema, inspected at opt-in and prepared only by the migrator.
use super::*;
use crate::store::schema::index_contract_v1::{inspect_collection, missing};

pub(in crate::store) const MIGRATION: &str = "am-sandbox-usage-capture-v1";
pub(in crate::store) const MAX_STEPS: i64 = 12;

fn indexes(name: &str) -> Vec<Document> {
    match name {
        STREAMS => vec![doc! {"name":"usage_timeline_v1", "key":{
        "namespace":1, "sandbox_id":1, "created_at":1}, "unique":true}],
        CAPTURES => vec![
            doc! {"name":"usage_capture_order_v1", "key":{"stream_id":1,"revision":1},"unique":true},
            doc! {"name":"usage_pending_capture_v1", "key":{"complete":1,"_id":1}},
        ],
        EVIDENCE => vec![doc! {"name":"usage_source_part_v1", "key":{
        "stream_id":1,"source_id":1,"part":1},"unique":true}],
        OUTBOX => vec![
            doc! {"name":"usage_sequence_v1", "key":{"stream_id":1,"sequence":1},"unique":true},
            doc! {"name":"usage_source_delivery_v1", "key":{
            "stream_id":1,"source_id":1,"part":1},"unique":true},
        ],
        _ => vec![],
    }
}

enum Change {
    Collection(&'static str),
    Index(&'static str, Document),
}

async fn changes(database: &Database) -> Result<Vec<Change>, &'static str> {
    let mut pending = Vec::new();
    for &name in COLLECTIONS {
        let actual = inspect_collection(database, name)
            .await
            .map_err(<&str>::from)?;
        let Some(actual) = actual else {
            pending.push(Change::Collection(name));
            pending.extend(
                indexes(name)
                    .into_iter()
                    .map(|index| Change::Index(name, index)),
            );
            continue;
        };
        // Financial evidence must never acquire an automatic expiration policy.
        if actual
            .iter()
            .any(|index| index.contains_key("expireAfterSeconds"))
        {
            return Err("usage evidence TTL is forbidden");
        }
        let expected = indexes(name);
        if actual.iter().any(|index| {
            index.get_str("name").ok() != Some("_id_")
                && !expected
                    .iter()
                    .any(|wanted| wanted.get("name") == index.get("name"))
        }) {
            return Err("usage evidence has an unreviewed index");
        }
        for index in indexes(name) {
            pending.extend(
                missing(&actual, &index)?
                    .into_iter()
                    .map(|index| Change::Index(name, index)),
            );
        }
    }
    Ok(pending)
}

pub(in crate::store) async fn pending(database: &Database) -> Result<usize, &'static str> {
    let pending = changes(database).await?.len();
    crate::store::schema::index_contract_v1::require_binding_versions(database, COLLECTIONS)
        .await?;
    Ok(pending)
}

pub(in crate::store) async fn apply_next(database: &Database) -> Result<bool, &'static str> {
    let Some(change) = changes(database).await?.into_iter().next() else {
        return Ok(false);
    };
    let mut command = match change {
        Change::Collection(name) => doc! {"create":name},
        Change::Index(name, index) => doc! {"createIndexes":name,"indexes":[index]},
    };
    command.insert("maxTimeMS", 10_000);
    command.insert("writeConcern", doc! {"w":"majority","wtimeout":5000});
    let reply = database
        .run_command(command)
        .await
        .map_err(|_| "usage schema write outcome uncertain")?;
    if reply.contains_key("writeConcernError") {
        return Err("usage schema acknowledgement uncertain");
    }
    Ok(true)
}
