use layer2_core::StoreError;
use mongodb::bson::doc;
use mongodb::options::IndexOptions;
use mongodb::{Collection, IndexModel};

use super::MongoTriggerDefinitionStore;

pub(super) fn definition_indexes() -> Vec<IndexModel> {
    let mut indexes = vec![
        named_index(
            doc! { "owner": 1_i32, "_id": 1_i32 },
            "trigger_definitions_owner_id",
        ),
        IndexModel::builder()
            .keys(doc! { "public_id": 1_i32 })
            .options(
                IndexOptions::builder()
                    .name(String::from("trigger_definitions_public_id"))
                    .unique(true)
                    .partial_filter_expression(doc! { "public_id": { "$type": "string" } })
                    .build(),
            )
            .build(),
    ];
    for (field, name) in [
        (
            "latest_sandbox_profile_id",
            "trigger_definitions_sandbox_guard",
        ),
        (
            "latest_workspace_profile_id",
            "trigger_definitions_workspace_guard",
        ),
        ("latest_agent_profile_id", "trigger_definitions_agent_guard"),
    ] {
        indexes.push(named_index(
            doc! { "availability": 1_i32, field: 1_i32, "_id": 1_i32 },
            name,
        ));
    }
    indexes
}

pub(super) fn revision_indexes() -> Vec<IndexModel> {
    let mut indexes = vec![
        IndexModel::builder()
            .keys(doc! { "trigger_id": 1_i32, "revision": 1_i32 })
            .options(
                IndexOptions::builder()
                    .name(String::from(
                        "trigger_definition_revisions_trigger_revision",
                    ))
                    .unique(true)
                    .build(),
            )
            .build(),
        named_index(
            doc! { "trigger_id": 1_i32, "record.digest": 1_i32, "revision": 1_i32 },
            "trigger_definition_revisions_trigger_digest",
        ),
    ];
    for (field, name) in [
        (
            "sandbox_profile_id",
            "trigger_definition_revisions_sandbox_guard",
        ),
        (
            "workspace_profile_id",
            "trigger_definition_revisions_workspace_guard",
        ),
        (
            "agent_profile_id",
            "trigger_definition_revisions_agent_guard",
        ),
    ] {
        indexes.push(named_index(
            doc! { field: 1_i32, "trigger_id": 1_i32, "revision": -1_i32 },
            name,
        ));
    }
    indexes
}

pub(super) async fn ensure(store: &MongoTriggerDefinitionStore) -> Result<(), StoreError> {
    create(
        &store.definitions,
        definition_indexes(),
        "Trigger definition",
    )
    .await?;
    create(
        &store.revisions,
        revision_indexes(),
        "Trigger definition revision",
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
        .map_err(|error| StoreError::Unavailable(format!("{context} indexes: {error}")))
}

fn named_index(keys: mongodb::bson::Document, name: &str) -> IndexModel {
    IndexModel::builder()
        .keys(keys)
        .options(IndexOptions::builder().name(name.to_owned()).build())
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_ids_are_unique_only_after_assignment() {
        let index = definition_indexes()
            .into_iter()
            .find(|index| {
                index
                    .options
                    .as_ref()
                    .and_then(|options| options.name.as_deref())
                    == Some("trigger_definitions_public_id")
            })
            .unwrap();
        let options = index.options.unwrap();
        assert_eq!(options.unique, Some(true));
        assert_eq!(
            options.partial_filter_expression,
            Some(doc! { "public_id": { "$type": "string" } })
        );
    }

    #[test]
    fn every_exact_profile_reference_has_a_guard_index() {
        for kind in ["sandbox", "workspace", "agent"] {
            let latest = format!("latest_{kind}_profile_id");
            let definition = definition_indexes()
                .into_iter()
                .find(|index| index.keys.contains_key(&latest))
                .unwrap();
            assert_eq!(definition.keys.get_i32("availability").unwrap(), 1);

            let reference = format!("{kind}_profile_id");
            let revision = revision_indexes()
                .into_iter()
                .find(|index| index.keys.contains_key(&reference))
                .unwrap();
            assert_eq!(revision.keys.get_i32(&reference).unwrap(), 1);
            assert_eq!(revision.keys.get_i32("trigger_id").unwrap(), 1);
        }
    }

    #[test]
    fn exact_revision_reads_have_a_deterministic_lookup_index() {
        let index = revision_indexes()
            .into_iter()
            .find(|index| {
                index
                    .options
                    .as_ref()
                    .and_then(|options| options.name.as_deref())
                    == Some("trigger_definition_revisions_trigger_digest")
            })
            .unwrap();
        assert_eq!(
            index.keys,
            doc! { "trigger_id": 1_i32, "record.digest": 1_i32, "revision": 1_i32 }
        );
        assert_eq!(index.options.unwrap().unique, None);
    }
}
