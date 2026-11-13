use layer2_core::StoreError;
use mongodb::bson::doc;
use mongodb::options::IndexOptions;
use mongodb::{Collection, IndexModel};

use super::LaunchCollections;

pub(super) fn launch_indexes() -> Vec<IndexModel> {
    vec![
        named_index(
            doc! { "work_step": 1, "work_ready_at": 1, "work_lease_expires_at": 1, "blocked_reason": 1, "blocked_reason.type": 1 },
            "trigger_launches_observations",
        ),
        named_index(
            doc! { "owner": 1, "record.resolution.agent.id": 1, "created_at": 1, "_id": 1 },
            "trigger_launches_owned_agent_source",
        ),
        named_index(
            doc! { "owner": 1_i32, "_id": 1_i32 },
            "trigger_launches_owner_id",
        ),
        named_index(
            doc! { "trigger_id": 1_i32, "_id": 1_i32 },
            "trigger_launches_trigger_id",
        ),
        named_index(
            doc! { "owner": 1_i32, "created_at": -1_i32, "_id": -1_i32 },
            "trigger_launches_owner_created",
        ),
        named_index(
            doc! {
                "owner": 1_i32,
                "trigger_id": 1_i32,
                "created_at": -1_i32,
                "_id": -1_i32,
            },
            "trigger_launches_owner_trigger_created",
        ),
        named_index(
            doc! {
                "owner": 1_i32,
                "state": 1_i32,
                "created_at": -1_i32,
                "_id": -1_i32,
            },
            "trigger_launches_owner_state_created",
        ),
        named_index(
            doc! {
                "owner": 1_i32,
                "trigger_id": 1_i32,
                "state": 1_i32,
                "created_at": -1_i32,
                "_id": -1_i32,
            },
            "trigger_launches_owner_trigger_state_created",
        ),
        unique_index(
            doc! { "owner": 1_i32, "public_id": 1_i32, "command_key_hash": 1_i32 },
            "trigger_launches_owner_command",
        ),
        named_index(
            doc! {
                "blocked_reason": 1_i32,
                "blocked_reason.type": 1_i32,
                "work_step": 1_i32,
                "work_ready_at": 1_i32,
                "work_lease_expires_at": 1_i32,
                "_id": 1_i32,
            },
            "trigger_launches_due_work",
        ),
    ]
}

pub(super) fn event_indexes() -> Vec<IndexModel> {
    vec![unique_index(
        doc! { "launch_id": 1_i32, "seq": 1_i32 },
        "trigger_launch_events_launch_sequence",
    )]
}

pub(super) fn handoff_indexes() -> Vec<IndexModel> {
    vec![
        named_index(
            doc! { "owner": 1_i32, "_id": 1_i32 },
            "service_connection_handoffs_owner_id",
        ),
        unique_index(
            doc! { "owner": 1_i32, "launch_id": 1_i32, "intent_id": 1_i32 },
            "service_connection_handoffs_owner_intent",
        ),
    ]
}

pub(super) async fn ensure(collections: &LaunchCollections) -> Result<(), StoreError> {
    create(&collections.launches, launch_indexes(), "Trigger Launch").await?;
    create(&collections.events, event_indexes(), "Trigger Launch event").await?;
    create(
        &collections.handoffs,
        handoff_indexes(),
        "service connection handoff",
    )
    .await
}

async fn create(
    collection: &Collection<mongodb::bson::Document>,
    indexes: Vec<IndexModel>,
    context: &str,
) -> Result<(), StoreError> {
    collection
        .create_indexes(indexes)
        .await
        .map(|_| ())
        .map_err(|error| StoreError::Unavailable(format!("create {context} indexes: {error}")))
}

fn named_index(keys: mongodb::bson::Document, name: &str) -> IndexModel {
    IndexModel::builder()
        .keys(keys)
        .options(IndexOptions::builder().name(name.to_owned()).build())
        .build()
}

fn unique_index(keys: mongodb::bson::Document, name: &str) -> IndexModel {
    IndexModel::builder()
        .keys(keys)
        .options(
            IndexOptions::builder()
                .name(name.to_owned())
                .unique(true)
                .build(),
        )
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_and_event_indexes_are_unique() {
        for (indexes, name) in [
            (launch_indexes(), "trigger_launches_owner_command"),
            (event_indexes(), "trigger_launch_events_launch_sequence"),
            (
                handoff_indexes(),
                "service_connection_handoffs_owner_intent",
            ),
        ] {
            let index = indexes
                .iter()
                .find(|index| {
                    index
                        .options
                        .as_ref()
                        .and_then(|options| options.name.as_deref())
                        == Some(name)
                })
                .unwrap();
            assert_eq!(
                index.options.as_ref().and_then(|row| row.unique),
                Some(true)
            );
        }
    }

    #[test]
    fn due_index_has_deterministic_tie_breaking() {
        let index = launch_indexes()
            .into_iter()
            .find(|index| {
                index.options.as_ref().and_then(|row| row.name.as_deref())
                    == Some("trigger_launches_due_work")
            })
            .unwrap();
        assert_eq!(index.keys.get_i32("work_ready_at").unwrap(), 1);
        assert_eq!(index.keys.get_i32("_id").unwrap(), 1);
    }
}
