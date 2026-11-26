//! Validate every source record before folding, retaining only source references.
//! The import artifact remains held even if all mutation histories are valid.
use std::collections::BTreeMap;

use super::inventory::{Inventory, Kind};
use super::{capacity, capacity_at_least, invalid, Budget, Limits};
use crate::config::RuntimeConfig;
use crate::failure::Result;
use crate::mutation_authority::{self, Disposition, Record as Authority};
use crate::mutation_journal::{Record as Journal, Status};

type Key = (u64, String);
type Survivors = [Option<usize>; 3];

#[derive(Default)]
pub(super) struct JournalGroup {
    pub(super) canonical: Option<usize>,
    pub(super) replacement: Option<usize>,
    pub(super) selected: Option<usize>,
}

#[derive(Default)]
pub(super) struct Analysis {
    pub(super) authority: BTreeMap<Key, Survivors>,
    pub(super) journals: BTreeMap<Key, JournalGroup>,
    statuses: [u64; 4],
    auxiliary_files: u64,
    metadata: usize,
}

impl Analysis {
    pub(super) fn summary(&self, inventory: &Inventory) -> serde_json::Value {
        let mut summary = inventory.summary();
        summary["semantic_validation"] = "mutation_histories_valid".into();
        summary["authority_resources"] = self.authority.len().into();
        summary["journal_operations"] = self.journals.len().into();
        summary["started"] = self.statuses[0].into();
        summary["completed"] = self.statuses[1].into();
        summary["failed"] = self.statuses[2].into();
        summary["indeterminate"] = self.statuses[3].into();
        summary["auxiliary_files"] = self.auxiliary_files.into();
        summary["auxiliary_recovery"] = "unqualified".into();
        summary
    }
    fn reserve(&mut self, key: &str) -> Result<()> {
        self.metadata = self
            .metadata
            .checked_add(512 + key.len() * 2)
            .ok_or_else(capacity)?;
        if self.metadata > 8 * 1024 * 1024 {
            return Err(capacity_at_least(
                "index_metadata_bytes",
                self.metadata as u128,
            ));
        }
        Ok(())
    }
}

pub(super) fn analyze(config: &RuntimeConfig, inventory: &Inventory) -> Result<Analysis> {
    analyze_budget(config, inventory, Budget::new(Limits::default()))
}

pub(super) fn analyze_budget(
    config: &RuntimeConfig,
    inventory: &Inventory,
    mut budget: Budget,
) -> Result<Analysis> {
    analyze_with_budget(config, inventory, &mut budget)
}

pub(super) fn analyze_with_budget(
    config: &RuntimeConfig,
    inventory: &Inventory,
    budget: &mut Budget,
) -> Result<Analysis> {
    budget.check()?;
    let mut result = Analysis::default();
    let mut binding = false;
    for (index, entry) in inventory.entries.iter().enumerate() {
        budget.check()?;
        let root = inventory.roots.get(entry.directory).ok_or_else(invalid)?;
        let bytes = inventory.read(index, budget)?;
        let path = root.path.join(&entry.name);
        let mut scoped = config.clone();
        if root.scope != 0 {
            scoped.generation = root.scope;
        }
        match root.kind {
            Kind::Authority if entry.temporary => {} // V1 ignores unpublished authority.
            Kind::Authority => {
                super::json::check(&bytes)?;
                let record =
                    Authority::decode_inventory(&bytes, &scoped, &path).map_err(|_| invalid())?;
                result.authority(index, record, &scoped, inventory, budget)?;
            }
            Kind::Journal => {
                super::json::check(&bytes)?;
                let record = Journal::decode(&bytes, &scoped).map_err(|_| invalid())?;
                if record.inventory_name(&scoped).parent() != path.parent()
                    || (!entry.temporary && record.inventory_name(&scoped) != path)
                {
                    return Err(invalid());
                }
                result.journal(index, record, &scoped, inventory, budget)?;
            }
            Kind::Workspace => {
                super::json::check(&bytes)?;
                crate::workspace_journal::validate_inventory(
                    &bytes,
                    config,
                    &path,
                    entry.temporary,
                )
                .map_err(|_| invalid())?;
                result.auxiliary_files += 1;
            }
            Kind::Installation => {
                super::json::check(&bytes)?;
                let record = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
                crate::install_state_validation::validate_record(config, &record)
                    .map_err(|_| invalid())?;
                result.auxiliary_files += 1;
            }
            Kind::Ownership => {
                if entry.name == "binding.json" {
                    super::json::check(&bytes)?;
                    crate::persistent_home::Binding::validate_inventory(&bytes, config)
                        .map_err(|_| invalid())?;
                    if binding {
                        return Err(invalid());
                    }
                    binding = true;
                }
                result.auxiliary_files += 1;
            }
        }
    }
    if !binding {
        return Err(invalid());
    }
    for ((scope, _), group) in &mut result.journals {
        let mut scoped = config.clone();
        if *scope != 0 {
            scoped.generation = *scope;
        }
        let canonical = group.canonical.ok_or_else(invalid)?;
        let record = journal(inventory, canonical, &scoped, budget)?;
        let mut selected = canonical;
        let mut status = record.status();
        if let Some(index) = group.replacement {
            let candidate = journal(inventory, index, &scoped, budget)?;
            record
                .inventory_replacement(&candidate)
                .map_err(|_| invalid())?;
            if record.status() == Status::Started {
                selected = index;
                status = candidate.status();
            } else if record != candidate {
                return Err(invalid());
            }
        }
        group.selected = Some(selected);
        result.statuses[match status {
            Status::Started => 0,
            Status::Completed => 1,
            Status::Failed => 2,
            Status::Indeterminate => 3,
        }] += 1;
    }
    budget.check()?;
    Ok(result)
}

impl Analysis {
    fn authority(
        &mut self,
        index: usize,
        record: Authority,
        config: &RuntimeConfig,
        inventory: &Inventory,
        budget: &mut Budget,
    ) -> Result<()> {
        let scope = inventory.roots[inventory.entries[index].directory].scope;
        let resource = serde_json::to_string(&record.resource).map_err(|_| invalid())?;
        let key = (scope, resource);
        if !self.authority.contains_key(&key) {
            self.reserve(&key.1)?;
        }
        let survivors = self.authority.entry(key).or_insert([None; 3]);
        let mut records = Vec::with_capacity(4);
        for prior in survivors.iter().flatten() {
            if records.iter().any(|(id, _)| id == prior) {
                continue;
            }
            let entry = &inventory.entries[*prior];
            let root = &inventory.roots[entry.directory];
            let bytes = inventory.read(*prior, budget)?;
            records.push((
                *prior,
                Authority::decode_inventory(&bytes, config, &root.path.join(&entry.name))
                    .map_err(|_| invalid())?,
            ));
        }
        records.push((index, record));
        mutation_authority::validate_history(
            &records.iter().map(|(_, record)| record).collect::<Vec<_>>(),
        )
        .map_err(|_| invalid())?;
        survivors[0] = records
            .iter()
            .filter(|(_, record)| record.disposition == Disposition::Present)
            .max_by_key(|(_, record)| record.authority_fence)
            .map(|(id, _)| *id);
        survivors[1] = records
            .iter()
            .filter(|(_, record)| record.disposition == Disposition::Absent)
            .min_by_key(|(_, record)| record.authority_fence)
            .map(|(id, _)| *id);
        survivors[2] = records
            .iter()
            .filter(|(_, record)| record.disposition == Disposition::Absent)
            .max_by_key(|(_, record)| record.authority_fence)
            .map(|(id, _)| *id);
        Ok(())
    }

    fn journal(
        &mut self,
        index: usize,
        record: Journal,
        config: &RuntimeConfig,
        inventory: &Inventory,
        budget: &mut Budget,
    ) -> Result<()> {
        let entry = &inventory.entries[index];
        let scope = inventory.roots[entry.directory].scope;
        let key = (scope, record.operation().operation_id);
        if !self.journals.contains_key(&key) {
            self.reserve(&key.1)?;
        }
        let group = self.journals.entry(key).or_default();
        if entry.temporary {
            if record.status() == Status::Started {
                return Err(invalid());
            }
            if let Some(prior) = group.replacement {
                if journal(inventory, prior, config, budget)? != record {
                    return Err(invalid());
                }
            }
            // Sorted input chooses the same final replacement as V1 reconcile.
            group.replacement = Some(index);
        } else if group.canonical.replace(index).is_some() {
            return Err(invalid());
        }
        Ok(())
    }
}

fn journal(
    inventory: &Inventory,
    index: usize,
    config: &RuntimeConfig,
    budget: &mut Budget,
) -> Result<Journal> {
    Journal::decode(&inventory.read(index, budget)?, config).map_err(|_| invalid())
}
