use std::time::{Duration, SystemTime};

use layer2_core::sandbox::{Sandbox, SandboxCredentialAuthority};
use layer2_core::seconds_from_rfc3339;

use crate::sandbox::credential::CredentialMaterial;

use super::super::{CREDENTIAL_EXPIRY_MARGIN, CREDENTIAL_ROTATION_AGE};

pub(in crate::sandbox::worker) fn material_matches(
    sandbox: &Sandbox,
    pending: &SandboxCredentialAuthority,
    material: &CredentialMaterial,
) -> bool {
    material.sandbox_id == sandbox.id
        && material.owner == sandbox.owner
        && material.source == sandbox.credential.source
        && material.matches_source()
        && material.matches_connection(sandbox)
        && &material.authority == pending
}

pub(in crate::sandbox::worker) fn credential_valid_for_delivery(
    sandbox: &Sandbox,
    now: SystemTime,
) -> bool {
    if !sandbox.credential.source.is_managed() {
        return sandbox
            .credential
            .pending
            .as_ref()
            .is_some_and(|pending| sandbox.credential.source.accepts(pending));
    }
    let now = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    sandbox
        .credential
        .pending
        .as_ref()
        .and_then(|pending| seconds_from_rfc3339(&pending.expires_at))
        .is_some_and(|expires| expires > now.saturating_add(CREDENTIAL_EXPIRY_MARGIN.as_secs()))
}

pub(in crate::sandbox::worker) fn credential_rotation_at(sandbox: &Sandbox) -> Option<SystemTime> {
    if !sandbox.credential.source.is_managed() {
        return None;
    }
    let current = sandbox.credential.current.as_ref()?;
    let issued = seconds_from_rfc3339(&current.issued_at)?;
    let expires = seconds_from_rfc3339(&current.expires_at)?;
    let age_due = issued.checked_add(CREDENTIAL_ROTATION_AGE.as_secs())?;
    let expiry_due = expires.checked_sub(CREDENTIAL_EXPIRY_MARGIN.as_secs())?;
    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(age_due.min(expiry_due)))
}
