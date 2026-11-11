//! Replace exactly one proved installation; the published directory is never absent.
use crate::{
    agent_skill_install::{root, MARKER},
    config::RuntimeConfig,
    failure::{Result, RuntimeError},
};
use layer3_adapter::{AgentSkillInstallReceipt, AgentSkillInstallRequest};
use serde::{Deserialize, Serialize};
use std::{
    fs::OpenOptions,
    io::Write as _,
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
};

#[cfg(test)]
#[path = "agent_skill_upgrade_tests.rs"]
mod tests;
#[path = "agent_skill_upgrade_tree.rs"]
mod tree;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Proof {
    request_sha256: String,
    previous_tree: String,
    new_tree: String,
}

pub(crate) fn conflict(detail: &str) -> RuntimeError {
    RuntimeError::rejected("agent_skill_upgrade_conflict", detail)
}
fn io(error: std::io::Error) -> RuntimeError {
    RuntimeError::unavailable(
        "agent_skill_upgrade_unavailable",
        format!("skill replacement: {error}"),
    )
}
fn staging(parent: &Path, request: &AgentSkillInstallRequest) -> PathBuf {
    parent.join(format!(".cma-upgrade-{}", request.request_sha256))
}
fn proof_path(config: &RuntimeConfig, request: &AgentSkillInstallRequest) -> Result<PathBuf> {
    let parent = config.state_root.join("skill-upgrades");
    if !crate::paths::path_is_directory(&config.state_root) {
        return Err(conflict("runtime state is not a directory"));
    }
    match std::fs::create_dir(&parent) {
        Ok(()) => crate::fs_durability::sync_parent(&parent).map_err(io)?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(io(error)),
    }
    if !crate::paths::path_is_directory(&parent) {
        return Err(conflict("upgrade evidence parent is not a directory"));
    }
    Ok(parent.join(format!("{}.json", request.request_sha256)))
}
fn load_proof(path: &Path, request: &AgentSkillInstallRequest) -> Result<Option<Proof>> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io(error)),
        Ok(m) if m.is_file() && !m.is_symlink() && m.len() <= 4096 => {}
        _ => return Err(conflict("invalid upgrade evidence")),
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(io)?;
    let proof: Proof =
        serde_json::from_reader(file).map_err(|_| conflict("invalid upgrade evidence"))?;
    let digest = |v: &str| {
        v.len() == 64
            && v.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    if proof.request_sha256 != request.request_sha256
        || !digest(&proof.previous_tree)
        || !digest(&proof.new_tree)
    {
        return Err(conflict("upgrade evidence belongs to another request"));
    }
    Ok(Some(proof))
}
fn publish_proof(path: &Path, proof: &Proof) -> Result<()> {
    let temporary = path.with_extension("partial");
    match std::fs::symlink_metadata(&temporary) {
        Ok(m) if m.is_file() && !m.is_symlink() => std::fs::remove_file(&temporary).map_err(io)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        _ => return Err(conflict("unexpected partial upgrade evidence")),
    }
    write_new(
        &temporary,
        &serde_json::to_vec(proof).expect("serializable proof"),
    )?;
    crate::agent_skill_install::rename_new(&temporary, path).map_err(io)?;
    crate::fs_durability::sync_parent(path).map_err(io)
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(io)?;
    file.write_all(bytes).map_err(io)?;
    file.sync_all().map_err(io)
}
fn clean(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() && !m.is_symlink() => {
            crate::fs_durability::remove_dir_all(path).map_err(io)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io(error)),
        _ => Err(conflict("upgrade staging is not a real directory")),
    }
}

pub(crate) fn preflight(config: &RuntimeConfig, request: &AgentSkillInstallRequest) -> Result<()> {
    let parent = root(config, request)?;
    if !crate::paths::path_is_directory(&parent.join(&request.skill.id)) {
        return Err(conflict(
            "previous skill destination is not a real directory",
        ));
    }
    Ok(())
}

pub(crate) async fn install(
    config: &RuntimeConfig,
    client: &reqwest::Client,
    request: &AgentSkillInstallRequest,
) -> Result<AgentSkillInstallReceipt> {
    crate::agent_skill_install::validate(request)?;
    let result = replace(config, client, request).await;
    if result.as_ref().is_err_and(|error| !error.retryable) {
        if let Ok(parent) = root(config, request) {
            let evidence = config
                .state_root
                .join("skill-upgrades")
                .join(format!("{}.json", request.request_sha256));
            // Terminal pre-publication refusals must not accumulate extracted
            // archives. After proof publication, retain ambiguous evidence.
            if std::fs::symlink_metadata(&evidence)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            {
                clean(&staging(&parent, request))?;
            }
        }
    }
    result
}

async fn replace(
    config: &RuntimeConfig,
    client: &reqwest::Client,
    request: &AgentSkillInstallRequest,
) -> Result<AgentSkillInstallReceipt> {
    let upgrade = request
        .upgrade
        .as_ref()
        .ok_or_else(|| conflict("upgrade intent is absent"))?;
    let parent = root(config, request)?;
    let target = parent.join(&request.skill.id);
    let stage = staging(&parent, request);
    let candidate = stage.join("new");
    let expected = stage.join("previous");
    let evidence = proof_path(config, request)?;
    let receipt = AgentSkillInstallReceipt::from_request(request);
    let proof = match load_proof(&evidence, request)? {
        Some(proof) => proof,
        None => {
            preflight(config, request)?;
            clean(&stage)?;
            crate::fs_durability::create_dir(&stage).map_err(io)?;
            crate::skills::install_prepared(config, client, &upgrade.previous, &expected).await?;
            point(config, "previous_extracted")?;
            // A profile installation has no marker. Live installations must carry
            // the exact previous receipt; a marker alone never proves its files.
            if std::fs::symlink_metadata(expected.join(MARKER)).is_ok() {
                return Err(conflict("previous archive contains a reserved receipt"));
            }
            let previous_tree = tree::digest(&expected, None)?;
            if tree::digest(&target, upgrade.previous_receipt.as_ref())? != previous_tree {
                return Err(conflict(
                    "previous installation was modified or has unexpected content",
                ));
            }
            crate::skills::install_prepared(config, client, &request.skill, &candidate).await?;
            point(config, "new_extracted")?;
            let new_tree = tree::digest(&candidate, None)?;
            write_new(
                &candidate.join(MARKER),
                &serde_json::to_vec(&receipt).expect("serializable receipt"),
            )?;
            crate::fs_durability::sync_tree(&stage).map_err(io)?;
            point(config, "receipt_staged")?;
            let proof = Proof {
                request_sha256: request.request_sha256.clone(),
                previous_tree,
                new_tree,
            };
            publish_proof(&evidence, &proof)?;
            point(config, "proof_published")?;
            proof
        }
    };
    // Repair a prior ambiguous proof-publication sync before any directory swap.
    crate::fs_durability::sync_file(&evidence).map_err(io)?;
    crate::fs_durability::sync_parent(&evidence).map_err(io)?;
    root(config, request)?;
    // The sidecar is durable before exchange. It permits recovery even when the
    // original archives are unavailable, including after partial old-tree cleanup.
    if tree::receipt_matches(&target, &receipt)? {
        if tree::digest(&target, Some(&receipt))? != proof.new_tree {
            return Err(conflict("published replacement was modified"));
        }
    } else {
        if tree::digest(&target, upgrade.previous_receipt.as_ref())? != proof.previous_tree
            || tree::digest(&candidate, Some(&receipt))? != proof.new_tree
        {
            return Err(conflict(
                "previous installation or staged replacement changed",
            ));
        }
        // Descriptor-relative exchange refuses symlinked parents and is serialized
        // with every other supervisor mutation by the operation gate.
        tree::exchange(config, request, &stage)?;
        point(config, "exchanged")?;
    }
    crate::fs_durability::sync_parent(&target).map_err(io)?;
    if stage.exists() {
        crate::fs_durability::sync_directory(&stage).map_err(io)?;
    }
    point(config, "exchange_synced")?;
    clean(&stage)?;
    point(config, "old_removed")?;
    // Keep the small proof beside runtime journal evidence. A crash before the
    // terminal journal receipt can recover without fetching either archive again.
    Ok(receipt)
}

fn point(_config: &RuntimeConfig, _name: &str) -> Result<()> {
    #[cfg(test)]
    {
        let mut points = tests::FAULTS.lock().unwrap();
        if points
            .get(&_config.state_root)
            .is_some_and(|name| *name == _name)
        {
            points.remove(&_config.state_root);
            return Err(RuntimeError::unavailable("injected_crash", _name));
        }
    }
    Ok(())
}
