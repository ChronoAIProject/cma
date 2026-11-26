//! Bounded ownership discovery cached for one resumable cohort.
use super::{catalog::InventorySet as Set, Error, Target};
use futures_util::TryStreamExt;
use layer2_core::account_deletion::ErasureInventory;
use mongodb::{
    bson::{doc, Bson, Document},
    Database,
};
use std::collections::{BTreeMap, BTreeSet};

#[path = "history.rs"]
mod history;
pub(super) use history::discover_usage_projections;
#[path = "context_inventory.rs"]
mod inventory;
const PAGE: i64 = 500;
const MAX_IDS: usize = 20_000;
const MAX_ID_BYTES: usize = 512 * 1024;
const MAX_SNAPSHOT_BYTES: usize = 1024 * 1024;
pub(super) const MAX_LIVE_SNAPSHOT_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(super) struct Context {
    pub subject: String,
    pub sets: BTreeMap<Set, BTreeSet<String>>,
    pub protected: BTreeSet<String>,
    #[serde(default)]
    pub organization_sandboxes: BTreeSet<String>,
    protected_sets: BTreeMap<Set, BTreeSet<String>>,
    // The cursor stores resource IDs, never stale lease/key authority snapshots.
    #[serde(skip)]
    pub sandboxes: Vec<Document>,
    /// Share IDs on personal Sandboxes, re-read on every refresh and never
    /// persisted. Resource-execution evidence names shares only by ID (#3296).
    #[serde(skip)]
    pub shares: BTreeSet<String>,
    /// Deterministic execution digests naming the subject, recomputed with the
    /// shares on every refresh and never persisted (#3296).
    #[serde(skip)]
    pub execution_digests: BTreeSet<String>,
}
impl Context {
    pub fn ids(&self, set: Set) -> Vec<String> {
        self.sets.get(&set).into_iter().flatten().cloned().collect()
    }
    pub fn inventory(&self) -> ErasureInventory {
        ErasureInventory {
            sandboxes: self.ids(Set::Sandboxes),
            workspaces: self.ids(Set::Workspaces),
            worktrees: self.ids(Set::Worktrees),
            agents: self.ids(Set::Agents),
            launches: self.ids(Set::Launches),
            bindings: self.ids(Set::Bindings),
            triggers: self.ids(Set::Triggers),
            documents: self.ids(Set::Documents),
            ..Default::default()
        }
    }
    pub fn protects(&self, row: &Document) -> bool {
        if super::identity_activity::sandbox_target(row)
            .is_some_and(|id| self.organization_sandboxes.contains(id))
        {
            return true;
        }
        super::selectors::PROTECTION_PATHS.iter().any(|path| {
            super::selectors::strings(row, path)
                .iter()
                .any(|id| self.protected.contains(*id))
        })
    }
    fn add(&mut self, set: Set, id: &str) -> Result<(), Error> {
        if id.is_empty() || id.len() > 4096 {
            return Err(
                Error::InvalidRecord("historical resource identity").attribute(&self.subject)
            );
        }
        self.sets.entry(set).or_default().insert(id.to_owned());
        Ok(())
    }
    fn protect(&mut self, set: Set, id: &str) {
        self.protected.insert(id.to_owned());
        // Only roots are revalidated. Descendants are vetoed by their cached
        // identity without a duplicate cache or organization subtree traversal.
        if set == Set::Sandboxes {
            self.protected_sets
                .entry(set)
                .or_default()
                .insert(id.to_owned());
        }
    }
    fn bounds(&self) -> Result<(), Error> {
        let values = self.sets.values();
        let (count, bytes) = values.flatten().fold((0_usize, 0_usize), |(n, bytes), id| {
            (n + 1, bytes + id.len())
        });
        if count > MAX_IDS {
            return Err(Error::capacity(
                &self.subject,
                "personal resource IDs",
                MAX_IDS,
            ));
        }
        if bytes > MAX_ID_BYTES {
            return Err(Error::capacity(
                &self.subject,
                "personal resource ID bytes",
                MAX_ID_BYTES,
            ));
        }
        Ok(())
    }
    fn exclude_protected(&mut self) {
        for values in self.sets.values_mut() {
            values.retain(|id| !self.protected.contains(id));
        }
    }
    /// New durable inventory is a lower bound even after earlier pages finished.
    pub async fn merge_inventory(
        &mut self,
        db: &Database,
        inventory: &ErasureInventory,
    ) -> Result<bool, Error> {
        let changed = inventory::merge(db, self, inventory).await?;
        if changed {
            self.refresh(db).await?;
            descendants(db, &mut self.sets, &self.subject).await?;
            self.refresh(db).await?;
        }
        self.bounds()?;
        Ok(changed)
    }
    /// Revalidate known authority without repeating owner/history discovery.
    pub async fn refresh(&mut self, db: &Database) -> Result<(), Error> {
        for (name, set, owner_path) in [
            ("agents", Set::Agents, "owner"),
            ("workspaces", Set::Workspaces, "owner"),
            ("worktrees", Set::Worktrees, "owner"),
            ("trigger_launches", Set::Launches, "owner"),
            ("provider_bindings", Set::Bindings, "record.owner"),
            ("trigger_definitions", Set::Triggers, "owner"),
            ("cma_document_artifacts", Set::Documents, "owner_subject"),
        ] {
            let ids = self.ids(set);
            if ids.is_empty() {
                continue;
            }
            for row in rows(db, name, doc! {"_id":{"$in":ids}}, &self.subject).await? {
                let mut owner = super::selectors::strings(&row, owner_path);
                if set == Set::Documents {
                    owner.extend(super::selectors::strings(&row, "audit_owner_subject"));
                }
                let foreign = owner.iter().any(|owner| *owner != self.subject);
                if foreign || (owner.is_empty() && matches!(set, Set::Launches | Set::Bindings)) {
                    if let Ok(id) = row.get_str("_id") {
                        self.protect(set, id);
                    }
                    for (kind, path) in [
                        (Set::Sandboxes, "sandbox_id"),
                        (Set::Sandboxes, "record.sandbox_id"),
                        (Set::Workspaces, "workspace_id"),
                        (Set::Worktrees, "worktree_id"),
                    ] {
                        for id in super::selectors::strings(&row, path) {
                            self.protect(kind, id);
                        }
                    }
                } else {
                    for path in ["sandbox_id", "record.sandbox_id"] {
                        for id in super::selectors::strings(&row, path) {
                            self.add(Set::Sandboxes, id)?;
                        }
                    }
                }
            }
            self.bounds()?;
        }
        // Sidecar ownership survives the aggregate's ordinary purge. It vetoes
        // stale inventory exactly as a live root does.
        for (name, set, field, owner_path) in [
            (
                "trigger_launch_input_protocols",
                Set::Launches,
                "_id",
                "owner",
            ),
            (
                "binding_conversations",
                Set::Bindings,
                "binding_id",
                "record.owner",
            ),
        ] {
            let ids = self.ids(set);
            if ids.is_empty() {
                continue;
            }
            let filter = if set == Set::Bindings {
                doc! {"$or":[{"binding_id":{"$in":&ids}},{"record.binding_id":{"$in":&ids}}]}
            } else {
                doc! {field:{"$in":&ids}}
            };
            for row in rows(db, name, filter, &self.subject).await? {
                if super::selectors::strings(&row, owner_path)
                    .iter()
                    .any(|owner| *owner != self.subject)
                {
                    for path in if set == Set::Bindings {
                        vec!["binding_id", "record.binding_id"]
                    } else {
                        vec!["_id"]
                    } {
                        for id in super::selectors::strings(&row, path) {
                            self.protect(set, id);
                        }
                    }
                    for (kind, path) in [
                        (Set::Sandboxes, "sandbox_id"),
                        (Set::Sandboxes, "record.sandbox_id"),
                        (Set::Agents, "agent_id"),
                        (Set::Agents, "record.agent_id"),
                        (Set::Workspaces, "record.workspace_id"),
                        (Set::Worktrees, "record.worktree_id"),
                    ] {
                        for id in super::selectors::strings(&row, path) {
                            self.protect(kind, id);
                        }
                    }
                }
            }
        }
        let candidates: BTreeSet<_> = self
            .ids(Set::Sandboxes)
            .into_iter()
            .chain(
                self.protected_sets
                    .get(&Set::Sandboxes)
                    .into_iter()
                    .flatten()
                    .cloned(),
            )
            .collect();
        if !candidates.is_empty() {
            let ids: Vec<_> = candidates.into_iter().collect();
            for row in rows(
                db,
                "resource_policies",
                doc! {"_id":{"$in":&ids}},
                &self.subject,
            )
            .await?
            {
                if row.get_str("owner_kind").ok() == Some("organization") {
                    if let Ok(id) = row.get_str("_id") {
                        self.organization_sandboxes.insert(id.into());
                    }
                }
                if row.get_str("owner_kind").ok() == Some("organization")
                    || row
                        .get_str("owner_subject")
                        .is_ok_and(|owner| owner != self.subject)
                {
                    if let Ok(id) = row.get_str("_id") {
                        self.protect(Set::Sandboxes, id);
                    }
                }
            }
            // Classify projected identities first. Foreign organization payloads
            // must never consume the deleting person's live snapshot budget.
            let current = rows(db, "sandboxes", doc! {"_id":{"$in":&ids}}, &self.subject).await?;
            for row in &current {
                if row
                    .get_str("owner")
                    .is_ok_and(|owner| owner != self.subject)
                {
                    if let Ok(id) = row.get_str("_id") {
                        self.protect(Set::Sandboxes, id);
                    }
                }
            }
            let personal: Vec<_> = current
                .into_iter()
                .filter_map(|row| {
                    row.get_str("_id")
                        .ok()
                        .filter(|id| !self.protected.contains(*id))
                        .map(str::to_owned)
                })
                .collect();
            self.sandboxes = live_snapshots(db, &personal, &self.subject).await?;
        } else {
            self.sandboxes.clear();
        }
        self.exclude_protected();
        let launches: Vec<_> = self
            .ids(Set::Launches)
            .into_iter()
            .map(layer2_core::LaunchId)
            .collect();
        let subject = layer2_core::SubjectId::new(self.subject.clone());
        self.execution_digests =
            layer2_core::resource_access::execution::subject_execution_digests(&subject, &launches)
                .map_err(|_| Error::InvalidRecord("resource execution identity digest"))?
                .into_iter()
                .collect();
        self.shares.clear();
        let personal = self.ids(Set::Sandboxes);
        if !personal.is_empty() {
            let filter = doc! {"sandbox_id":{"$in":personal}};
            for row in rows(db, "resource_shares", filter, &self.subject).await? {
                if let Ok(id) = row.get_str("_id") {
                    self.shares.insert(id.to_owned());
                }
            }
        }
        self.bounds()
    }
}

#[cfg(test)]
pub(super) async fn load(db: &Database, target: &Target<'_>) -> Result<Context, Error> {
    load_cohort(db, std::slice::from_ref(target))
        .await?
        .pop()
        .ok_or(Error::InvalidRecord("erasure target"))
}

#[cfg(test)]
pub(super) async fn load_cohort(
    db: &Database,
    targets: &[Target<'_>],
) -> Result<Vec<Context>, Error> {
    let receipt = super::optional::inspect(db, Some(false), Some(false)).await?;
    load_cohort_with_receipt(db, targets, &receipt).await
}

pub(super) async fn load_cohort_with_receipt(
    db: &Database,
    targets: &[Target<'_>],
    receipt: &super::optional::Receipt,
) -> Result<Vec<Context>, Error> {
    super::indexes::ensure_with_receipt(db, receipt).await?;
    let mut contexts = Vec::new();
    for target in targets {
        let mut context = Context {
            subject: target.subject.as_str().into(),
            ..Default::default()
        };
        inventory::merge(db, &mut context, target.inventory).await?;
        let live = rows(
            db,
            "sandboxes",
            doc! {"owner":&context.subject},
            &context.subject,
        )
        .await?;
        for row in &live {
            context.add(
                Set::Sandboxes,
                row.get_str("_id")
                    .map_err(|_| Error::InvalidRecord("sandboxes").attribute(&context.subject))?,
            )?;
        }
        context.bounds()?;
        contexts.push(context);
    }
    if contexts.is_empty() {
        return Ok(contexts);
    }
    history::discover(db, &mut contexts, !receipt.usage_projections_absent).await?;
    for context in &mut contexts {
        context
            .refresh(db)
            .await
            .map_err(|error| error.attribute(&context.subject))?;
        descendants(db, &mut context.sets, &context.subject).await?;
        context.bounds()?;
        // Current ownership also fences inventoried/derived children when
        // their retained parent has already disappeared.
        context
            .refresh(db)
            .await
            .map_err(|error| error.attribute(&context.subject))?;
    }
    Ok(contexts)
}

async fn descendants(
    db: &Database,
    sets: &mut BTreeMap<Set, BTreeSet<String>>,
    subject: &str,
) -> Result<(), Error> {
    for (name, set, field, parent) in [
        ("workspaces", Set::Workspaces, "sandbox_id", Set::Sandboxes),
        ("worktrees", Set::Worktrees, "sandbox_id", Set::Sandboxes),
        ("agents", Set::Agents, "sandbox_id", Set::Sandboxes),
        ("worktrees", Set::Worktrees, "workspace_id", Set::Workspaces),
        ("agents", Set::Agents, "workspace_id", Set::Workspaces),
        ("agents", Set::Agents, "worktree_id", Set::Worktrees),
    ] {
        let parents: Vec<_> = sets.get(&parent).into_iter().flatten().cloned().collect();
        if parents.is_empty() {
            continue;
        }
        for row in rows(db, name, doc! {field:{"$in":parents}}, subject).await? {
            if let Ok(id) = row.get_str("_id") {
                sets.entry(set).or_default().insert(id.to_owned());
            }
        }
        let ids = sets.values().flatten();
        if ids.clone().count() > MAX_IDS || ids.map(String::len).sum::<usize>() > MAX_ID_BYTES {
            return Err(Error::capacity(subject, "personal resource IDs", MAX_IDS));
        }
    }
    Ok(())
}

pub(super) fn history_indexes() -> Vec<(&'static str, &'static str)> {
    history::indexes()
}

async fn rows(
    db: &Database,
    name: &str,
    filter: Document,
    subject: &str,
) -> Result<Vec<Document>, Error> {
    let collection = crate::store::durability::majority_collection::<Document>(db, name);
    // One streaming cursor follows the selector index. There is no repeated
    // _id sort or hint that could scan other tenants on every refresh.
    let query = collection.find(filter).batch_size(PAGE as u32)
        .projection(doc! {"_id":1,"sandbox_id":1,"workspace_id":1,"worktree_id":1,
            "owner":1,"owner_kind":1,"owner_subject":1,"audit_owner_subject":1,"record.owner":1,"record.sandbox_id":1,
            "binding_id":1,"record.binding_id":1,"record.agent_id":1,"record.workspace_id":1,"record.worktree_id":1});
    let mut cursor = query.await?;
    let mut result = Vec::new();
    let mut bytes = 0_usize;
    while let Some(row) = cursor.try_next().await? {
        bytes += mongodb::bson::serialize_to_vec(&row)
            .map_err(|_| Error::InvalidRecord("ownership snapshot"))?
            .len();
        if result.len() >= MAX_IDS || bytes > MAX_SNAPSHOT_BYTES {
            return Err(Error::capacity(
                subject,
                "ownership snapshot bytes",
                MAX_SNAPSHOT_BYTES,
            ));
        }
        result.push(row);
    }
    Ok(result)
}

async fn live_snapshots(
    db: &Database,
    ids: &[String],
    subject: &str,
) -> Result<Vec<Document>, Error> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let mut cursor = crate::store::durability::majority_collection::<Document>(db, "sandboxes")
        .find(doc! {"_id":{"$in":ids}})
        .batch_size(PAGE as u32)
        .await?;
    let mut rows = vec![];
    let mut bytes = 0_usize;
    while let Some(row) = cursor.try_next().await? {
        // Malformed query-field ownership must reach a permanent attributed
        // refusal before descendants can be erased. A valid foreign owner is
        // still outside this person's live snapshot budget.
        let owner = row
            .get_str("owner")
            .map_err(|_| Error::InvalidRecord("sandboxes").attribute(subject))?;
        if owner != subject {
            continue;
        }
        bytes = bytes.saturating_add(
            mongodb::bson::serialize_to_vec(&row)
                .map_err(|_| Error::InvalidRecord("sandboxes").attribute(subject))?
                .len(),
        );
        if bytes > MAX_LIVE_SNAPSHOT_BYTES {
            return Err(Error::capacity(
                subject,
                "live Sandbox snapshot bytes",
                MAX_LIVE_SNAPSHOT_BYTES,
            ));
        }
        rows.push(row);
    }
    Ok(rows)
}

pub(super) fn value<'a>(row: &'a Document, path: &str) -> Option<&'a Bson> {
    let mut parts = path.split('.');
    let mut value = row.get(parts.next()?)?;
    for part in parts {
        value = value.as_document()?.get(part)?;
    }
    Some(value)
}
