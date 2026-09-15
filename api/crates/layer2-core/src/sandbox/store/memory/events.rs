use super::*;

pub(super) fn bounded_events<T: Clone>(
    events: Option<&Vec<T>>,
    after: u64,
    limit: u16,
    sequence: impl Fn(&T) -> u64,
) -> Result<Vec<T>, SandboxStoreError> {
    if limit == 0 || limit > MAX_PAGE_SIZE {
        return Err(SandboxStoreError::InvalidPage);
    }
    Ok(events
        .into_iter()
        .flatten()
        .filter(|event| sequence(event) > after)
        .take(usize::from(limit))
        .cloned()
        .collect())
}

pub(super) fn append_sandbox_event(
    state: &mut State,
    sandbox_id: &SandboxId,
    appended_at: String,
    kind: SandboxEventKind,
) -> Result<SandboxEvent, SandboxStoreError> {
    if !state.sandboxes.contains_key(sandbox_id) {
        return Err(SandboxStoreError::NotFound);
    }
    let events = state.sandbox_events.entry(sandbox_id.clone()).or_default();
    let seq = u64::try_from(events.len())
        .map_err(|_| SandboxStoreError::SequenceConflict)?
        .checked_add(1)
        .ok_or(SandboxStoreError::SequenceConflict)?;
    let event = SandboxEvent {
        sandbox_id: sandbox_id.clone(),
        seq,
        appended_at,
        kind,
    };
    events.push(event.clone());
    Ok(event)
}
