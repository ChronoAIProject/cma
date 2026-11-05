//! Page acceptance is atomic; an incomplete scan never grants cleanup authority.
use super::corrupt;
use layer2_core::{
    sandbox::{
        store::{Page, PageRequest},
        Sandbox, SandboxStoreError,
    },
    SandboxId,
};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Authority {
    pub(super) request: PageRequest,
    pub(super) items: BTreeMap<SandboxId, Sandbox>,
}

impl Authority {
    pub(super) fn accept(&mut self, page: Page<Sandbox>) -> Result<bool, SandboxStoreError> {
        let mut previous = self.request.cursor.as_deref();
        for sandbox in &page.items {
            sandbox
                .validate()
                .map_err(|error| corrupt(&error.to_string()))?;
            if previous.is_some_and(|id| id >= sandbox.id.as_str())
                || self.items.contains_key(&sandbox.id)
            {
                return Err(corrupt("runtime authority scan is not strictly ordered"));
            }
            previous = Some(sandbox.id.as_str());
        }
        if page.items.len() > usize::from(self.request.limit)
            || page
                .next
                .as_ref()
                .is_some_and(|next| page.items.is_empty() || previous != Some(next.as_str()))
        {
            return Err(corrupt("runtime authority scan did not advance completely"));
        }
        self.items.extend(
            page.items
                .into_iter()
                .map(|sandbox| (sandbox.id.clone(), sandbox)),
        );
        self.request.cursor = page.next;
        Ok(self.request.cursor.is_none())
    }
}
