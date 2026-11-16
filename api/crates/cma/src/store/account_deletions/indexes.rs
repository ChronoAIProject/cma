use super::*;
use mongodb::{bson::doc, options::IndexOptions, IndexModel};

pub(super) async fn ensure(store: &MongoAccountDeletionStore) -> Result<(), mongodb::error::Error> {
    let index = |keys, options| IndexModel::builder().keys(keys).options(options).build();
    store
        .records
        .create_indexes([
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
        ])
        .await?;
    store
        .events
        .create_indexes([
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
        ])
        .await?;
    Ok(())
}
