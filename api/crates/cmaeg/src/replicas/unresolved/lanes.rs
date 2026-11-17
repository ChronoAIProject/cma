use super::*;
use crate::records::mongo::ReplicaKind;
impl Operator {
    pub(super) async fn lane_views(&self) -> Result<Vec<LaneView>, Error> {
        let mut views = Vec::new();
        let mut cursor = None;
        let now = self.partitions.store.clock().await?.millis / 1000;
        loop {
            let rows = self
                .partitions
                .store
                .active_in::<Head>(
                    ReplicaKind::Partition,
                    &self.partitions.membership.deployment,
                    cursor.as_deref(),
                    128,
                    true,
                )
                .await?;
            let count = rows.len();
            for (id, head, _) in rows {
                cursor = Some(id.clone());
                head.validate(&id, &self.partitions.membership.deployment, &head.owner)?;
                let Some(work) = head.queue.front() else {
                    return Err(Error::Corrupt);
                };
                let (item, _) = self
                    .partitions
                    .store
                    .read::<Item>(&super::super::partition_model::item_id(work), true)
                    .await?
                    .ok_or(Error::Expired)?;
                let plan = self
                    .plans
                    .load(&item.identity)
                    .await
                    .map_err(admission_error)?
                    .ok_or(Error::Expired)?;
                let target = plan
                    .targets
                    .iter()
                    .find(|t| t.id == item.target)
                    .ok_or(Error::Corrupt)?;
                if super::super::delayed_lane::id(
                    &self.partitions.membership.deployment,
                    &plan,
                    target,
                ) != id
                    || item.partition != id
                {
                    return Err(Error::Corrupt);
                }
                let (admitted, _) = self
                    .partitions
                    .store
                    .read::<super::super::delayed_lane::AdmissionTime>(
                        &super::super::delayed_lane::time_id(work),
                        true,
                    )
                    .await?
                    .ok_or(Error::Corrupt)?;
                if admitted.kind != "delayed_lane_admission_v1"
                    || admitted.target != target.id
                    || admitted.work != *work
                {
                    return Err(Error::Corrupt);
                }
                views.push(LaneView {
                    id,
                    target: item.target,
                    depth: head.queue.len(),
                    oldest_age_seconds: (now - admitted.admitted_at).max(0),
                    lease_until_ms: head.authority.map(|a| a.until_ms),
                    retained_until: utc(item.retained_until)?,
                });
                if views.len() > 4096 {
                    return Err(Error::Capacity);
                }
            }
            if count < 128 {
                return Ok(views);
            }
        }
    }
}
