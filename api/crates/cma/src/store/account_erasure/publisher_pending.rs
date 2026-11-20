//! Temporary C1 refusal until C2 installs the publisher retention protocol.
//!
//! These checks only read current ownership. In particular, a Profile revision
//! has no owner projection: the current aggregate establishes its ownership.
use super::{Error, Target};
use crate::store::durability::majority_collection;
use layer2_core::account_deletion::ErasureBlock;
use mongodb::{
    bson::{doc, Document},
    Database,
};
use std::collections::BTreeSet;

const OWNED: &[&str] = &[
    "agent_profiles",
    "workspace_profiles",
    "sandbox_profiles",
    "trigger_definitions",
    "trigger_definition_revisions",
];

fn pending(subject: &str, collection: &'static str) -> Error {
    Error::blocked(
        subject,
        ErasureBlock::RetainedReferencePending {
            collection: collection.into(),
        },
    )
}

async fn owned(
    db: &Database,
    collection: &'static str,
    mut filter: Document,
    subjects: &BTreeSet<String>,
) -> Result<Option<String>, Error> {
    if subjects.is_empty() {
        return Ok(None);
    }
    let owners: Vec<_> = subjects.iter().cloned().collect();
    filter.insert("owner", doc! {"$in":owners});
    let Some(row) = majority_collection::<Document>(db, collection)
        .find_one(filter)
        .projection(doc! {"owner":1})
        .await?
    else {
        return Ok(None);
    };
    let observed = super::selectors::strings(&row, "owner");
    subjects
        .iter()
        .find(|subject| observed.contains(&subject.as_str()))
        .cloned()
        .map(Some)
        .ok_or(Error::InvalidRecord(collection))
}

/// Run on every invocation before inventory handoff, debt or record effects.
///
/// Rejecting every owned aggregate also protects all of its immutable Profile
/// revisions. The gate deliberately refuses unused authoring data as well:
/// deciding that no survivor pin exists requires C2's fence and drain protocol.
pub(super) async fn preflight(db: &Database, targets: &[Target<'_>]) -> Result<(), Error> {
    let subjects: BTreeSet<_> = targets
        .iter()
        .map(|target| target.subject.as_str().to_owned())
        .collect();
    for collection in OWNED {
        if let Some(subject) = owned(db, collection, doc! {}, &subjects).await? {
            return Err(pending(&subject, collection));
        }
    }
    Ok(())
}

fn aggregate(collection: &str) -> Option<&'static str> {
    match collection {
        "agent_profile_revisions" => Some("agent_profiles"),
        "workspace_profile_revisions" => Some("workspace_profiles"),
        "sandbox_profile_revisions" => Some("sandbox_profiles"),
        _ => None,
    }
}

pub(super) fn protected_collection(name: &str) -> bool {
    OWNED.contains(&name) || aggregate(name).is_some()
}

/// Defensive operation guard; call before destructive or reference dispatch.
///
/// Reread the selected document and its parent instead of treating a cursor's
/// cached identity or a stale selected owner as current ownership authority.
pub(super) async fn guard_row(
    db: &Database,
    collection: &'static str,
    row: &Document,
    subjects: &BTreeSet<String>,
) -> Result<(), Error> {
    if !protected_collection(collection) {
        return Ok(());
    }
    let id = row.get("_id").ok_or(Error::InvalidRecord(collection))?;
    if OWNED.contains(&collection) {
        if let Some(subject) = owned(db, collection, doc! {"_id":id.clone()}, subjects).await? {
            return Err(pending(&subject, collection));
        }
        return Ok(());
    }
    let parent = aggregate(collection).ok_or(Error::InvalidRecord(collection))?;
    let Some(current) = majority_collection::<Document>(db, collection)
        .find_one(doc! {"_id":id.clone()})
        .projection(doc! {"profile_id":1,"record.profile_id":1})
        .await?
    else {
        return Ok(());
    };
    // Any persisted identity can reveal an owned damaged revision; disagreeing
    // denormalized fields must not hide a revision from this refusal boundary.
    let mut candidates = BTreeSet::new();
    for path in ["profile_id", "record.profile_id"] {
        candidates.extend(
            super::selectors::strings(&current, path)
                .into_iter()
                .map(str::to_owned),
        );
    }
    if let Some((profile, _)) = current
        .get_str("_id")
        .ok()
        .and_then(|key| key.rsplit_once(':'))
    {
        candidates.insert(profile.to_owned());
    }
    if !candidates.is_empty() {
        let ids: Vec<_> = candidates.into_iter().collect();
        if let Some(subject) = owned(db, parent, doc! {"_id":{"$in":ids}}, subjects).await? {
            return Err(pending(&subject, collection));
        }
    }
    Ok(())
}
