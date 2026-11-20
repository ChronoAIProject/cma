use super::*;
use mongodb::{bson::doc, options::IndexOptions, IndexModel};

fn definitions() -> [(&'static str, Vec<IndexModel>); 2] {
    let index = |keys, options| IndexModel::builder().keys(keys).options(options).build();
    [
        (
            RECORDS,
            vec![
                index(
                    doc! {"active_subject_hash":1},
                    IndexOptions::builder()
                        .name("account_deletions_active_subject".to_owned())
                        .unique(true)
                        .partial_filter_expression(doc! {"active_subject_hash":{"$exists":true}})
                        .build(),
                ),
                index(
                    doc! {"subject_hash":1,"requested_at":-1},
                    IndexOptions::builder()
                        .name("account_deletions_subject_history".to_owned())
                        .build(),
                ),
                index(
                    doc! {"status":1,"subject_hash":1},
                    IndexOptions::builder()
                        .name("account_deletions_status_subject".to_owned())
                        .build(),
                ),
                index(
                    doc! {"subject_hash":1},
                    IndexOptions::builder()
                        .name("account_deletions_array_status_subject".to_owned())
                        .partial_filter_expression(doc! {"status":{"$type":"array"}})
                        .build(),
                ),
                index(
                    doc! {"subject_hash":1,"request_keys":1},
                    IndexOptions::builder()
                        .name("account_deletions_request_keys".to_owned())
                        .unique(true)
                        .build(),
                ),
                index(
                    doc! {"status":1,"work.ready_at":1},
                    IndexOptions::builder()
                        .name("account_deletions_due".to_owned())
                        .build(),
                ),
                index(
                    doc! {"expire_at":1},
                    IndexOptions::builder()
                        .name("account_deletions_expiry".to_owned())
                        .expire_after(Duration::ZERO)
                        .build(),
                ),
            ],
        ),
        (
            EVENTS,
            vec![
                index(
                    doc! {"deletion_id":1,"seq":1},
                    IndexOptions::builder()
                        .name("account_deletion_events_sequence".to_owned())
                        .unique(true)
                        .build(),
                ),
                index(
                    doc! {"expire_at":1},
                    IndexOptions::builder()
                        .name("account_deletion_events_expiry".to_owned())
                        .expire_after(Duration::ZERO)
                        .build(),
                ),
            ],
        ),
    ]
}

#[cfg(test)]
pub(super) async fn ensure(store: &MongoAccountDeletionStore) -> Result<(), mongodb::error::Error> {
    for (name, indexes) in definitions() {
        store
            .database
            .collection::<Document>(name)
            .create_indexes(indexes)
            .await?;
    }
    Ok(())
}

pub(super) async fn verify(store: &MongoAccountDeletionStore) -> Result<(), StoreError> {
    use crate::store::schema::index_contract_v1::{inspect_collection, missing};
    for (name, indexes) in definitions() {
        let existing = inspect_collection(&store.database, name)
            .await
            .map_err(|_| {
                StoreError::Unavailable("account deletion schema inspection failed".into())
            })?
            .ok_or_else(|| {
                StoreError::Unavailable("account deletion collection is not prepared".into())
            })?;
        for index in indexes {
            let desired = mongodb::bson::serialize_to_document(&index)
                .map_err(|error| StoreError::Unavailable(error.to_string()))?;
            if !missing(&existing, &desired)
                .map_err(|error| StoreError::Unavailable(error.into()))?
                .is_empty()
            {
                return Err(StoreError::Unavailable(
                    "account deletion index is not prepared".into(),
                ));
            }
        }
    }
    crate::store::schema::verify_existing_collections(&store.database, &[RECORDS, EVENTS]).await
}
