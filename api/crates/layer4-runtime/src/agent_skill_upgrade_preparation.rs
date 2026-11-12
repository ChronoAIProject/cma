//! Durable bounded preparation; terminal exhaustion prevents restart-side publication.
use super::{conflict, io, tree, write_new};
use crate::{
    agent_filesystem::io as fd,
    failure::{Result, RuntimeError},
};
use layer3_adapter::{
    AgentSkillInstallRequest, MAX_AGENT_SKILL_UPGRADE_PREPARATION_ATTEMPTS as LIMIT,
};
use std::path::Path;

pub(super) fn refusal() -> RuntimeError {
    let reason = layer3_adapter::AgentSkillUpgradeRefusal::PreparationExhausted;
    RuntimeError::rejected(reason.code(), reason.to_string())
}

fn used(evidence: &Path, request: &AgentSkillInstallRequest) -> Result<u32> {
    let parent = evidence.parent().expect("evidence parent");
    if std::fs::symlink_metadata(parent).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        return Ok(0);
    }
    let directory = tree::directory(parent)?;
    let mut count = 0;
    let mut absent = false;
    for index in 1..=LIMIT {
        let path = evidence.with_extension(format!("attempt-{index}"));
        let name = path.file_name().unwrap().to_str().unwrap();
        match fd::stat(&directory, name) {
            Err(error) if error.code == "file_not_found" => absent = true,
            Err(error) => return Err(error),
            Ok(_) => {
                if absent
                    || fd::read_regular(&directory, name, 64).ok().as_deref()
                        != Some(request.request_sha256.as_bytes())
                {
                    return Err(conflict("invalid skill preparation attempt evidence"));
                }
                count += 1;
            }
        }
    }
    Ok(count)
}

pub(super) fn begin(evidence: &Path, request: &AgentSkillInstallRequest) -> Result<()> {
    let count = used(evidence, request)?;
    if count == LIMIT {
        return Err(refusal());
    }
    let path = evidence.with_extension(format!("attempt-{}", count + 1));
    write_new(&path, request.request_sha256.as_bytes())?;
    crate::fs_durability::sync_parent(&path).map_err(io)
}

pub(super) fn exhausted(evidence: &Path, request: &AgentSkillInstallRequest) -> Result<bool> {
    Ok(used(evidence, request)? == LIMIT)
}
