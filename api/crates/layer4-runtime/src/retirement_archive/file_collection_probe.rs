//! Create-only V1 source files qualify the real completion/reopen protocol.
//! These synthetic originals never grant authority over an existing home.
use super::*;
use crate::{
    ledger_reclamation::CollectionEvidence, mutation_authority::Record as Authority,
    mutation_journal::Record as Journal,
};
use layer3_adapter::{
    runtime_ledger_collection::CollectionGroup, AgentHomeRemoveRequest, AgentSlot,
};
use std::{
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
};

pub(super) fn source(path: &Path, base: &RuntimeConfig) -> Result<RuntimeConfig> {
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .map_err(|_| invalid())?;
    let mut config = base.clone();
    config.home = path.join("home");
    config.state_root = path.join("state");
    config.checkpoint_root = path.join("transfer");
    for directory in [
        &config.home,
        &config.state_root,
        &config.checkpoint_root,
        &config.state_root.join("runtime-mutations"),
        &config.state_root.join("mutation-authority"),
    ] {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(directory)
            .map_err(|_| invalid())?;
    }
    Ok(config)
}
pub(super) fn add(config: &RuntimeConfig, name: &str, complete: bool) -> Result<Operation> {
    let operation = Operation::remove_agent_home(&AgentHomeRemoveRequest::new(
        format!("agent:{name}:home:remove:v1"),
        AgentSlot::new(name),
    ));
    let journal = Journal::started(config, &operation)?;
    let journal = if complete {
        journal.completed(&layer3_adapter::wire::AgentHomeRemoveReceipt {
            operation_id: operation.operation_id.clone(),
            request_sha256: operation.request_sha256.clone(),
            agent_id: AgentSlot::new(name),
        })?
    } else {
        journal
    };
    let authority = Authority::new(config, 1, &operation);
    for (path, bytes) in [
        (journal.inventory_name(config), journal.encode()?),
        (
            authority.inventory_name(config),
            serde_json::to_vec(&authority).map_err(|_| invalid())?,
        ),
    ] {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .map_err(|_| invalid())?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| invalid())?;
        std::fs::File::open(path.parent().ok_or_else(invalid)?)
            .and_then(|p| p.sync_all())
            .map_err(|_| invalid())?;
    }
    Ok(operation)
}
pub(super) async fn run(path: &Path, base: &RuntimeConfig) -> Result<()> {
    let config = source(&path.with_extension("file-source"), base)?;
    let id = uuid::Uuid::new_v4().to_string();
    let mut first = None;
    for (index, name) in ["file-first", "file-later"].into_iter().enumerate() {
        let operation = add(&config, name, true)?;
        let archive = Archive::open_version(
            path.into(),
            config.clone(),
            id.clone(),
            index == 0,
            256,
            Version::FileCollection,
        )
        .await?;
        let revision = archive.observe_file_collection().await?;
        let (binding, originals) = super::collection_probe::file_artifacts(
            &config,
            &id,
            revision,
            CollectionGroup::Agent {
                agent_id: name.into(),
            },
        )?;
        let evidence =
            CollectionEvidence::parse(&binding, &group_key(&operation), originals.clone())?;
        let (reference, proof) = archive
            .prepare_file_collection(evidence, "a".repeat(64))
            .await?;
        if archive.require_ready().await.is_ok()
            || reference.archive_version != 4
            || archive
                .read_collection(binding.clone(), reference.clone())
                .await?
                .originals()
                != originals.each_ref().map(Vec::as_slice)
        {
            return Err(invalid());
        }
        archive.close().await?;
        let archive = Archive::open_version(
            path.into(),
            config.clone(),
            id.clone(),
            false,
            256,
            Version::FileCollection,
        )
        .await?;
        let receipt = complete_file_collection_held(
            archive,
            binding.clone(),
            reference.clone(),
            proof.clone(),
            (),
        )
        .await?;
        if receipt.operations != 1 || receipt.authority != 1 {
            return Err(invalid());
        }
        if index == 0 {
            first = Some((binding, reference, proof, receipt));
        }
    }
    let (binding, reference, proof, receipt) = first.ok_or_else(invalid)?;
    let archive = Archive::open_version(
        path.into(),
        config.clone(),
        id.clone(),
        false,
        256,
        Version::FileCollection,
    )
    .await?;
    archive.require_ready().await?;
    if complete_file_collection_held(archive, binding, reference, proof, ()).await? != receipt {
        return Err(invalid());
    }
    if let Ok(old) =
        Archive::open_version(path.into(), config, id, false, 256, Version::Collection).await
    {
        old.close().await?;
        return Err(invalid());
    }
    Ok(())
}
