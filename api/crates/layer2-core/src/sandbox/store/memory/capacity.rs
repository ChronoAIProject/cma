use super::*;

pub(super) fn read(
    store: &InMemorySandboxStore,
    owner: &SubjectId,
) -> Result<crate::sandbox::SandboxCapacitySnapshot, SandboxStoreError> {
    let state = store.lock();
    let mut counts: Vec<(SandboxStatus, u64)> = Vec::new();
    let mut retiring_released = 0;
    for sandbox in state
        .sandboxes
        .values()
        .filter(|sandbox| &sandbox.owner == owner && sandbox.retired_at.is_none())
    {
        if sandbox.retirement_released_owner_slot() {
            retiring_released += 1;
            continue;
        }
        if let Some((_, count)) = counts
            .iter_mut()
            .find(|(status, _)| status == &sandbox.status)
        {
            *count += 1;
        } else {
            counts.push((sandbox.status, 1));
        }
    }
    Ok(crate::sandbox::SandboxCapacitySnapshot {
        counts,
        retiring_released,
        public_owner_limit: None,
        observed_at: crate::now_rfc3339(),
    })
}
