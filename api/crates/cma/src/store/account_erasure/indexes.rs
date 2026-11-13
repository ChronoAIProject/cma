//! Discovery indexes installed before a cohort performs ownership queries.
use super::Error;
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, Document},
    options::IndexOptions,
    Database, IndexModel,
};
use std::collections::BTreeMap;

pub(super) async fn ensure(db: &Database) -> Result<(), Error> {
    let mut fields: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (name, field) in super::context::history_indexes() {
        fields.entry(name).or_default().push(field);
    }
    for (name, field) in [
        ("sandboxes", "owner"),
        (
            "resource_policies",
            "record.organization_evidence.admitted_by.subject",
        ),
        ("agent_creation_provenance", "record.created_by.subject"),
        (
            "agent_creation_provenance",
            "record.creation_origin.launching_actor.subject",
        ),
        ("agent_creation_provenance", "record.sandbox_id"),
        ("binding_conversations", "binding_id"),
        ("binding_conversations", "record.binding_id"),
        ("sandbox_usage_events", "sandbox_id"),
        ("trigger_definition_revisions", "owner"),
        ("trigger_definition_revisions", "trigger_id"),
        ("agent_profile_revisions", "profile_id"),
        ("workspace_profile_revisions", "profile_id"),
        ("sandbox_profile_revisions", "profile_id"),
    ] {
        fields.entry(name).or_default().push(field);
    }
    for name in ["workspaces", "worktrees", "agents"] {
        for field in ["sandbox_id", "workspace_id", "worktree_id"] {
            fields.entry(name).or_default().push(field);
        }
    }
    for (name, mut paths) in fields {
        paths.sort_unstable();
        paths.dedup();
        let collection = db.collection::<Document>(name);
        let exists = db
            .list_collection_names()
            .filter(doc! {"name":name})
            .await?
            .iter()
            .any(|n| n == name);
        let existing = if exists {
            collection
                .list_indexes()
                .await?
                .try_collect::<Vec<_>>()
                .await?
        } else {
            vec![]
        };
        for path in paths {
            if path == "_id"
                || existing.iter().any(|index| {
                    index.keys.keys().next().is_some_and(|key| key == path)
                        && !index.options.as_ref().is_some_and(|options| {
                            options.sparse == Some(true)
                                || options.partial_filter_expression.is_some()
                        })
                })
            {
                continue;
            }
            let mut index = IndexModel::builder().keys(doc! {path:1}).build();
            // These full-child indexes also belong to ordinary store startup.
            // A different name for the same key would prevent later startup.
            if path == "sandbox_id" && matches!(name, "agents" | "workspaces" | "worktrees") {
                index.options = Some(
                    IndexOptions::builder()
                        .name(format!("{name}_sandbox"))
                        .build(),
                );
            }
            collection.create_index(index).await?;
        }
        if matches!(
            name,
            "agent_profiles" | "workspace_profiles" | "sandbox_profiles"
        ) && !existing.iter().any(|index| {
            index.keys == doc! {"owner":1,"_id":1}
                && !index.options.as_ref().is_some_and(|options| {
                    options.sparse == Some(true) || options.partial_filter_expression.is_some()
                })
        }) {
            collection
                .create_index(IndexModel::builder().keys(doc! {"owner":1,"_id":1}).build())
                .await?;
        }
    }
    ensure_effect_keysets(db).await?;
    Ok(())
}

async fn ensure_effect_keysets(db: &Database) -> Result<(), Error> {
    for entry in super::catalog::ERASURE {
        let fields = super::indexed::fields(entry);
        if fields.iter().all(|field| *field == "_id") {
            continue;
        }
        let collection = db.collection::<Document>(entry.collection);
        let specification = db
            .list_collections()
            .filter(doc! {"name":entry.collection})
            .await?
            .try_next()
            .await?;
        let exists = specification.is_some();
        // Versioned log carriers have explicit, complete DDL. Never introduce
        // an absent carrier or modify a validated carrier during migration.
        if super::indexed::logs::managed(entry.collection)
            && specification
                .as_ref()
                .is_none_or(|spec| spec.options.validator.is_some())
        {
            continue;
        }
        let mut existing = if exists {
            collection
                .list_indexes()
                .await?
                .try_collect::<Vec<_>>()
                .await?
        } else {
            vec![]
        };
        if super::indexed::logs::installed(entry.collection, &existing) {
            continue;
        }
        for field in fields.into_iter().filter(|field| *field != "_id") {
            let keys = doc! {field:1,"_id":1};
            if existing.iter().any(|index| {
                index.keys == keys
                    && !index.options.as_ref().is_some_and(|options| {
                        options.sparse == Some(true) || options.partial_filter_expression.is_some()
                    })
            }) {
                continue;
            }
            let index = IndexModel::builder().keys(keys).build();
            collection.create_index(index.clone()).await?;
            existing.push(index);
        }
    }
    Ok(())
}
