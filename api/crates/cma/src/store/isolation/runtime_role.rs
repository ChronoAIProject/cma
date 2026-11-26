use super::*;

const JOURNAL_ACTIONS: &[&str] = &["find", "insert", "update", "listIndexes"];

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
                && **name != USAGE_PROJECTIONS
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
        for &collection in JOURNAL_COLLECTIONS {
            grants.push(
                doc! {"resource":{"db":database,"collection":collection},"actions":JOURNAL_ACTIONS},
            );
        }
        grants.push(doc! {"resource":{"db":database,"collection":USAGE_PROJECTIONS},"actions":USAGE_PROJECTION_ACTIONS});
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

pub(super) fn validate_index_role(
    status: &Document,
    database: &str,
    metadata_collections: &[&str],
    migration_ledger: bool,
) -> Result<(), StoreError> {
    let denied = || {
        StoreError::Unavailable(
            "Agent Manager database credential is not isolated to its schema".into(),
        )
    };
    let info = status.get_document("authInfo").map_err(|_| denied())?;
    if info
        .get_array("authenticatedUsers")
        .map_err(|_| denied())?
        .len()
        != 1
    {
        return Err(denied());
    }
    let mut grants = BTreeMap::<String, BTreeSet<String>>::new();
    for grant in info
        .get_array("authenticatedUserPrivileges")
        .map_err(|_| denied())?
    {
        let grant = grant.as_document().ok_or_else(&denied)?;
        let resource = grant.get_document("resource").map_err(|_| denied())?;
        let collection = resource.get_str("collection").map_err(|_| denied())?;
        if resource.len() != 2
            || resource.get_str("db").ok() != Some(database)
            || (!collection.is_empty()
                && !COLLECTIONS.contains(&collection)
                && !replicas::COLLECTIONS.contains(&collection)
                && !JOURNAL_COLLECTIONS.contains(&collection)
                && collection != MIGRATION_COLLECTION
                && collection != super::provenance_progress::COLLECTION)
        {
            return Err(denied());
        }
        for action in grant.get_array("actions").map_err(|_| denied())? {
            let action = action.as_str().ok_or_else(&denied)?;
            if !(if collection.is_empty() {
                action == "listCollections"
            } else if collection == super::provenance_progress::COLLECTION {
                super::provenance_progress::ACTIONS.contains(&action)
            } else if collection == USAGE_PROJECTIONS {
                USAGE_PROJECTION_ACTIONS.contains(&action)
            } else if MEASURED_APPEND_ONLY.contains(&collection) {
                MEASURED_CUSTODY_ACTIONS.contains(&action)
            } else if collection == MEASURED_MANIFESTS {
                MEASURED_MANIFEST_ACTIONS.contains(&action)
            } else if collection == retained_consents::COLLECTION {
                retained_consents::ACTIONS.contains(&action)
            } else if JOURNAL_COLLECTIONS.contains(&collection) {
                JOURNAL_ACTIONS.contains(&action)
            } else if collection == MIGRATION_COLLECTION {
                if migration_ledger {
                    MIGRATION_ACTIONS.contains(&action)
                } else {
                    action == "find"
                }
            } else if metadata_collections.contains(&collection)
                && (ACTIONS.contains(&action) || ACTIVITY_INSPECTION_ACTIONS.contains(&action))
            {
                true
            } else {
                ACTIONS.contains(&action)
                    || (collection == "binding_conversations" && action == "dropIndex")
            }) {
                return Err(denied());
            }
            grants
                .entry(collection.into())
                .or_default()
                .insert(action.into());
        }
    }
    if grants
        .get(super::provenance_progress::COLLECTION)
        .is_some_and(|actions| {
            !super::provenance_progress::ACTIONS
                .iter()
                .all(|action| actions.contains(*action))
        })
    {
        return Err(denied());
    }
    if grants
        .get(retained_consents::COLLECTION)
        .is_some_and(|actions| {
            !retained_consents::ACTIONS
                .iter()
                .all(|action| actions.contains(*action))
        })
    {
        return Err(denied());
    }
    if grants.get(USAGE_PROJECTIONS).is_some_and(|actions| {
        !USAGE_PROJECTION_ACTIONS
            .iter()
            .all(|action| actions.contains(*action))
    }) {
        return Err(denied());
    }
    let journal_complete = JOURNAL_COLLECTIONS.iter().all(|name| {
        grants.get(*name).is_none_or(|actions| {
            JOURNAL_ACTIONS
                .iter()
                .all(|action| actions.contains(*action))
        })
    });
    if !journal_complete
        || !measured::complete(&grants)
        || !COLLECTIONS.iter().all(|name| {
            *name == retained_consents::COLLECTION
                || *name == MEASURED_MANIFESTS
                || *name == USAGE_PROJECTIONS
                || MEASURED_APPEND_ONLY.contains(name)
                || (OPTIONAL_COLLECTIONS.contains(name) && !grants.contains_key(*name))
                || ACTIONS
                    .iter()
                    .all(|action| grants.get(*name).is_some_and(|s| s.contains(*action)))
        })
        || !metadata_collections.iter().all(|collection| {
            grants.get(*collection).is_some_and(|actions| {
                ACTIVITY_INSPECTION_ACTIONS
                    .iter()
                    .all(|action| actions.contains(*action))
            })
        })
        || (migration_ledger
            && !grants.get(MIGRATION_COLLECTION).is_some_and(|actions| {
                MIGRATION_ACTIONS
                    .iter()
                    .all(|action| actions.contains(*action))
            }))
        || !grants
            .get("binding_conversations")
            .is_some_and(|actions| actions.contains("dropIndex"))
    {
        return Err(denied());
    }
    Ok(())
}
