//! Account erasure policy for every collection in the service authority.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
pub(crate) enum InventorySet {
    Sandboxes,
    Workspaces,
    Worktrees,
    Agents,
    Launches,
    Bindings,
    Triggers,
    Documents,
    AgentProfiles,
    WorkspaceProfiles,
    SandboxProfiles,
    Reports,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum KeyRule {
    Subject,
    SubjectPrefix(&'static str),
    VisitorDay,
    RequestLimit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReferenceRule {
    Shares,
    Documents,
    TriggerRevisions,
    ActivityTargets,
    ShareReceipts,
    PlatformSettings,
    LogGates,
    Profiles,
    Identities,
    DiagnosticBindings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Erasure {
    Owner(&'static [&'static str]),
    Linked(InventorySet, &'static [&'static str]),
    LinkedKey(InventorySet, &'static str),
    OwnerKey(KeyRule),
    Reference(ReferenceRule),
    Custody(&'static [&'static str]),
    Sealed,
    ObjectAccounting,
    Retained(&'static str),
    Global,
    ProvenAbsent(&'static [&'static str]),
    /// Fail-closed for stores that keep identities inside serialized JSON:
    /// any string in these top-level JSON fields naming the subject or one of
    /// its erased resources, or an unreadable value, blocks erasure.
    ProvenAbsentJson(&'static [&'static str]),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Rule {
    pub erasure: Erasure,
    pub indexed: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Entry {
    pub collection: &'static str,
    pub rules: &'static [Rule],
}

#[path = "catalog_entries.rs"]
mod entries;
pub(crate) use entries::ERASURE;

pub(crate) fn validate_catalog() -> Result<(), &'static str> {
    use std::collections::BTreeSet;
    let expected: BTreeSet<_> = super::super::isolation::COLLECTIONS
        .iter()
        .chain(super::super::isolation::replicas::COLLECTIONS)
        .copied()
        .collect();
    let actual: BTreeSet<_> = ERASURE.iter().map(|entry| entry.collection).collect();
    if actual != expected || actual.len() != ERASURE.len() {
        return Err("account erasure catalog differs from collection authority");
    }
    if ERASURE.iter().any(|entry| entry.rules.is_empty()) {
        return Err("account erasure catalog contains an unclassified collection");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn catalog_covers_every_isolation_collection() {
        assert_eq!(super::validate_catalog(), Ok(()));
    }
}
