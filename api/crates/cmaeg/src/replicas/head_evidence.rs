//! One head's read-only evidence; failures never authorize repair or queue advancement.
use super::{
    delayed_lane,
    diagnostics::{Class, ScanError, Stage},
    partition::Partitions,
    partition_model::{item_id, partition_id, Head, Item, ITEM_KIND},
};
use crate::ingress::routing_store::{AdmittedPlan, PlanStore, StoredTarget};

fn matches_id(deployment: &str, plan: &AdmittedPlan, target: &StoredTarget, id: &str) -> bool {
    id == partition_id(deployment, plan, target) || id == delayed_lane::id(deployment, plan, target)
}

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
    let lane_id = delayed_lane::id(&partitions.membership.deployment, &plan, target);
    if targets.next().is_some()
        || plan.request_hash != item.request_hash
        || plan.identity.owner != head.owner
        || !matches_id(&partitions.membership.deployment, &plan, target, id)
    {
        return Err(mismatch());
    }
    if id == lane_id
        && (!item.effect
            || item.work_id == target.id
            || partitions
                .validate_effect_item(&plan, target, &item.work_id, &item)
                .map_or(true, |retention| {
                    retention != Some(delayed_lane::retention(&plan, target))
                }))
    {
        return Err(fail(Class::Mismatch, Stage::ItemCheck));
    }
    Ok(Some(Loaded {
        item,
        item_version,
        plan,
        target_index,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn head_id_requires_exact_plan_and_target_inputs() {
        let (_server, plan) = crate::replicas::partition_tests::plan().await;
        let target = &plan.targets[0];
        for id in [
            partition_id("deployment", &plan, target),
            delayed_lane::id("deployment", &plan, target),
        ] {
            assert!(matches_id("deployment", &plan, target, &id));
            assert!(!matches_id(
                "deployment",
                &plan,
                target,
                &format!("{id}extra")
            ));
            assert!(!matches_id("other-deployment", &plan, target, &id));
        }
        for field in ["admission", "owner", "target"] {
            let mut other = plan.clone();
            match field {
                "admission" => other.admission_id = uuid::Uuid::new_v4().to_string(),
                "owner" => other.identity.owner.push_str("-other"),
                _ => other.targets[0].id = uuid::Uuid::new_v4().to_string(),
            }
            let wrong = delayed_lane::id("deployment", &other, &other.targets[0]);
            assert!(!matches_id("deployment", &plan, target, &wrong), "{field}");
        }
        let mut other = target.clone();
        other.launch.event.partition_key.push_str("/other");
        assert!(!matches_id(
            "deployment",
            &plan,
            target,
            &partition_id("deployment", &plan, &other)
        ));
    }
}
