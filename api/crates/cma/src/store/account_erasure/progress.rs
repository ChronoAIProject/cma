//! Cohort identity, bounded cached lineage and durable lower-bound handoff.
use super::{
    catalog::{self, Entry, Erasure},
    context::Context,
    *,
};

pub(super) fn operations() -> Vec<(&'static Entry, bool)> {
    let mut entries: Vec<_> = catalog::ERASURE.iter().collect();
    entries.sort_by_key(|entry| rank(entry.collection));
    entries
        .into_iter()
        .flat_map(|entry| [true, false].map(move |indexed| (entry, indexed)))
        .filter(|(entry, indexed)| {
            entry
                .rules
                .iter()
                .any(|rule| rule.indexed == *indexed && executable(rule.erasure))
        })
        .collect()
}
fn rank(name: &str) -> u8 {
    match name {
        "trigger_definitions" => 0,
        "agents" => 2,
        "agent_read_observers_v1" => 3,
        "worktrees" => 3,
        "workspaces" => 4,
        "trigger_launches" | "cma_document_artifacts" | "sandbox_usage_reports" => 5,
        "sandbox_profile_revisions" => 6,
        "workspace_profile_revisions" => 7,
        "agent_profile_revisions" => 8,
        "agent_profiles" | "workspace_profiles" | "sandbox_profiles" => 9,
        // Historical attribution survives every ordinary descendant pass.
        "resource_policies"
        | "agent_creation_provenance"
        | "agent_lifecycle_operations"
        | "sandbox_metric_bindings"
        | "resource_shares"
        | "agent_reuse_bindings"
        | "trigger_launch_input_protocols"
        | "binding_conversations"
        | "sandbox_usage_report_control"
        | "native_prepared_sandboxes"
        | "provider_bindings"
        | "provider_binding_preparations"
        | "service_connection_handoffs" => 10,
        "sandbox_usage_events" => 11,
        "sandbox_billing_state" => 12,
        "sandbox_usage" => 13,
        "sandboxes" => 14,
        // Keep the erasure fence until all source and its Agent are gone.
        "agent_log_active_proofs_v1" => 15,
        _ => 1,
    }
}
fn executable(rule: Erasure) -> bool {
    matches!(
        rule,
        Erasure::Owner(_)
            | Erasure::Linked(..)
            | Erasure::LinkedKey(..)
            | Erasure::OwnerKey(_)
            | Erasure::Reference(_)
            | Erasure::ProvenAbsent(_)
            | Erasure::ProvenAbsentJson(_)
    )
}
pub(super) fn bind(cursor: &mut Cursor, operations: &[(&Entry, bool)]) {
    cursor.collection = operations
        .get(cursor.operation)
        .map(|(entry, _)| entry.collection.to_owned());
    cursor.indexed = operations
        .get(cursor.operation)
        .map(|(_, indexed)| *indexed);
}
pub(super) fn prepare(
    cursor: &mut Cursor,
    subjects: BTreeSet<String>,
    options: Options,
    operations: &[(&Entry, bool)],
) {
    let digest = layer2_core::document_artifacts::digest(
        format!(
            "account-record-effects-c2-qualified:{operations:?}:{:?}:{:?}:{}:{:?}{}",
            options.allocation_rollups_enabled,
            options.agent_shared_observation,
            options.transaction_drain_minimum_secs,
            options.drain_qualification,
            if options.retain_legal_consents {
                ":retain-legal-consents"
            } else {
                ""
            },
        )
        .as_bytes(),
    );
    let collection = operations
        .get(cursor.operation)
        .map(|(entry, _)| entry.collection);
    let indexed = operations
        .get(cursor.operation)
        .map(|(_, indexed)| *indexed);
    let cached_subjects: BTreeSet<_> = cursor
        .contexts
        .iter()
        .map(|context| context.subject.clone())
        .collect();
    if (!cached_subjects.is_empty() && cached_subjects != subjects)
        || cursor.subjects != subjects
        || cursor.catalog_digest != digest
        || cursor.billing_enabled != Some(options.billing_enabled)
        || cursor.collection.as_deref() != collection
        || cursor.indexed != indexed
        || cursor.operation > operations.len()
    {
        #[cfg(test)]
        let test_drain_seconds = cursor.test_drain_seconds;
        *cursor = Cursor {
            subjects,
            catalog_digest: digest,
            billing_enabled: Some(options.billing_enabled),
            #[cfg(test)]
            test_drain_seconds,
            ..Default::default()
        };
        bind(cursor, operations);
    }
}
pub(super) const MAX_CURSOR_BYTES: usize = 1024 * 1024;
const MAX_MEMBER_BYTES: usize = 768 * 1024;
pub(super) fn capacity(contexts: &[Context]) -> Result<(), Error> {
    for context in contexts {
        let ids = context.sets.values().flatten();
        let (count, bytes) = ids.fold((0_usize, 0_usize), |(count, bytes), id| {
            (count + 1, bytes.saturating_add(id.len()))
        });
        if count > 20_000 {
            return Err(Error::capacity(
                &context.subject,
                "personal resource IDs",
                20_000,
            ));
        }
        if bytes > 512 * 1024 {
            return Err(Error::capacity(
                &context.subject,
                "personal resource ID bytes",
                512 * 1024,
            ));
        }
        let snapshots = context
            .sandboxes
            .iter()
            .try_fold(0_usize, |sum, row| {
                mongodb::bson::serialize_to_vec(row).map(|bytes| sum.saturating_add(bytes.len()))
            })
            .map_err(|_| Error::InvalidRecord("Sandbox snapshot").attribute(&context.subject))?;
        if snapshots > super::context::MAX_LIVE_SNAPSHOT_BYTES {
            return Err(Error::capacity(
                &context.subject,
                "live Sandbox snapshot bytes",
                super::context::MAX_LIVE_SNAPSHOT_BYTES,
            ));
        }
    }
    Ok(())
}
/// Include the growing graph and retained pins when attributing cursor bounds.
pub(super) fn serialized_bytes<T: serde::Serialize>(value: &T) -> Result<usize, Error> {
    let json = serde_json::to_vec(value)
        .map_err(|error| Error::Store(error.to_string()))?
        .len();
    let bson = mongodb::bson::serialize_to_vec(value)
        .map_err(|error| Error::Store(error.to_string()))?
        .len();
    Ok(json.max(bson))
}
pub(super) fn contributions(
    cursor: &Cursor,
    contexts: &[Context],
) -> Result<BTreeMap<String, usize>, Error> {
    let mut sizes = BTreeMap::new();
    for subject in cursor
        .subjects
        .iter()
        .chain(contexts.iter().map(|c| &c.subject))
    {
        if sizes.contains_key(subject) {
            continue;
        }
        let context = contexts.iter().find(|context| context.subject == *subject);
        let mut bytes = context.map(serialized_bytes).transpose()?.unwrap_or(0);
        if let Some(graph) = &cursor.profile_discovery {
            bytes = bytes.saturating_add(graph.contribution(subject)?);
        }
        #[derive(serde::Serialize)]
        struct Retained<'a> {
            pins: Vec<&'a String>,
            owners: BTreeMap<&'a String, &'a String>,
        }
        let belongs = |pin: &str| {
            cursor
                .retained_profile_owners
                .get(pin.rsplit_once(':').map_or(pin, |(id, _)| id))
                .is_some_and(|owner| owner == subject)
        };
        bytes = bytes.saturating_add(serialized_bytes(&Retained {
            pins: cursor
                .retained_profiles
                .iter()
                .flatten()
                .filter(|pin| belongs(pin))
                .collect(),
            owners: cursor
                .retained_profile_owners
                .iter()
                .filter(|(_, owner)| *owner == subject)
                .collect(),
        })?);
        sizes.insert(subject.clone(), bytes);
    }
    Ok(sizes)
}
fn check_capacity(cursor: &Cursor, contexts: &[Context], bytes: usize) -> Result<(), Error> {
    let sizes = contributions(cursor, contexts)?;
    for (subject, bytes) in &sizes {
        if *bytes > MAX_MEMBER_BYTES {
            return Err(Error::capacity(
                subject,
                "serialized member state bytes",
                MAX_MEMBER_BYTES,
            ));
        }
    }
    if bytes <= MAX_CURSOR_BYTES {
        return Ok(());
    }
    let subject = cursor
        .capacity_subject
        .as_ref()
        .filter(|subject| sizes.contains_key(*subject))
        .or_else(|| {
            sizes
                .iter()
                .max_by_key(|(_, bytes)| *bytes)
                .map(|(subject, _)| subject)
        })
        .ok_or_else(|| Error::Store("cursor metadata exceeds its bounded size".into()))?;
    Err(Error::capacity(
        subject,
        "serialized cohort cursor bytes",
        MAX_CURSOR_BYTES,
    ))
}
/// Error paths release oversized state while preserving the original cause.
pub(super) fn cursor_capacity(cursor: &mut Cursor) -> Result<(), Error> {
    let bytes = serialized_bytes(cursor)?;
    let result = check_capacity(cursor, &cursor.contexts, bytes);
    if bytes > MAX_CURSOR_BYTES {
        *cursor = Cursor::default();
    }
    result
}
pub(super) fn handoff(contexts: &[Context], targets: &[Target<'_>], batch: &mut Batch) {
    for context in contexts {
        let Some(target) = targets
            .iter()
            .find(|target| target.subject.as_str() == context.subject)
        else {
            continue;
        };
        let discovered = context.inventory();
        let mut delta = ErasureInventory::default();
        macro_rules! missing {
            ($($field:ident),+) => { $(delta.$field = discovered.$field.into_iter().filter(|id| !target.inventory.$field.contains(id)).collect();)+ };
        }
        missing!(sandboxes, workspaces, worktrees, agents, launches, bindings, triggers, documents);
        if delta != ErasureInventory::default() {
            batch.discovered.insert(context.subject.clone(), delta);
            batch.waiting = true;
            batch.has_more = true;
        }
    }
}
#[cfg(test)]
pub(super) fn acknowledge(inventory: &mut ErasureInventory, batch: &Batch, subject: &str) {
    if let Some(delta) = batch.discovered.get(subject) {
        macro_rules! merge { ($($field:ident),+) => { $(for id in &delta.$field { if !inventory.$field.contains(id) {inventory.$field.push(id.clone());}})+ }; }
        merge!(sandboxes, workspaces, worktrees, agents, launches, bindings, triggers, documents);
    }
}

/// Check the prospective cache before returning discovered inventory or effects.
pub(super) fn cached_capacity(cursor: &Cursor, contexts: &[Context]) -> Result<(), Error> {
    // During a call the live Contexts are held outside the serialized cursor.
    // Charging the larger encoding for each part is a conservative bound for
    // either persistence format, with no duplicate snapshots or graph clone.
    let mut bytes = serialized_bytes(cursor)?;
    for context in contexts {
        bytes = bytes.saturating_add(serialized_bytes(context)?);
    }
    check_capacity(cursor, contexts, bytes)
}
