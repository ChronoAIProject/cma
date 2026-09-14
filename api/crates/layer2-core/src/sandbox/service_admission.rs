use super::store::SandboxAdmissionName;
use super::SandboxServiceError;

const MAX_SANDBOX_NAME_BYTES: usize = 64;

pub(super) fn exact_name(name: String) -> Result<String, SandboxServiceError> {
    if name.trim().is_empty() || name.len() > MAX_SANDBOX_NAME_BYTES || name.contains('\0') {
        Err(SandboxServiceError::InvalidName)
    } else {
        Ok(name)
    }
}

pub(super) fn requested_name(
    name: Option<String>,
    profile_name: &str,
) -> Result<SandboxAdmissionName, SandboxServiceError> {
    match name {
        Some(name) => exact_name(name).map(SandboxAdmissionName::Exact),
        None => Ok(SandboxAdmissionName::Allocate(derived_name(profile_name))),
    }
}

fn derived_name(profile_name: &str) -> String {
    let trimmed = profile_name.trim();
    let mut end = trimmed.len().min(MAX_SANDBOX_NAME_BYTES);
    while !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    trimmed[..end].to_string()
}
