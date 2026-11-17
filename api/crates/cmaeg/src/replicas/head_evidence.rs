//! One head's read-only evidence; failures never authorize repair or queue advancement.
use super::{
    diagnostics::{Class, ScanError, Stage},
    partition::Partitions,
    partition_model::{item_id, partition_id, Head, Item, ITEM_KIND},
};
use crate::ingress::routing_store::{AdmittedPlan, PlanStore};

pub(super) struct Loaded {
    pub item: Item,
    pub item_version: u64,
    pub plan: AdmittedPlan,
    pub target_index: usize,
}
pub(super) async fn load(
    partitions: &Partitions,
    plans: &PlanStore,
    id: &str,
    head: &Head,
    cached: bool,
) -> Result<Option<Loaded>, ScanError> {
    let fail = |class, stage| ScanError::new(class, stage, Some(id));
    let work = head
        .queue
        .front()
        .ok_or_else(|| fail(Class::Corrupt, Stage::Front))?;
    let (item, item_version) = partitions
        .store
        .read::<Item>(&item_id(work), true)
        .await
        .map_err(|error| fail(error.into(), Stage::ItemRead))?
        .ok_or_else(|| fail(Class::MissingItem, Stage::ItemRead))?;
    if item.kind != ITEM_KIND || item.partition != id || item.work_id != *work {
        return Err(fail(Class::Mismatch, Stage::ItemCheck));
    }
    if item.done {
        if cached {
            // Completion atomically pops the item. A 100ms snapshot can precede
            // that commit; only a fresh, validated head proves this hint obsolete.
            let (current, _) = partitions
                .head(id, &head.owner)
                .await
                .map_err(|error| fail(error.into(), Stage::HeadRead))?;
            if !current.queue.contains(work) {
                return Ok(None);
            }
        }
        return Err(fail(Class::Corrupt, Stage::ItemCheck));
    }
    let plan = plans
        .load(&item.identity)
        .await
        .map_err(|error| fail(error.into(), Stage::PlanLoad))?
        .ok_or_else(|| fail(Class::MissingPlan, Stage::PlanLoad))?;
    let mismatch = || {
        fail(Class::Mismatch, Stage::PlanCheck)
            .aged((partitions.membership.now_seconds() - plan.admitted_at).max(0))
    };
    let mut targets = plan
        .targets
        .iter()
        .enumerate()
        .filter(|(_, target)| target.id == item.target);
    let (target_index, target) = targets.next().ok_or_else(mismatch)?;
    if targets.next().is_some()
        || plan.request_hash != item.request_hash
        || plan.identity.owner != head.owner
        || partition_id(&partitions.membership.deployment, &plan, target) != id
    {
        return Err(mismatch());
    }
    Ok(Some(Loaded {
        item,
        item_version,
        plan,
        target_index,
    }))
}
