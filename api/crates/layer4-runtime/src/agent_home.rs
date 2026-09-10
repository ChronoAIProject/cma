use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use layer3_adapter::{AgentHomePlan, HomeFile, HomeSymlink};
use sha2::{Digest, Sha256};

use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
use crate::paths::{lexical_target, safe_component, safe_relative};

const MAX_HOME_FILE_BYTES: usize = 16 * 1024 * 1024;
const MAX_HOME_BYTES: usize = 64 * 1024 * 1024;
const MAX_HOME_ENTRIES: usize = 8_192;
const MAX_SKILLS: usize = 128;

#[derive(Clone, Copy, PartialEq, Eq)]
enum PlannedKind {
    File,
    Symlink,
    Skill,
}

pub(crate) async fn materialize(
    config: &RuntimeConfig,
    client: &reqwest::Client,
    plan: &AgentHomePlan,
) -> Result<PathBuf> {
    safe_component(plan.agent_id.as_str(), "agent id")?;
    let planned = validate_plan(plan)?;
    let target = config.agent_home(plan.agent_id.as_str());
    let agent_root = target.parent().expect("an agent home has a parent");
    ensure_parents(&config.home, agent_root)?;
    if entry_exists(&target, "inspect existing agent home")? {
        return Err(RuntimeError::rejected(
            "agent_home_exists",
            "agent home already exists",
        ));
    }
    let staging = staging_path(config, plan.agent_id.as_str(), &plan.operation_id);
    if entry_exists(&staging, "inspect agent home staging path")? {
        return Err(RuntimeError::rejected(
            "agent_home_staging_conflict",
            "agent home staging path already exists",
        ));
    }
    let mut cleanup = Cleanup::new(staging.clone());
    crate::fs_durability::create_dir(&staging).map_err(home_io("create staged agent home"))?;

    for file in &plan.files {
        write_file(&staging, file)?;
    }
    for skill in &plan.skills {
        let destination = staging.join("skills").join(&skill.id);
        ensure_parents(&staging, destination.parent().unwrap())?;
        crate::skills::install_prepared(config, client, skill, &destination).await?;
    }
    for link in &plan.symlinks {
        write_symlink(&staging, &target, &config.home, link)?;
    }
    verify_materialized(&staging, &planned)?;
    crate::fs_durability::rename(&staging, &target).map_err(home_io("publish agent home"))?;
    cleanup.disarm();
    Ok(target)
}

pub(crate) fn preflight_materialize(config: &RuntimeConfig, plan: &AgentHomePlan) -> Result<()> {
    safe_component(plan.agent_id.as_str(), "agent id")?;
    validate_plan(plan)?;
    let target = config.agent_home(plan.agent_id.as_str());
    let parent = target.parent().expect("an agent home has a parent");
    if existing_parents_are_directories(&config.home, parent)? {
        if entry_exists(&target, "inspect existing agent home")? {
            return Err(RuntimeError::rejected(
                "agent_home_exists",
                "agent home already exists",
            ));
        }
        let staging = staging_path(config, plan.agent_id.as_str(), &plan.operation_id);
        if entry_exists(&staging, "inspect agent home staging path")? {
            return Err(RuntimeError::rejected(
                "agent_home_staging_conflict",
                "agent home staging path already exists",
            ));
        }
    }
    Ok(())
}

pub(crate) fn recover_materialization(
    config: &RuntimeConfig,
    agent_id: &str,
    operation_id: &str,
) -> Result<Option<PathBuf>> {
    safe_component(agent_id, "agent id")?;
    let target = config.agent_home(agent_id);
    let parent = target.parent().expect("an agent home has a parent");
    if !existing_parents_are_directories(&config.home, parent)? {
        return Ok(None);
    }
    let staging = staging_path(config, agent_id, operation_id);
    let target_exists = real_directory_or_absent(&target, "agent home")?;
    let staging_exists = real_directory_or_absent(&staging, "agent home staging path")?;
    match (target_exists, staging_exists) {
        (true, false) => Ok(Some(target)),
        (false, true) => {
            crate::fs_durability::remove_dir_all(&staging)
                .map_err(home_io("remove incomplete staged agent home"))?;
            Ok(None)
        }
        (false, false) => Ok(None),
        (true, true) => Err(RuntimeError::rejected(
            "agent_home_create_indeterminate",
            "agent home has both staged and published directories",
        )),
    }
}

pub(crate) fn remove(config: &RuntimeConfig, agent_id: &str) -> Result<bool> {
    safe_component(agent_id, "agent id")?;
    let home = config.agent_home(agent_id);
    let parent = home.parent().expect("an agent home has a parent");
    if !existing_parents_are_directories(&config.home, parent)? {
        return Ok(false);
    }
    match std::fs::symlink_metadata(&home) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(RuntimeError::rejected(
                "agent_home_invalid",
                "agent home is not a directory",
            ))
        }
        Err(error) => return Err(home_io("inspect agent home")(error)),
    }
    crate::fs_durability::remove_dir_all(&home).map_err(home_io("remove agent home"))?;
    if let Some(parent) = home.parent() {
        let _ = crate::fs_durability::remove_dir(parent);
    }
    Ok(true)
}

pub(crate) fn preflight_remove(config: &RuntimeConfig, agent_id: &str) -> Result<()> {
    safe_component(agent_id, "agent id")?;
    let home = config.agent_home(agent_id);
    let parent = home.parent().expect("an agent home has a parent");
    if existing_parents_are_directories(&config.home, parent)? {
        real_directory_or_absent(&home, "agent home")?;
    }
    Ok(())
}

fn validate_plan(plan: &AgentHomePlan) -> Result<BTreeMap<PathBuf, PlannedKind>> {
    let entries = plan
        .files
        .len()
        .checked_add(plan.symlinks.len())
        .and_then(|entries| entries.checked_add(plan.skills.len()))
        .ok_or_else(|| RuntimeError::rejected("agent_home_too_large", "agent home is too large"))?;
    if entries > MAX_HOME_ENTRIES || plan.skills.len() > MAX_SKILLS {
        return Err(RuntimeError::rejected(
            "agent_home_too_large",
            format!("agent home exceeds {MAX_HOME_ENTRIES} entries or {MAX_SKILLS} skills"),
        ));
    }
    let mut planned = BTreeMap::new();
    let total = plan.files.iter().try_fold(0_usize, |total, file| {
        if file.bytes.len() > MAX_HOME_FILE_BYTES {
            return Err(RuntimeError::rejected(
                "agent_home_too_large",
                format!("agent home file {} exceeds its size limit", file.path),
            ));
        }
        total.checked_add(file.bytes.len()).ok_or_else(|| {
            RuntimeError::rejected("agent_home_too_large", "agent home is too large")
        })
    })?;
    if total > MAX_HOME_BYTES {
        return Err(RuntimeError::rejected(
            "agent_home_too_large",
            "agent home files exceed their aggregate size limit",
        ));
    }
    for file in &plan.files {
        insert_plan(
            &mut planned,
            safe_relative(&file.path, "home file path")?,
            PlannedKind::File,
        )?;
    }
    for link in &plan.symlinks {
        insert_plan(
            &mut planned,
            safe_relative(&link.path, "home symlink path")?,
            PlannedKind::Symlink,
        )?;
        if link.target.is_empty() || link.target.contains('\0') {
            return Err(RuntimeError::rejected(
                "agent_home_invalid",
                "agent home symlink target is empty or contains NUL",
            ));
        }
    }
    let mut skill_ids = std::collections::BTreeSet::new();
    for skill in &plan.skills {
        safe_component(&skill.id, "skill id")?;
        if !skill_ids.insert(&skill.id) {
            return Err(RuntimeError::rejected(
                "duplicate_skill",
                format!("skill {} is listed more than once", skill.id),
            ));
        }
        insert_plan(
            &mut planned,
            PathBuf::from("skills").join(&skill.id),
            PlannedKind::Skill,
        )?;
    }
    for (path, kind) in &planned {
        for ancestor in path
            .ancestors()
            .skip(1)
            .filter(|path| !path.as_os_str().is_empty())
        {
            if planned.contains_key(ancestor) {
                return Err(RuntimeError::rejected(
                    "agent_home_path_conflict",
                    format!(
                        "agent home path {} is below another planned entry",
                        path.display()
                    ),
                ));
            }
        }
        let _ = kind;
    }
    Ok(planned)
}

fn insert_plan(
    planned: &mut BTreeMap<PathBuf, PlannedKind>,
    path: PathBuf,
    kind: PlannedKind,
) -> Result<()> {
    if planned.insert(path.clone(), kind).is_some() {
        return Err(RuntimeError::rejected(
            "agent_home_path_conflict",
            format!(
                "agent home path {} is listed more than once",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn write_file(root: &Path, file: &HomeFile) -> Result<()> {
    let relative = safe_relative(&file.path, "home file path")?;
    let path = root.join(relative);
    ensure_parents(root, path.parent().unwrap())?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(home_io("create agent home file"))?;
    output
        .write_all(&file.bytes)
        .map_err(home_io("write agent home file"))?;
    output.sync_all().map_err(home_io("sync agent home file"))?;
    crate::fs_durability::sync_parent(&path).map_err(home_io("sync agent home file parent"))
}

fn write_symlink(root: &Path, final_root: &Path, sandbox: &Path, link: &HomeSymlink) -> Result<()> {
    let relative = safe_relative(&link.path, "home symlink path")?;
    let staged_path = root.join(&relative);
    let final_path = final_root.join(&relative);
    let target = Path::new(&link.target);
    if lexical_target(sandbox, &final_path, target).is_none() {
        return Err(RuntimeError::rejected(
            "agent_home_symlink_escape",
            format!("agent home symlink {} escapes the sandbox home", link.path),
        ));
    }
    ensure_parents(root, staged_path.parent().unwrap())?;
    crate::fs_durability::symlink(target, &staged_path)
        .map_err(home_io("create agent home symlink"))
}

fn ensure_parents(root: &Path, parent: &Path) -> Result<()> {
    if !crate::paths::path_is_directory(root) {
        return Err(RuntimeError::rejected(
            "agent_home_invalid",
            "sandbox home is not a real directory",
        ));
    }
    let relative = parent.strip_prefix(root).map_err(|_| {
        RuntimeError::rejected("agent_home_invalid", "agent home parent escaped its root")
    })?;
    let mut current = root.to_path_buf();
    for part in relative.components() {
        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(RuntimeError::rejected(
                    "agent_home_path_conflict",
                    "agent home parent is not a directory",
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                crate::fs_durability::create_dir(&current)
                    .map_err(home_io("create agent home directory"))?;
            }
            Err(error) => return Err(home_io("inspect agent home directory")(error)),
        }
    }
    Ok(())
}

fn existing_parents_are_directories(root: &Path, parent: &Path) -> Result<bool> {
    if !crate::paths::path_is_directory(root) {
        return Err(RuntimeError::rejected(
            "agent_home_invalid",
            "sandbox home is not a real directory",
        ));
    }
    let relative = parent.strip_prefix(root).map_err(|_| {
        RuntimeError::rejected("agent_home_invalid", "agent home parent escaped its root")
    })?;
    let mut current = root.to_path_buf();
    for part in relative.components() {
        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(RuntimeError::rejected(
                    "agent_home_invalid",
                    "agent home parent is not a real directory",
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(home_io("inspect agent home parent")(error)),
        }
    }
    Ok(true)
}

fn verify_materialized(root: &Path, planned: &BTreeMap<PathBuf, PlannedKind>) -> Result<()> {
    for (relative, kind) in planned {
        let metadata = std::fs::symlink_metadata(root.join(relative))
            .map_err(home_io("verify agent home entry"))?;
        let valid = match kind {
            PlannedKind::File => metadata.is_file() && !metadata.file_type().is_symlink(),
            PlannedKind::Symlink => metadata.file_type().is_symlink(),
            PlannedKind::Skill => metadata.is_dir() && !metadata.file_type().is_symlink(),
        };
        if !valid {
            return Err(RuntimeError::rejected(
                "agent_home_invalid",
                format!("agent home entry {} has the wrong type", relative.display()),
            ));
        }
    }
    Ok(())
}

fn staging_path(config: &RuntimeConfig, agent_id: &str, operation_id: &str) -> PathBuf {
    let digest = Sha256::digest(operation_id.as_bytes());
    let name = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    config
        .agent_home(agent_id)
        .parent()
        .expect("an agent home has a parent")
        .join(format!(".home-staging-{name}"))
}

fn real_directory_or_absent(path: &Path, label: &str) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(RuntimeError::rejected(
            "agent_home_invalid",
            format!("{label} is not a real directory"),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(home_io("inspect agent home recovery path")(error)),
    }
}

fn entry_exists(path: &Path, operation: &'static str) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(home_io(operation)(error)),
    }
}

fn home_io(operation: &'static str) -> impl FnOnce(std::io::Error) -> RuntimeError + Copy {
    move |error| RuntimeError::unavailable("agent_home_io_failed", format!("{operation}: {error}"))
}

struct Cleanup {
    path: Option<PathBuf>,
}

impl Cleanup {
    fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    fn disarm(&mut self) {
        self.path = None;
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = crate::fs_durability::remove_dir_all(&path);
        }
    }
}

#[cfg(test)]
#[path = "agent_home_tests.rs"]
mod tests;
