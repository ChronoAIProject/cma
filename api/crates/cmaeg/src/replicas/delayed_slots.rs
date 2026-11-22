//! A bounded per-occurrence reservation precedes payload storage. It carries
//! no send authority and never changes the shared guard or a peer's live claim.
use super::{config::STORE_TIMEOUT, partition::Partitions, store::Write, Error};
use crate::ingress::routing_store::{AdmittedPlan, StoredTarget};
use futures_util::TryStreamExt;
use mongodb::bson::{doc, Document};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const LIMIT: usize = 64;
const KIND: &str = "routed_delayed_admission_slot_v1";

pub(crate) enum Reservation {
    Ready,
    Conflict,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Slot {
    kind: String,
    owner: String,
    admission: String,
    target: String,
    work: String,
    content_hash: String,
    retained_until: i64,
}
impl Partitions {
    pub(crate) async fn reserve_delayed(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        work: &str,
        content_hash: &str,
    ) -> Result<Reservation, Error> {
        let keys: Vec<_> = (0..LIMIT)
            .map(|slot| {
                crate::connections::store::index_id(
                    KIND,
                    &[
                        &self.membership.deployment,
                        &plan.identity.owner,
                        &plan.admission_id,
                        &target.id,
                        &slot.to_string(),
                    ],
                )
            })
            .collect();
        for _ in 0..8 {
            if self.store.clock().await?.millis / 1000
                >= super::delayed_lane::retention(plan, target)
            {
                return Err(Error::Expired);
            }
            let rows: Vec<Document> = tokio::time::timeout(STORE_TIMEOUT, async {
                self.store
                    .collection(true)?
                    .find(doc! {
                        "_id":{"$in": &keys}, "$expr":{"$gt":["$expires_at","$$NOW"]}
                    })
                    .limit(LIMIT as i64)
                    .await
                    .map_err(|_| Error::Unavailable)?
                    .try_collect()
                    .await
                    .map_err(|_| Error::Unavailable)
            })
            .await
            .map_err(|_| Error::Unavailable)??;
            let mut occupied = BTreeMap::new();
            for row in rows {
                let id = row.get_str("_id").map_err(|_| Error::Corrupt)?.to_owned();
                let (slot, version) = self.store.decode::<Slot>(&id, true, row)?;
                if version != 1
                    || slot.kind != KIND
                    || slot.owner != plan.identity.owner
                    || slot.admission != plan.admission_id
                    || slot.target != target.id
                    || slot.retained_until != plan.retained_until
                    || uuid::Uuid::parse_str(&slot.work).is_err()
                    || slot.content_hash.len() != 64
                    || !slot.content_hash.bytes().all(|b| b.is_ascii_hexdigit())
                {
                    return Err(Error::Corrupt);
                }
                if slot.work == work {
                    return Ok(if slot.content_hash == content_hash {
                        Reservation::Ready
                    } else {
                        Reservation::Conflict
                    });
                }
                occupied.insert(id, slot);
            }
            // Immutable slots are never freed during admission's lifetime.
            // Every contender picks the first hole; a stale reader conflicts
            // there before it can reserve a second slot for the same work.
            let key = keys
                .iter()
                .find(|id| !occupied.contains_key(*id))
                .ok_or(Error::Capacity)?;
            let slot = Slot {
                kind: KIND.into(),
                owner: plan.identity.owner.clone(),
                admission: plan.admission_id.clone(),
                target: target.id.clone(),
                work: work.into(),
                content_hash: content_hash.into(),
                retained_until: plan.retained_until,
            };
            match self
                .store
                .write(
                    key,
                    &slot.owner,
                    &slot,
                    Write {
                        retained_until: Some(slot.retained_until),
                        ..Default::default()
                    },
                )
                .await
            {
                Ok(_) => return Ok(Reservation::Ready),
                Err(Error::Busy) => continue,
                Err(error) => return Err(error),
            }
        }
        // No executable root has been admitted, so this cannot mean HTTP 409
        // routed_reply_in_progress. The caller can reconcile the same key.
        Err(Error::Unavailable)
    }
}
