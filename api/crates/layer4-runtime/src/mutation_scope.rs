//! Filesystem mutations outlive a runtime; Agent processes belong to one generation.

use std::path::{Path, PathBuf};

use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
use crate::mutation_journal::Subject;

pub(crate) fn generation(config: &RuntimeConfig, subject: &Subject) -> Option<u64> {
    (config.home_identity.is_some()
        && matches!(
            subject,
            Subject::OpenAgent { .. } | Subject::CloseAgent { .. }
        ))
    .then_some(config.generation)
}

pub(crate) fn root(config: &RuntimeConfig, subject: &Subject) -> PathBuf {
    match generation(config, subject) {
        Some(generation) => process_root(config, generation),
        None => config.state_root.clone(),
    }
}

pub(crate) fn roots(config: &RuntimeConfig) -> Result<Vec<PathBuf>> {
    crate::persistent_home::verify(config)?;
    let mut roots = vec![config.state_root.clone()];
    if config.home_identity.is_some() {
        roots.push(process_root(config, config.generation));
    }
    for root in &roots {
        validate_directory(root)?;
    }
    Ok(roots)
}

fn process_root(config: &RuntimeConfig, generation: u64) -> PathBuf {
    config
        .state_root
        .join(format!("process-generation-{generation}"))
}

fn validate_directory(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(RuntimeError::rejected(
            "mutation_scope_invalid",
            "mutation generation root is not a real directory",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(RuntimeError::unavailable(
            "mutation_scope_unavailable",
            format!("inspect mutation generation root: {error}"),
        )),
    }
}
