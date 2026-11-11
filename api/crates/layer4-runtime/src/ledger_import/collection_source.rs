//! Exact bounded V1 journal/authority census. No recovery or effects occur here.
//! Physical exclusion remains an independently qualified caller prerequisite.
use super::{
    changed, invalid,
    io::{Directory, Identity},
    Budget, Limits,
};
use crate::mutation_authority::{Disposition, Record as Authority, ResourceKey};
use crate::mutation_journal::{Record as Journal, Status};
use crate::retirement_archive::{group_key, GroupProof, SourceRecord};
use crate::{config::RuntimeConfig, failure::Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Root {
    path: PathBuf,
    device: u64,
    inode: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    directory: usize,
    scope: u64,
    kind: u8,
    name: String,
    identity: Identity,
    sha256: String,
    group: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Snapshot {
    roots: Vec<Root>,
    entries: Vec<Entry>,
}

pub(crate) fn scan(config: &RuntimeConfig) -> Result<Snapshot> {
    if config.generation == 0 || config.home_identity.is_none() {
        return Err(invalid());
    }
    let mut budget = Budget::new(Limits::default());
    let state = Directory::open(&config.state_root)?;
    let before = state.identity()?;
    let mut scopes = vec![(0, config.state_root.clone())];
    for name in state.names(&mut budget)? {
        if name.starts_with("mutation-ledger-v2") || name.starts_with(".mutation-ledger-v2") {
            return Err(invalid());
        }
        if let Some(value) = name.strip_prefix("process-generation-") {
            let generation: u64 = value.parse().map_err(|_| invalid())?;
            if generation == 0 || generation > config.generation || generation.to_string() != value
            {
                return Err(invalid());
            }
            scopes.push((generation, config.state_root.join(name)));
        }
    }
    scopes.sort();
    let mut snapshot = Snapshot {
        roots: vec![Root {
            path: config.state_root.clone(),
            device: before.device,
            inode: before.inode,
        }],
        entries: Vec::new(),
    };
    for (scope, path) in scopes {
        let parent = Directory::open(&path)?;
        let parent_before = parent.identity()?;
        if scope != 0
            && parent
                .names(&mut budget)?
                .iter()
                .any(|name| name != "runtime-mutations" && name != "mutation-authority")
        {
            return Err(invalid());
        }
        for (kind, name) in [(1, "runtime-mutations"), (2, "mutation-authority")] {
            let Some(directory) = parent.child(name)? else {
                continue;
            };
            let identity = directory.identity()?;
            budget.path(path.as_os_str().len() + name.len())?;
            let directory_index = snapshot.roots.len();
            snapshot.roots.push(Root {
                path: path.join(name),
                device: identity.device,
                inode: identity.inode,
            });
            for name in directory.names(&mut budget)? {
                let (identity, bytes) = directory.read(&name, maximum(kind), &mut budget)?;
                let record = SourceRecord {
                    scope,
                    kind,
                    name: name.clone(),
                    bytes,
                };
                let operation = record.operation(config)?;
                snapshot.entries.push(Entry {
                    directory: directory_index,
                    scope,
                    kind,
                    name,
                    identity,
                    sha256: hash(&record.bytes),
                    group: group_key(&operation),
                });
            }
            if directory.identity()? != identity {
                return Err(changed());
            }
        }
        if parent.identity()? != parent_before {
            return Err(changed());
        }
    }
    if state.identity()? != before {
        return Err(changed());
    }
    snapshot
        .entries
        .sort_by(|a, b| (a.scope, a.kind, &a.name).cmp(&(b.scope, b.kind, &b.name)));
    // Encoding has its own cap, independent of source file byte limits.
    snapshot.encode()?;
    Ok(snapshot)
}

impl Snapshot {
    pub(crate) fn encode(&self) -> Result<Vec<u8>> {
        let bytes = serde_json::to_vec(self).map_err(|_| invalid())?;
        if bytes.len() > 24 * 1024 * 1024 {
            return Err(invalid());
        }
        Ok(bytes)
    }
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > 24 * 1024 * 1024 {
            return Err(invalid());
        }
        let value: Self = serde_json::from_slice(bytes).map_err(|_| invalid())?;
        if value.roots.is_empty()
            || value.roots.len() > 65_536
            || value.entries.len() > 65_536
            || value.entries.windows(2).any(|pair| {
                (pair[0].scope, pair[0].kind, &pair[0].name)
                    >= (pair[1].scope, pair[1].kind, &pair[1].name)
            })
            || value
                .entries
                .iter()
                .any(|e| e.directory >= value.roots.len() || !matches!(e.kind, 1 | 2))
        {
            return Err(invalid());
        }
        Ok(value)
    }
    pub(crate) fn hash(&self) -> Result<String> {
        Ok(hash(&self.encode()?))
    }
    pub(crate) fn without(&self, group: &str) -> Self {
        Self {
            roots: self.roots.clone(),
            entries: self
                .entries
                .iter()
                .filter(|e| e.group != group)
                .cloned()
                .collect(),
        }
    }
    pub(crate) fn counts(&self, group: &str) -> [u64; 2] {
        let mut counts = [0; 2];
        for e in self.entries.iter().filter(|e| e.group == group) {
            counts[e.kind as usize - 1] += 1;
        }
        counts
    }
    pub(crate) fn require_partial(&self, current: &Self, group: &str) -> Result<()> {
        if self.without(group) != current.without(group)
            || current
                .entries
                .iter()
                .filter(|e| e.group == group)
                .any(|e| {
                    self.entries
                        .binary_search_by(|old| {
                            (old.scope, old.kind, &old.name).cmp(&(e.scope, e.kind, &e.name))
                        })
                        .ok()
                        .is_none_or(|index| self.entries[index] != *e)
                })
        {
            return Err(changed());
        }
        Ok(())
    }
    pub(crate) fn group_indexes(&self, group: &str) -> Vec<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| (e.group == group).then_some(i))
            .collect()
    }
    pub(crate) fn read_record(&self, config: &RuntimeConfig, index: usize) -> Result<SourceRecord> {
        let entry = self.entries.get(index).ok_or_else(invalid)?;
        let root = &self.roots[entry.directory];
        let directory = Directory::open(&root.path)?;
        let actual = directory.identity()?;
        if (actual.device, actual.inode) != (root.device, root.inode) {
            return Err(changed());
        }
        let (identity, bytes) = directory.read(
            &entry.name,
            maximum(entry.kind),
            &mut Budget::new(Limits::default()),
        )?;
        if identity != entry.identity || hash(&bytes) != entry.sha256 {
            return Err(changed());
        }
        let record = SourceRecord {
            scope: entry.scope,
            kind: entry.kind,
            name: entry.name.clone(),
            bytes,
        };
        if group_key(&record.operation(config)?) != entry.group {
            return Err(changed());
        }
        Ok(record)
    }
    pub(crate) fn active_path(
        &self,
        config: &RuntimeConfig,
        record: &SourceRecord,
    ) -> Result<PathBuf> {
        let index = self
            .entries
            .binary_search_by(|e| {
                (e.scope, e.kind, &e.name).cmp(&(record.scope, record.kind, &record.name))
            })
            .map_err(|_| invalid())?;
        let e = &self.entries[index];
        if e.sha256 != hash(&record.bytes) || group_key(&record.operation(config)?) != e.group {
            return Err(changed());
        }
        let mut scoped = config.clone();
        if record.scope != 0 {
            scoped.generation = record.scope;
        }
        let expected = match record.kind {
            1 => Journal::decode(&record.bytes, &scoped)?.inventory_name(&scoped),
            2 => serde_json::from_slice::<Authority>(&record.bytes)
                .map_err(|_| invalid())?
                .inventory_name(&scoped),
            _ => return Err(invalid()),
        };
        let root = &self.roots[e.directory];
        if expected.parent() != Some(root.path.as_path()) {
            return Err(changed());
        }
        let actual = Directory::open(&root.path)?.identity()?;
        if (actual.device, actual.inode) != (root.device, root.inode) {
            return Err(changed());
        }
        Ok(root.path.clone())
    }
    pub(crate) fn transfer(
        &self,
        config: &RuntimeConfig,
        archive: &str,
        record: &SourceRecord,
    ) -> Result<()> {
        use std::{
            ffi::CString,
            os::{fd::AsRawFd, unix::fs::MetadataExt},
        };
        let id = uuid::Uuid::parse_str(archive).map_err(|_| invalid())?;
        if id.is_nil() || id.to_string() != archive {
            return Err(invalid());
        }
        let active = self.active_path(config, record)?;
        let mut path = config.checkpoint_root.clone();
        let mut parent = Directory::open(&path)?;
        for name in [
            "runtime-ledger-retired-v4".to_string(),
            archive.to_string(),
            format!("{}-{}", record.scope, record.kind),
        ] {
            let component = CString::new(name.as_str()).map_err(|_| invalid())?;
            let result = unsafe { libc::mkdirat(parent.0.as_raw_fd(), component.as_ptr(), 0o700) };
            if result != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST) {
                return Err(invalid());
            }
            let child = parent.child(&name)?.ok_or_else(invalid)?;
            let metadata = child.0.metadata().map_err(|_| invalid())?;
            if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
                return Err(invalid());
            }
            parent.0.sync_all().map_err(|_| invalid())?;
            path.push(name);
            if Directory::open(&path)?.identity()? != child.identity()? {
                return Err(changed());
            }
            parent = child;
        }
        super::retirement_move::Pair::open(&active, &path)?.transfer(
            &record.name,
            &record.bytes,
            false,
            |_| Ok(()),
        )
    }
    pub(crate) fn proof(
        &self,
        config: &RuntimeConfig,
        group: &str,
        observation: &str,
    ) -> Result<GroupProof> {
        let mut resources = BTreeSet::new();
        let mut deleted = BTreeSet::new();
        let mut authority = BTreeMap::new();
        let mut digest = Sha256::new();
        let counts = self.counts(group);
        if counts.contains(&0) {
            return Err(invalid());
        }
        let records = u32::try_from(counts.iter().sum::<u64>()).map_err(|_| invalid())?;
        for index in self.group_indexes(group) {
            let record = self.read_record(config, index)?;
            let operation = record.operation(config)?;
            let (resource, disposition) = crate::mutation_authority::resource_operation(&operation);
            let key = (record.scope, resource);
            resources.insert(key.clone());
            let mut scoped = config.clone();
            if record.scope != 0 {
                scoped.generation = record.scope;
            }
            if record.kind == 1 {
                if Journal::decode(&record.bytes, &scoped)?.status() != Status::Completed {
                    return Err(invalid());
                }
                if disposition == Disposition::Absent {
                    deleted.insert((
                        key,
                        operation.operation_id.clone(),
                        operation.request_sha256.clone(),
                    ));
                }
            } else {
                let value: Authority =
                    serde_json::from_slice(&record.bytes).map_err(|_| invalid())?;
                match authority.get(&key) {
                    Some((fence, old, old_operation, old_hash))
                        if *fence == value.authority_fence
                            && (*old != value.disposition
                                || old_operation != &operation.operation_id
                                || old_hash != &operation.request_sha256) =>
                    {
                        return Err(invalid())
                    }
                    Some((fence, _, _, _)) if *fence >= value.authority_fence => {}
                    _ => {
                        authority.insert(
                            key,
                            (
                                value.authority_fence,
                                value.disposition,
                                operation.operation_id,
                                operation.request_sha256,
                            ),
                        );
                    }
                }
            }
            record.hash_into(&mut digest);
        }
        for resource in resources {
            let key = if let ResourceKey::AgentSkill { agent_id, .. } = resource.1 {
                (0, ResourceKey::AgentHome { agent_id })
            } else {
                resource
            };
            let (_, disposition, operation, request) = authority.get(&key).ok_or_else(invalid)?;
            if *disposition != Disposition::Absent
                || !deleted.contains(&(key, operation.clone(), request.clone()))
            {
                return Err(invalid());
            }
        }
        Ok(GroupProof {
            key: group.into(),
            source_sha256: format!("{:x}", digest.finalize()),
            observation_sha256: observation.into(),
            records,
        })
    }
}
fn maximum(kind: u8) -> u64 {
    if kind == 1 {
        crate::mutation_journal::MAX_JOURNAL_BYTES
    } else {
        crate::mutation_authority::MAX_AUTHORITY_BYTES
    }
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
