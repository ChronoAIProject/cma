//! Retain complete provider prefixes without exposing a partial routed set.
use super::*;
use crate::InventoryCursor;
use std::collections::BTreeSet;

pub(super) struct Cursor {
    router: RoutedHomeVolumes,
    route: usize,
    child: Option<Box<dyn InventoryCursor<HomeVolumeAttachment>>>,
    items: Vec<HomeVolumeAttachment>,
    volumes: BTreeSet<String>,
}

impl Cursor {
    pub(super) fn new(router: RoutedHomeVolumes) -> Self {
        Self {
            router,
            route: 0,
            child: None,
            items: vec![],
            volumes: BTreeSet::new(),
        }
    }
}

#[async_trait::async_trait]
impl InventoryCursor<HomeVolumeAttachment> for Cursor {
    async fn advance(&mut self) -> Result<Option<Vec<HomeVolumeAttachment>>> {
        if self.route == 2 {
            return Ok(Some(std::mem::take(&mut self.items)));
        }
        let native = self.route == 1;
        let route = if native {
            &self.router.native
        } else {
            &self.router.local
        };
        let Some(route) = route else {
            self.route += 1;
            return Ok(None);
        };
        if self.child.is_none() {
            self.child = route.store.inventory_cursor();
        }
        let items = match &mut self.child {
            Some(child) => child.advance().await?,
            None => Some(route.store.inventory().await?),
        };
        let Some(items) = items else { return Ok(None) };
        // The child returned its complete set; a rejected route must start
        // again, never reuse a drained child or expose its invalid result.
        self.child = None;
        self.accept_route(&items, native)?;
        self.route += 1;
        Ok(None)
    }
}

impl Cursor {
    fn accept_route(&mut self, items: &[HomeVolumeAttachment], native: bool) -> Result<()> {
        // Validate a complete route atomically before retaining its prefix.
        let mut volumes = self.volumes.clone();
        let mut accepted = Vec::new();
        for attachment in items {
            attachment.validate_for(&attachment.request.sandbox_id)?;
            if self.router.is_native(&attachment.request)? != native {
                if self.items.iter().any(|prior| prior == attachment) {
                    continue;
                }
                return Err(rejected("home inventory is outside its recorded cluster"));
            }
            if !volumes.insert(attachment.request.volume_id.clone()) {
                return Err(rejected(
                    "home inventory repeats an allocation across providers",
                ));
            }
            accepted.push(attachment.clone());
        }
        self.items.extend(accepted);
        self.volumes = volumes;
        Ok(())
    }
}
