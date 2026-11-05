use super::*;
use crate::{
    inventory_cursor::{request, Child},
    InventoryCursor,
};
use std::collections::BTreeSet;

pub(super) struct Cursor {
    store: PersistentHomeVolumes,
    params: ListParams,
    cursors: BTreeSet<String>,
    volumes: BTreeSet<String>,
    items: Vec<HomeVolumeAttachment>,
}

impl Cursor {
    pub(super) fn new(store: PersistentHomeVolumes) -> Self {
        let params = ListParams::default()
            .labels(&format!("cma/home-managed-by={}", store.controller))
            .limit(200);
        Self {
            store,
            params,
            cursors: BTreeSet::new(),
            volumes: BTreeSet::new(),
            items: vec![],
        }
    }
}

#[async_trait::async_trait]
impl InventoryCursor<HomeVolumeAttachment> for Cursor {
    async fn advance(&mut self) -> Result<Option<Vec<HomeVolumeAttachment>>> {
        let page = request(Child::RouteList, async {
            self.store
                .claims
                .list(&self.params)
                .await
                .map_err(|error| failure("list durable home authority", error))
        })
        .await?;
        let mut volumes = self.volumes.clone();
        let mut items = Vec::new();
        for claim in page.items {
            let request = identity::recorded_request(&claim)?;
            let receipt = self.store.receipt(&request, &claim)?;
            receipt.validate_for(&request)?;
            if !volumes.insert(request.volume_id.clone()) {
                return Err(AdapterError::Rejected(
                    "durable home inventory repeats an identity".into(),
                ));
            }
            items.push(HomeVolumeAttachment { request, receipt });
        }
        let next = page.metadata.continue_.filter(|value| !value.is_empty());
        if next
            .as_ref()
            .is_some_and(|cursor| self.cursors.contains(cursor))
        {
            return Err(AdapterError::Rejected(
                "durable home inventory did not advance".into(),
            ));
        }
        // No checkpoint changes until the whole page has validated.
        self.items.extend(items);
        self.volumes = volumes;
        if let Some(cursor) = next {
            self.params = self.params.clone().continue_token(&cursor);
            self.cursors.insert(cursor);
            Ok(None)
        } else {
            Ok(Some(std::mem::take(&mut self.items)))
        }
    }
}
