//! A durable intent authorizes only completing its exact, unused fresh store.
//! No-intent initial staging recovery is separately bounded in aborted.
use std::path::{Path, PathBuf};

use super::{BARRIER, DATABASE, LOCK, MANIFEST, STAGE, TEMP, descriptor::Descriptor, hold_at, io};
use crate::config::RuntimeConfig;
use crate::failure::Result;
use crate::mutation_ledger::Ledger;

pub(super) const INTENT: &str = ".mutation-ledger-v2.fresh.json";

pub(super) fn inspect(config: &RuntimeConfig) -> Result<Descriptor> {
    let descriptor = Descriptor::decode(config, &io::read(&config.state_root.join(INTENT))?)?;
    let _proof = verify(config, &descriptor)?;
    Ok(descriptor)
}

fn verify(
    config: &RuntimeConfig,
    descriptor: &Descriptor,
) -> Result<(PathBuf, crate::ledger_storage::ClosedInspection)> {
    crate::retirement_format::require_absent(config)?;
    crate::persistent_home::verify(config)?;
    if descriptor.born != config.generation
        || io::read(&config.state_root.join(INTENT))? != descriptor.bytes()?
    {
        return Err(hold_at("publication.intent_identity"));
    }
    let generation = format!("process-generation-{}", descriptor.born);
    only_entries(
        &config.state_root,
        &[
            "home-binding",
            "workspace-removal-writer-v1.lock",
            "workspace-removal-writer-v1.json",
            LOCK,
            INTENT,
            STAGE,
            DATABASE,
            TEMP,
            MANIFEST,
            "mutation-authority",
            "runtime-mutations",
            &generation,
        ],
    )?;
    only_entries(&config.state_root.join("home-binding"), &["binding.json"])?;
    if io::exists(&config.state_root.join(&generation))? {
        only_entries(
            &config.state_root.join(&generation),
            &["mutation-authority", "runtime-mutations"],
        )?;
    }
    let staged = io::exists(&config.state_root.join(STAGE))?;
    let committed = io::exists(&config.state_root.join(MANIFEST))?;
    if staged == io::exists(&config.state_root.join(DATABASE))?
        || (staged && (committed || io::exists(&config.state_root.join(TEMP))?))
        || (committed && io::exists(&config.state_root.join(TEMP))?)
    {
        return Err(hold_at("publication.intent_layout"));
    }
    for name in [TEMP, MANIFEST] {
        let path = config.state_root.join(name);
        if io::exists(&path)? && io::read(&path)? != descriptor.bytes()? {
            return Err(hold_at("publication.intent_descriptor"));
        }
    }
    // Validate every existing barrier before creating any missing barrier.
    for path in descriptor.base_markers() {
        let parent = path
            .parent()
            .ok_or_else(|| hold_at("publication.intent_layout"))?;
        if io::exists(parent)? {
            only_entries(parent, &[BARRIER])?;
        }
        if io::exists(&path)? {
            descriptor.verify([path])?;
        } else if committed {
            return Err(hold_at("publication.intent_barrier"));
        }
    }
    let database = config
        .state_root
        .join(if staged { STAGE } else { DATABASE });
    let writer = crate::ledger_storage::inspect_closed(&database, |db| {
        Ledger::verify_fresh_publication(db, config, &descriptor.store)
    })
    .map_err(|_| hold_at("publication.intent_database"))?;
    Ok((database, writer))
}

fn only_entries(path: &Path, allowed: &[&str]) -> Result<()> {
    io::directories(path)?;
    for entry in std::fs::read_dir(path)
        .map_err(|error| io::io_error("publication.intent_inventory", "intent_inventory", &error))?
    {
        let entry = entry.map_err(|error| {
            io::io_error("publication.intent_inventory", "intent_inventory", &error)
        })?;
        if !entry
            .file_name()
            .to_str()
            .is_some_and(|name| allowed.contains(&name))
        {
            return Err(hold_at("publication.intent_inventory"));
        }
    }
    Ok(())
}

pub(super) fn resume(config: &RuntimeConfig, descriptor: &Descriptor) -> Result<()> {
    super::retained_transfer(config)?;
    let (database, database_proof) = verify(config, descriptor)?;
    let intent_identity = intent_identity(config)?;
    #[cfg(test)]
    super::tests::replace_publication_path(config, "verified");
    check_paths(
        config,
        descriptor,
        &database_proof,
        &database,
        intent_identity,
    )?;
    crate::fs_durability::sync_tree(&database)
        .map_err(|error| io::io_error("publication.stage_sync", "stage_sync", &error))?;
    for path in descriptor.base_markers() {
        if !io::exists(&path)? {
            check_paths(
                config,
                descriptor,
                &database_proof,
                &database,
                intent_identity,
            )?;
            descriptor.publish([path])?;
        }
    }
    descriptor.verify(descriptor.base_markers())?;
    let final_database = config.state_root.join(DATABASE);
    if database != final_database {
        check_paths(
            config,
            descriptor,
            &database_proof,
            &database,
            intent_identity,
        )?;
        crate::fs_durability::rename(&database, &final_database).map_err(|error| {
            io::io_error(
                "publication.database_rename",
                "database_rename_and_sync",
                &error,
            )
        })?;
    }
    #[cfg(test)]
    super::tests::checkpoint(&config.state_root, "recovery-renamed")?;
    #[cfg(test)]
    super::tests::replace_publication_path(config, "renamed");
    check_paths(
        config,
        descriptor,
        &database_proof,
        &final_database,
        intent_identity,
    )?;
    if !io::exists(&config.state_root.join(MANIFEST))? {
        let temporary = config.state_root.join(TEMP);
        if !io::exists(&temporary)? {
            io::create(&temporary, &descriptor.bytes()?)?;
        }
        check_paths(
            config,
            descriptor,
            &database_proof,
            &final_database,
            intent_identity,
        )?;
        crate::fs_durability::rename(&temporary, &config.state_root.join(MANIFEST)).map_err(
            |error| {
                io::io_error(
                    "publication.manifest_rename",
                    "manifest_rename_and_sync",
                    &error,
                )
            },
        )?;
    }
    #[cfg(test)]
    super::tests::checkpoint(&config.state_root, "recovery-committed")?;
    #[cfg(test)]
    super::tests::replace_publication_path(config, "committed");
    check_paths(
        config,
        descriptor,
        &database_proof,
        &final_database,
        intent_identity,
    )?;
    consume(config, descriptor)
}

fn intent_identity(config: &RuntimeConfig) -> Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(config.state_root.join(INTENT))
        .map_err(|error| io::io_error("publication.intent_identity", "intent_metadata", &error))?;
    Ok((metadata.dev(), metadata.ino()))
}

fn check_paths(
    config: &RuntimeConfig,
    descriptor: &Descriptor,
    proof: &crate::ledger_storage::ClosedInspection,
    database: &Path,
    intent: (u64, u64),
) -> Result<()> {
    super::retained_transfer(config)?;
    proof
        .verify_at(database)
        .map_err(|_| hold_at("publication.intent_path_replaced"))?;
    crate::retirement_format::require_absent(config)?;
    crate::persistent_home::verify(config)?;
    if intent_identity(config)? != intent
        || io::read(&config.state_root.join(INTENT))? != descriptor.bytes()?
    {
        return Err(hold_at("publication.intent_identity"));
    }
    for name in [TEMP, MANIFEST] {
        let path = config.state_root.join(name);
        if io::exists(&path)? && io::read(&path)? != descriptor.bytes()? {
            return Err(hold_at("publication.intent_descriptor"));
        }
    }
    for path in descriptor.base_markers() {
        if io::exists(&path)? {
            descriptor.verify([path])?;
        }
    }
    Ok(())
}

pub(super) fn consume(config: &RuntimeConfig, descriptor: &Descriptor) -> Result<()> {
    if io::read(&config.state_root.join(INTENT))? != descriptor.bytes()? {
        return Err(hold_at("publication.intent_identity"));
    }
    #[cfg(test)]
    super::tests::checkpoint(&config.state_root, "intent-removal")?;
    crate::fs_durability::remove_file(&config.state_root.join(INTENT)).map_err(|error| {
        io::io_error(
            "publication.intent_consume",
            "intent_remove_and_sync",
            &error,
        )
    })?;
    #[cfg(test)]
    super::tests::checkpoint(&config.state_root, "intent-consumed")?;
    Ok(())
}
