use serde::{Deserialize, Serialize};

use super::ProviderError;

/// Mount a previously provisioned, namespaced volume through the sandbox provider.
/// Allocation, ownership and release remain the caller's durable responsibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderVolume {
    pub name: String,
    pub pvc: ProviderPersistentVolumeClaim,
    pub mount_path: String,
    pub read_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderPersistentVolumeClaim {
    pub claim_name: String,
    /// Always false: provider-side implicit allocation has no CMA ownership fence.
    pub create_if_not_exists: bool,
    /// Always false: only recorded CMA cleanup may release a durable home.
    pub delete_on_sandbox_termination: bool,
}

pub(super) fn validate(volumes: &[ProviderVolume]) -> Result<(), ProviderError> {
    for (index, volume) in volumes.iter().enumerate() {
        if !dns_name(&volume.name, 63) || !dns_name(&volume.pvc.claim_name, 253) {
            return Err(invalid(
                "volume and claim names must be provider DNS labels",
            ));
        }
        if volume.pvc.create_if_not_exists {
            return Err(invalid(
                "volume claims must be allocated before runtime creation",
            ));
        }
        if volume.pvc.delete_on_sandbox_termination {
            return Err(invalid(
                "volume cleanup must remain under durable CMA authority",
            ));
        }
        if !volume
            .mount_path
            .strip_prefix('/')
            .is_some_and(relative_path)
            || volume
                .sub_path
                .as_deref()
                .is_some_and(|path| !relative_path(path))
        {
            return Err(invalid(
                "volume paths must be normalized and cannot replace root",
            ));
        }
        for previous in &volumes[..index] {
            if previous.name == volume.name || overlaps(&previous.mount_path, &volume.mount_path) {
                return Err(invalid("volume names and mount paths must not overlap"));
            }
        }
    }
    Ok(())
}

fn dns_name(name: &str, maximum: usize) -> bool {
    !name.is_empty()
        && name.len() <= maximum
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && name
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && name
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
}

fn relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.chars().any(|ch| ch.is_control() || ch == '\\')
        && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

fn overlaps(left: &str, right: &str) -> bool {
    left == right
        || left
            .strip_prefix(right)
            .is_some_and(|tail| tail.starts_with('/'))
        || right
            .strip_prefix(left)
            .is_some_and(|tail| tail.starts_with('/'))
}

fn invalid(message: &str) -> ProviderError {
    ProviderError::Malformed(message.into())
}

#[cfg(test)]
#[path = "volumes_tests.rs"]
mod tests;
