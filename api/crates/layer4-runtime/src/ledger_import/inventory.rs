//! Bounded structural inventory only. Semantic import validation is separate.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::io::{Directory, Identity};
use super::{changed, invalid, Budget, Limits};
use crate::config::RuntimeConfig;
use crate::failure::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Kind {
    Authority,
    Journal,
    Workspace,
    Installation,
    Ownership,
}

impl Kind {
    fn maximum(self) -> u64 {
        match self {
            Self::Authority => crate::mutation_authority::MAX_AUTHORITY_BYTES,
            Self::Journal => crate::mutation_journal::MAX_JOURNAL_BYTES,
            Self::Workspace => crate::workspace_journal::MAX_JOURNAL_BYTES,
            Self::Installation => 2 * 1024 * 1024,
            Self::Ownership => 4096,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Entry {
    pub(super) directory: usize,
    pub(super) name: String,
    pub(super) identity: Identity,
    pub(super) sha256: String,
    pub(super) temporary: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Root {
    pub(super) path: std::path::PathBuf,
    pub(super) identity: Option<Identity>,
    pub(super) kind: Kind,
    pub(super) scope: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Inventory {
    #[serde(deserialize_with = "bounded_metadata")]
    pub(super) roots: Vec<Root>,
    #[serde(deserialize_with = "bounded_metadata")]
    pub(super) entries: Vec<Entry>,
    pub(super) digest: String,
    examined: usize,
    bytes: u64,
    #[serde(skip)]
    pub(super) archive: Option<(std::path::PathBuf, Identity)>,
}

fn bounded_metadata<'de, D, T>(deserializer: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Rows<T>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for Rows<T> {
        type Value = Vec<T>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("bounded ledger inventory")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut rows: A,
        ) -> std::result::Result<Vec<T>, A::Error> {
            let mut result = Vec::new();
            while let Some(row) = rows.next_element()? {
                // Every captured entry/root already costs at least 512 bytes
                // in the planner's metadata budget. Bound allocations on input
                // too, before a forged manifest can grow a history-sized vector.
                if result.len() >= Limits::default().metadata_bytes / 512 {
                    return Err(serde::de::Error::custom("ledger metadata budget"));
                }
                result.push(row);
            }
            Ok(result)
        }
    }
    deserializer.deserialize_seq(Rows(std::marker::PhantomData))
}

impl Inventory {
    /// Physical entries that the V1 serving readers count. Historical process
    /// scopes remain in the complete inventory but do not spend today's bound.
    pub(super) fn capacity_summary(&self, generation: u64) -> serde_json::Value {
        let mut journal = [0_u64; 2];
        let mut authority = [0_u64; 2];
        let mut historical = [0_u64; 2];
        for entry in &self.entries {
            let root = &self.roots[entry.directory];
            let current = root.scope == 0 || root.scope == generation;
            let temporary = usize::from(entry.temporary);
            match (root.kind, current) {
                (Kind::Journal, true) => journal[temporary] += 1,
                (Kind::Authority, true) => authority[temporary] += 1,
                (Kind::Journal, false) => historical[0] += 1,
                (Kind::Authority, false) => historical[1] += 1,
                _ => {}
            }
        }
        serde_json::json!({
            "journal_canonical_files": journal[0],
            "journal_temporary_files": journal[1],
            "journal_scan_entries": journal.iter().sum::<u64>(),
            "journal_scan_limit": crate::mutation_journal::MAX_JOURNAL_ENTRIES,
            "authority_canonical_files": authority[0],
            "authority_temporary_files": authority[1],
            "authority_scan_entries": authority[0],
            "authority_scan_limit": crate::mutation_authority::MAX_AUTHORITY_RECORDS,
            "historical_journal_files": historical[0],
            "historical_authority_files": historical[1],
            "admission_headroom": "unproved",
            "retirement_eligibility": "requires_authoritative_state",
        })
    }

    pub(super) fn read(&self, index: usize, budget: &mut Budget) -> Result<Vec<u8>> {
        let entry = self.entries.get(index).ok_or_else(invalid)?;
        let root = self.roots.get(entry.directory).ok_or_else(invalid)?;
        if let Some((path, expected)) = &self.archive {
            let directory = Directory::open(path)?;
            if directory.identity()? != *expected {
                return Err(changed());
            }
            let (identity, bytes) =
                directory.read(&format!("{index:08}.record"), root.kind.maximum(), budget)?;
            if !identity.sealed_file()
                || identity.bytes != entry.identity.bytes
                || crate::command::hex(&Sha256::digest(&bytes)) != entry.sha256
            {
                return Err(changed());
            }
            return Ok(bytes);
        }
        let directory = Directory::open(&root.path)?;
        if Some(directory.identity()?) != root.identity {
            return Err(changed());
        }
        let (identity, bytes) = directory.read(&entry.name, root.kind.maximum(), budget)?;
        if identity != entry.identity
            || crate::command::hex(&Sha256::digest(&bytes)) != entry.sha256
        {
            return Err(changed());
        }
        Ok(bytes)
    }

    pub(super) fn summary(&self) -> serde_json::Value {
        serde_json::json!({
            "format": "runtime-ledger-inventory-v1", "admission": "held",
            "writer_exclusion": "unproved", "semantic_validation": "pending",
            "directories": self.roots.len(), "examined_entries": self.examined,
            "files": self.entries.len(), "input_bytes": self.bytes,
            "temporary_files": self.entries.iter().filter(|entry| entry.temporary).count(),
            "inventory_sha256": self.digest,
        })
    }
}

pub(super) fn scan(config: &RuntimeConfig, limits: Limits) -> Result<Inventory> {
    scan_with_budget(config, &mut Budget::new(limits))
}

pub(super) fn scan_with_budget(config: &RuntimeConfig, budget: &mut Budget) -> Result<Inventory> {
    budget.check()?;
    let initial_entries = budget.entries;
    let initial_bytes = budget.bytes;
    if config.home_identity.is_none() || config.generation == 0 {
        return Err(invalid());
    }
    let home = Directory::open(&config.home)?.identity()?;
    let state = Directory::open(&config.state_root)?;
    let transfer = Directory::open(&config.checkpoint_root)?;
    let state_before = state.identity()?;
    let transfer_before = transfer.identity()?;
    let mut result = Inventory {
        roots: Vec::new(),
        entries: Vec::new(),
        digest: String::new(),
        examined: 0,
        bytes: 0,
        archive: None,
    };
    // Count every state-root entry, including unrelated helper directories;
    // never scan home files, native conversation data or credential directories.
    let names = state.names(budget)?;
    let mut scopes = vec![(0, config.state_root.clone())];
    for name in &names {
        if let Some(number) = name.strip_prefix("process-generation-") {
            let generation = number.parse::<u64>().map_err(|_| invalid())?;
            if generation == 0 || generation > config.generation || generation.to_string() != number
            {
                return Err(invalid());
            }
            budget.path(config.state_root.as_os_str().len() + name.len() + 1)?;
            scopes.push((generation, config.state_root.join(name)));
        }
        if name.starts_with("mutation-ledger-v2") || name.starts_with(".mutation-ledger-v2") {
            return Err(invalid());
        }
    }
    scopes.sort();
    for (scope, path) in scopes {
        let directory = Directory::open(&path)?;
        if scope != 0 {
            let children = directory.names(budget)?;
            if children
                .iter()
                .any(|name| name != "mutation-authority" && name != "runtime-mutations")
            {
                return Err(invalid());
            }
        }
        for (name, kind) in [
            ("mutation-authority", Kind::Authority),
            ("runtime-mutations", Kind::Journal),
        ] {
            result.collect(&directory, path.join(name), name, kind, scope, budget)?;
        }
    }
    for (name, kind) in [
        ("workspace-operations", Kind::Workspace),
        ("home-binding", Kind::Ownership),
    ] {
        result.collect(&state, config.state_root.join(name), name, kind, 0, budget)?;
    }
    result.collect(
        &transfer,
        config.checkpoint_root.join("runtime-state"),
        "runtime-state",
        Kind::Installation,
        0,
        budget,
    )?;
    // Retain the current removal writer and its unpublished replacements as
    // evidence. Neither their existence nor a released flock proves quiescence.
    let root = result.roots.len();
    budget.path(config.state_root.as_os_str().len())?;
    result.roots.push(Root {
        path: config.state_root.clone(),
        identity: Some(state_before.clone()),
        kind: Kind::Ownership,
        scope: 0,
    });
    for name in &names {
        if name == "workspace-removal-writer-v1.json"
            || name == "workspace-removal-writer-v1.lock"
            || temporary(name, ".workspace-removal-writer-", 128)
        {
            result.file(
                &state,
                root,
                name,
                Kind::Ownership,
                name.ends_with(".tmp"),
                budget,
            )?;
        }
        // The separate repair protocol has additional authority not represented
        // by the V1 mutation store. Refuse to omit it from a purported inventory.
        if name == "workspace-prepare-recovery" || name == "workspace-removal-current-v1" {
            return Err(invalid());
        }
    }
    if state.identity()? != state_before
        || transfer.identity()? != transfer_before
        || Directory::open(&config.state_root)?.identity()? != state_before
        || Directory::open(&config.checkpoint_root)?.identity()? != transfer_before
        || Directory::open(&config.home)?.identity()? != home
    {
        return Err(changed());
    }
    let mut digest = Sha256::new();
    digest.update(b"chrono.runtime.ledger.inventory.v1\0");
    // Stream serialization into the digest, avoiding a second history-sized buffer.
    struct HashWriter<'a>(&'a mut Sha256);
    impl std::io::Write for HashWriter<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(
        HashWriter(&mut digest),
        &(
            &config.sandbox_id,
            &config.home_identity,
            config.generation,
            &config.home,
            home,
            state_before,
            transfer_before,
            &result.roots,
            &result.entries,
        ),
    )
    .map_err(|_| invalid())?;
    result.digest = crate::command::hex(&digest.finalize());
    budget.check()?;
    result.examined = budget.entries - initial_entries;
    result.bytes = budget.bytes - initial_bytes;
    Ok(result)
}

impl Inventory {
    fn collect(
        &mut self,
        parent: &Directory,
        path: std::path::PathBuf,
        name: &str,
        kind: Kind,
        scope: u64,
        budget: &mut Budget,
    ) -> Result<()> {
        budget.path(path.as_os_str().len())?;
        let directory = parent.child(name)?;
        let identity = directory.as_ref().map(Directory::identity).transpose()?;
        let index = self.roots.len();
        self.roots.push(Root {
            path,
            identity: identity.clone(),
            kind,
            scope,
        });
        if let Some(directory) = directory {
            for name in directory.names(budget)? {
                let temporary = classify(&name, kind)?;
                self.file(&directory, index, &name, kind, temporary, budget)?;
            }
            if Some(directory.identity()?) != identity {
                return Err(changed());
            }
        }
        Ok(())
    }

    fn file(
        &mut self,
        directory: &Directory,
        index: usize,
        name: &str,
        kind: Kind,
        temporary: bool,
        budget: &mut Budget,
    ) -> Result<()> {
        let (identity, bytes) = directory.read(name, kind.maximum(), budget)?;
        self.entries.push(Entry {
            directory: index,
            name: name.into(),
            identity,
            sha256: crate::command::hex(&Sha256::digest(&bytes)),
            temporary,
        });
        Ok(())
    }
}

pub(crate) fn classify(name: &str, kind: Kind) -> Result<bool> {
    let canonical = name.strip_suffix(".json").is_some_and(|stem| {
        stem.len() == 64
            && stem
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    });
    let (valid, prefix, bound) = match kind {
        Kind::Authority => (canonical, ".authority-", 128),
        Kind::Journal => (canonical, ".mutation-journal-", 240),
        Kind::Workspace => (canonical, ".workspace-journal-", 240),
        Kind::Installation => (name == "install-v2.json", ".install-", 128),
        Kind::Ownership => (name == "binding.json", ".home-", 42),
    };
    if valid {
        Ok(false)
    } else if temporary(name, prefix, bound) {
        Ok(true)
    } else {
        Err(invalid())
    }
}

fn temporary(name: &str, prefix: &str, bound: usize) -> bool {
    name.starts_with(prefix)
        && name.ends_with(".tmp")
        && name.len() <= bound
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
}
