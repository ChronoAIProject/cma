//! Shared bounded cursor pagination for the in-memory stores.
use super::{Page, PageRequest, SandboxStoreError, MAX_PAGE_SIZE};

pub(super) fn page<T: Clone>(
    mut values: Vec<(String, T)>,
    request: &PageRequest,
) -> Result<Page<T>, SandboxStoreError> {
    if request.limit == 0 || request.limit > MAX_PAGE_SIZE {
        return Err(SandboxStoreError::InvalidPage);
    }
    values.sort_by(|left, right| left.0.cmp(&right.0));
    if let Some(cursor) = &request.cursor {
        values.retain(|(key, _)| key > cursor);
    }
    let has_more = values.len() > usize::from(request.limit);
    values.truncate(usize::from(request.limit));
    let next = has_more
        .then(|| values.last().map(|(key, _)| key.clone()))
        .flatten();
    Ok(Page {
        items: values.into_iter().map(|(_, value)| value).collect(),
        next,
    })
}
