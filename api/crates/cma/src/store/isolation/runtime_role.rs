use super::*;

pub(super) fn role_with_index_operation(
    database: &str,
    metadata_collections: &[&str],
    migration_ledger: bool,
    include_optional: bool,
) -> Document {
    let mut grants: Vec<_> = COLLECTIONS
        .iter()
        .filter(|name| include_optional || !OPTIONAL_COLLECTIONS.contains(name))
        .map(|name| {
            let mut actions = ACTIONS.to_vec();
            if *name == "binding_conversations" {
                actions.push("dropIndex");
            }
            if metadata_collections.contains(name) {
                actions.extend_from_slice(ACTIVITY_INSPECTION_ACTIONS);
            }
            doc! {"resource":{"db":database,"collection":*name},"actions":actions}
        })
        .collect();
    if include_optional {
        grants.push(doc! {
            "resource":{"db":database,"collection":MEASURED_CUSTODY},
            "actions":MEASURED_CUSTODY_ACTIONS,
        });
    }
    if migration_ledger {
        grants.push(
            doc! {"resource":{"db":database,"collection":MIGRATION_COLLECTION},"actions":MIGRATION_ACTIONS},
        );
    }
    grants.push(doc! {"resource":{"db":database,"collection":""},"actions":["listCollections"]});
    doc! {"createRole":"cma_am_runtime","privileges":grants,"roles":[]}
}
