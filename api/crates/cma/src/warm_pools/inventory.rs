//! Project released pool authority and current provider observations for operators.
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use super::{current, released::ReleasedImages, NativePoolStatus, NativeWarmPools, PoolTarget};
use layer1_public::rest::admin::warm_pools::{WarmPoolError, WarmPoolList, WarmPoolView};

pub(super) struct InventoryView {
    pub(super) items: Vec<WarmPoolView>,
    pub(super) claims: [Option<String>; 2],
    pub(super) capacity_pools: [Option<String>; 2],
}

impl NativeWarmPools {
    pub(super) async fn inventory(&self) -> Result<WarmPoolList, WarmPoolError> {
        let released = self.released_images();
        let default_config_sha256 = self.default_config_sha256(&released.agent);
        let machine_config_sha256 = released
            .machine
            .as_ref()
            .and_then(|(image, _)| self.default_config_sha256(image));
        let targets = self.store.list().await?;
        let images = std::iter::once(released.agent.uri.as_str())
            .chain(released.machine.iter().map(|(image, _)| image.uri.as_str()))
            .chain(
                targets
                    .iter()
                    .map(|target| target.plan.spec.image.uri.as_str()),
            );
        let versions = async {
            match &self.versions {
                Some(versions) => versions.list(images).await,
                None => Vec::new(),
            }
        };
        let inventory = async {
            match self.adapter.native_pools() {
                Some(provider) if self.home.is_some() => {
                    tokio::time::timeout(Duration::from_secs(8), provider.inventory())
                        .await
                        .ok()
                        .and_then(Result::ok)
                }
                _ => None,
            }
        };
        let (inventory, runtime_versions) = tokio::join!(inventory, versions);
        let available = inventory.is_some();
        let live = inventory.unwrap_or_default();
        let inventory = self.inventory_view(
            &released,
            &targets,
            &live,
            available,
            [
                default_config_sha256.as_deref(),
                machine_config_sha256.as_deref(),
            ],
        );
        let [default_claim_pool_id, machine_claim_pool_id] = inventory.claims;
        let [default_capacity_pool_id, machine_capacity_pool_id] = inventory.capacity_pools;
        Ok(WarmPoolList {
            items: inventory.items,
            default_image: released.agent.uri,
            default_cpu_millis: self.default_capacity.cpu_millis,
            default_memory_mib: self.default_capacity.memory_mib,
            default_config_sha256,
            available,
            machine_image: released.machine.map(|(image, _)| image.uri),
            machine_config_sha256,
            default_claim_pool_id,
            machine_claim_pool_id,
            default_capacity_pool_id,
            machine_capacity_pool_id,
            runtime_versions,
        })
    }

    pub(super) fn inventory_view(
        &self,
        released: &ReleasedImages,
        targets: &[PoolTarget],
        live: &[NativePoolStatus],
        available: bool,
        default_plans: [Option<&str>; 2],
    ) -> InventoryView {
        let provider = self.adapter.native_pools().zip(self.home.as_ref());
        let planned = |target: &PoolTarget| {
            if target.retiring || target.retired || target.plan.spec.cold_canary {
                return Some(false);
            }
            let image = if target.machine {
                let Some((image, _)) = &released.machine else {
                    return Some(false);
                };
                if Some(target.plan.config_sha256.as_str()) != default_plans[1] {
                    return default_plans[1].map(|_| false);
                }
                image
            } else {
                &released.agent
            };
            if target.plan.spec.image.uri != image.uri {
                return Some(false);
            }
            let (provider, home) = provider?;
            current::current_plan(provider, image, home, target).ok()
        };

        let mut counts = HashMap::new();
        for binding in targets.iter().filter_map(|target| target.binding.as_ref()) {
            *counts.entry(binding.uid.as_str()).or_insert(0) += 1;
        }
        let mut claimed = [Vec::new(), Vec::new()];
        let mut seen = HashSet::new();
        let mut items = Vec::new();
        // Prefer a current record over a historical one for a shared physical
        // binding. Duplicates never advertise default claim authority.
        let mut targets: Vec<_> = targets
            .iter()
            .map(|target| (target, planned(target)))
            .collect();
        targets.sort_by_key(|(_, current)| !current.unwrap_or(false));
        for (target, current) in targets {
            let status = target
                .binding
                .as_ref()
                .and_then(|binding| live.iter().find(|status| &status.binding == binding));
            let lane = usize::from(target.machine);
            let unique = target
                .binding
                .as_ref()
                .is_some_and(|binding| counts.get(binding.uid.as_str()) == Some(&1));
            if available
                && !self.default_holds.lane(target.machine)
                && current == Some(true)
                && Some(target.plan.config_sha256.as_str()) == default_plans[lane]
            {
                let bound = status.filter(|_| unique);
                let claimable = bound.is_some_and(|status| {
                    status.binding.spec.home.is_native_vct() || status.has_ready_spare()
                });
                claimed[lane].push((target.id.as_str(), bound.is_some(), claimable));
            }
            if target
                .binding
                .as_ref()
                .is_some_and(|binding| !seen.insert(binding.uid.as_str()))
            {
                continue;
            }
            let mut view = target.view(status, available);
            view.current_runtime = current;
            items.push(view);
        }

        InventoryView {
            items,
            claims: claimed.each_ref().map(|lane| match lane.as_slice() {
                [(id, true, true)] => Some((*id).to_owned()),
                _ => None,
            }),
            capacity_pools: claimed.each_ref().map(|lane| match lane.as_slice() {
                [(id, true, _)] => Some((*id).to_owned()),
                _ => None,
            }),
        }
    }
}
