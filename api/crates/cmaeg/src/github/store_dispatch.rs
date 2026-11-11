use super::*;
use crate::github::Delivery;

impl Store {
    pub async fn create_delivery(&self, owner: &str, delivery: &Delivery) -> Result<(), Error> {
        let row = self.row(&delivery.receipt.receipt_id, owner, "delivery", 1, delivery)?;
        self.records
            .insert_one(row)
            .await
            .map_err(|error| match error.kind.as_ref() {
                mongodb::error::ErrorKind::Write(mongodb::error::WriteFailure::WriteError(
                    write,
                )) if write.code == 11000 => Error::Conflict,
                _ => Error::Unavailable,
            })?;
        Ok(())
    }

    pub async fn claim_delivery(
        &self,
        owner: &str,
        id: &str,
        now: i64,
    ) -> Result<Option<(i64, Delivery)>, Error> {
        let row = self
            .records
            .find_one_and_update(
                doc! {
                    "_id": id,
                    "owner": owner,
                    "kind": "delivery",
                    "$or": [
                        {"lease_until": {"$exists": false}},
                        {"lease_until": {"$lte": now}},
                    ],
                },
                doc! {
                    "$set": {"lease_until": now.saturating_add(30)},
                    "$inc": {"guard_revision": 1_i64},
                },
            )
            .return_document(ReturnDocument::After)
            .await
            .map_err(|_| Error::Unavailable)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let guard = row.guard_revision;
        let (_, _, value) = self.open_row(row)?;
        Ok(Some((guard, value)))
    }

    pub async fn update_delivery(
        &self,
        owner: &str,
        id: &str,
        guard: i64,
        delivery: &Delivery,
    ) -> Result<i64, Error> {
        let row = self.row(id, owner, "delivery", 1, delivery)?;
        let changed = self
            .records
            .update_one(
                doc! {"_id":id,"owner":owner,"kind":"delivery","guard_revision":guard},
                doc! {
                    "$set": {
                        "blob": row.blob,
                        "expires_at": row.expires_at,
                        "next_dispatch_at": row.next_dispatch_at,
                    },
                    "$inc": {"guard_revision": 1_i64},
                },
            )
            .await
            .map_err(|_| Error::Unavailable)?;
        if changed.matched_count != 1 {
            return Err(Error::Retry);
        }
        Ok(guard.saturating_add(1))
    }

    pub async fn release_delivery(&self, owner: &str, id: &str, guard: i64) -> Result<(), Error> {
        let changed = self
            .records
            .update_one(
                doc! {"_id":id,"owner":owner,"kind":"delivery","guard_revision":guard},
                doc! {
                    "$unset": {"lease_until": ""},
                    "$inc": {"guard_revision": 1_i64},
                },
            )
            .await
            .map_err(|_| Error::Unavailable)?;
        if changed.matched_count != 1 {
            return Err(Error::Retry);
        }
        Ok(())
    }

    pub async fn list_pending(
        &self,
        after: Option<(&str, &str)>,
        now: i64,
        limit: i64,
    ) -> Result<Vec<(String, String, Delivery)>, Error> {
        let mut filter = doc! {
            "kind": "delivery",
            "next_dispatch_at": {"$lte": now},
        };
        if let Some((owner, id)) = after {
            filter.insert(
                "$or",
                mongodb::bson::bson!([
                    {"owner": {"$gt": owner}},
                    {"owner": owner, "_id": {"$gt": id}},
                ]),
            );
        }
        let mut cursor = self
            .records
            .find(filter)
            .sort(doc! {"owner": 1, "_id": 1})
            .limit(limit)
            .await
            .map_err(|_| Error::Unavailable)?;
        let mut values = Vec::new();
        while let Some(row) = cursor.try_next().await.map_err(|_| Error::Unavailable)? {
            let owner = row.owner.clone();
            let id = row.id.clone();
            let (_, _, delivery) = self.open_row(row)?;
            values.push((owner, id, delivery));
        }
        Ok(values)
    }
}

#[cfg(test)]
mod tests {
    use super::receipt_retention_limit;
    #[test]
    fn pending_receipts_cover_late_admission_execution() {
        assert_eq!(receipt_retention_limit(Some("ignored")), 7 * 86400);
        assert_eq!(receipt_retention_limit(Some("pending")), 14 * 86400);
        assert_eq!(receipt_retention_limit(Some("accepted")), 14 * 86400);
    }
}
