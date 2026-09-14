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
