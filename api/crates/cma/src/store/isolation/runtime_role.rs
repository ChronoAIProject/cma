use super::*;

pub(super) fn role_with_index_operation(
    database: &str,
    metadata_collections: &[&str],
    migration_ledger: bool,
    include_optional: bool,
) -> Document {
    let mut grants: Vec<_> = COLLECTIONS
        .iter()
        .filter(|name| {
            !MEASURED_APPEND_ONLY.contains(name)
                && **name != retained_consents::COLLECTION
                && **name != MEASURED_MANIFESTS
        })
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
    if migration_ledger {
        grants.push(
            doc! {"resource":{"db":database,"collection":MIGRATION_COLLECTION},"actions":MIGRATION_ACTIONS},
        );
    }
    if include_optional {
        // Detached legal receipts have no account or Sandbox selector. Their
        // writer can create the namespace and append receipts only.
        grants.push(doc! {"resource":{"db":database,"collection":retained_consents::COLLECTION},"actions":retained_consents::ACTIONS});
        grants.push(doc! {"resource":{"db":database,"collection":MEASURED_MANIFESTS},"actions":MEASURED_MANIFEST_ACTIONS});
        for &collection in MEASURED_APPEND_ONLY {
            grants.push(doc! {"resource":{"db": database,"collection": collection},"actions": MEASURED_CUSTODY_ACTIONS});
        }
    }
    grants.push(doc! {"resource":{"db":database,"collection":""},"actions":["listCollections"]});
    doc! {"createRole":"cma_am_runtime","privileges":grants,"roles":[]}
}
