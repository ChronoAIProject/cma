//! Optional measured custody has exact, narrowly scoped action grants.
use super::{MEASURED_CUSTODY_ACTIONS, MEASURED_MANIFEST_ACTIONS};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const MEASURED_BATCHES: &str = crate::store::migrations::measured_batches::COLLECTION;
pub(super) const MEASURED_CUSTODY: &str = crate::store::migrations::measured_custody::COLLECTION;
pub(super) const MEASURED_BATCHES_V2: &str = crate::store::migrations::measured_index::BATCHES;
pub(super) const MEASURED_PROJECTIONS: &str = crate::store::migrations::measured_index::PROJECTIONS;
pub(super) const MEASURED_MANIFESTS: &str = crate::store::migrations::measured_index::MANIFESTS;
pub(super) const MEASURED_APPEND_ONLY: &[&str] = &[
    MEASURED_CUSTODY,
    MEASURED_BATCHES,
    MEASURED_PROJECTIONS,
    MEASURED_BATCHES_V2,
];

pub(super) fn complete(grants: &BTreeMap<String, BTreeSet<String>>) -> bool {
    MEASURED_APPEND_ONLY
        .iter()
        .copied()
        .chain([MEASURED_MANIFESTS])
        .all(|name| {
            let required = if name == MEASURED_MANIFESTS {
                MEASURED_MANIFEST_ACTIONS
            } else {
                MEASURED_CUSTODY_ACTIONS
            };
            grants
                .get(name)
                .is_none_or(|actions| required.iter().all(|action| actions.contains(*action)))
        })
}
