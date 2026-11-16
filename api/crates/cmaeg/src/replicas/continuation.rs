//! A delayed reply continues its parent without releasing unrelated successors.
use super::{
    partition::Partitions,
    partition_model::{item_id, Head, Item},
    Error,
};

impl Partitions {
    /// Select the oldest continuation of the front parent, if present. Claim
    /// performs the same selection against a fresh head and takes a new fence.
    /// The parent remains queued, so no unrelated event can pass an unknown.
    pub(super) async fn next_work(&self, head: &Head) -> Result<String, Error> {
        let front = head.queue.front().ok_or(Error::Corrupt)?;
        let parent = self.queue_item(head, front).await?;
        if parent.effect || parent.done {
            return Ok(front.clone());
        }
        for work in head.queue.iter().skip(1) {
            let item = self.queue_item(head, work).await?;
            if item.done {
                continue;
            }
            if item.effect && item.target == parent.target {
                if item.identity != parent.identity || item.request_hash != parent.request_hash {
                    return Err(Error::Corrupt);
                }
                // Media continuations retain their existing ordering. Only a
                // durable delayed-reply root authorizes this scheduling hint.
                if let Some((value, _)) = self.store.read::<serde_json::Value>(work, true).await? {
                    if crate::turns::routed::delayed_reply_continuation(&value, &item.target, work)?
                    {
                        return Ok(work.clone());
                    }
                }
            }
        }
        Ok(front.clone())
    }

    async fn queue_item(&self, head: &Head, work: &str) -> Result<Item, Error> {
        let (item, _) = self
            .store
            .read::<Item>(&item_id(work), true)
            .await?
            .ok_or(Error::Expired)?;
        if item.partition != head.id || item.work_id != work || item.identity.owner != head.owner {
            return Err(Error::Corrupt);
        }
        Ok(item)
    }
}
