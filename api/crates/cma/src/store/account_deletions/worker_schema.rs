use crate::store::schema::index_contract_v1 as contract;
use layer2_core::account_deletion::worker::JournalError;
use mongodb::{
    bson::{doc, Document},
    Database,
};

pub(in crate::store) const WORK: &str = "account_erasure_work_v1";
pub(in crate::store) const EFFECTS: &str = "account_erasure_effects_v1";
pub(in crate::store) const COLLECTIONS: &[&str] = &[WORK, EFFECTS];

pub(in crate::store) fn indexes(collection: &str) -> Vec<Document> {
    match collection {
        WORK => vec![],
        EFFECTS => vec![
            doc! {"name":"erasure_operation_sequence_v1","key":{"deletion_id":1,"journal_sequence":1},"unique":true,"partialFilterExpression":{"kind":"operation"}},
            doc! {"name":"erasure_operation_history_v1","key":{"deletion_id":1,"kind":1,"journal_sequence":1}},
        ],
        _ => vec![],
    }
}

pub(in crate::store) async fn pending(
    database: &Database,
) -> Result<Vec<(&'static str, Document)>, &'static str> {
    let mut pending = vec![];
    for &collection in COLLECTIONS {
        match contract::inspect_collection(database, collection).await? {
            None => pending.push((collection, doc! {"kind":"collection"})),
            Some(existing) => {
                if existing
                    .iter()
                    .any(|index| index.contains_key("expireAfterSeconds"))
                {
                    return Err("active erasure evidence cannot have a TTL index");
                }
                for index in indexes(collection) {
                    for missing in contract::missing(&existing, &index)? {
                        pending.push((collection, missing));
                    }
                }
                let kinds = if collection == WORK {
                    vec!["checkpoint"]
                } else {
                    vec!["enrollment", "claim", "operation"]
                };
                let invalid = doc! {"$or":[{"schema":{"$ne":1_i64}},{"schema":{"$type":"array"}},{"kind":{"$nin":kinds}},{"kind":{"$type":"array"}}]};
                if database
                    .collection::<Document>(collection)
                    .find_one(invalid)
                    .max_time(std::time::Duration::from_secs(5))
                    .await
                    .map_err(|_| "erasure schema inspection unavailable")?
                    .is_some()
                {
                    return Err("erasure journal contains incompatible records");
                }
            }
        }
    }
    Ok(pending)
}

pub(super) async fn verify(database: &Database) -> Result<(), JournalError> {
    if !pending(database)
        .await
        .map_err(|_| JournalError::Incompatible)?
        .is_empty()
    {
        return Err(JournalError::Unprepared);
    }
    crate::store::migrations::account_erasure_work::prepared(database)
        .await
        .map_err(|_| JournalError::Unprepared)
}

#[cfg(test)]
pub(in crate::store) fn runtime_role(database: &str) -> Document {
    let mut role = crate::store::isolation::role(database);
    role.insert("createRole", "cma_am_erasure_journal");
    role.get_array_mut("privileges").expect("static runtime role").push(
        doc! {"resource":{"db":database,"collection":"agent_manager_migrations"},"actions":["find"]}.into()
    );
    role
}
