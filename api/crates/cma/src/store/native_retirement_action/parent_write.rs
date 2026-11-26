use super::*;

pub(in crate::store) fn preserve_custody(
    row: &Document,
    current: &Sandbox,
    next: &Sandbox,
) -> Result<(), SandboxStoreError> {
    if let Some(action) = action_from(row)? {
        if !action.matches(current)
            || !action.matches(next)
            || current.lifecycle != next.lifecycle
            || !preserves_credential_custody(current, next)
        {
            return Err(SandboxStoreError::Conflict);
        }
    }
    Ok(())
}

fn preserves_credential_custody(current: &Sandbox, next: &Sandbox) -> bool {
    if current.credential == next.credential {
        return true;
    }
    // Healthy recovery can settle ordinary revocations while dispatch custody
    // stays on the original runtime. It cannot publish new key authority.
    let mut settled = current.credential.clone();
    settled.current = None;
    settled.pending = None;
    settled.delivered = false;
    settled.revoke_debt.clear();
    next.credential == settled
}
