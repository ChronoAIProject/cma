use super::*;
use layer2_core::agent_ownership::candidates;
use mongodb::options::ReturnDocument;

enum Change<'a> {
    Acquire(&'a AgentId, &'a str, &'a Membership),
    Renew(&'a OwnershipToken),
    Release(&'a OwnershipToken),
}

enum AttemptError {
    Domain(OwnershipError),
    Driver(mongodb::error::Error),
}

impl From<OwnershipError> for AttemptError {
    fn from(error: OwnershipError) -> Self {
        Self::Domain(error)
    }
}

impl From<mongodb::error::Error> for AttemptError {
    fn from(error: mongodb::error::Error) -> Self {
        Self::Driver(error)
    }
}

impl MongoAgentOwnership {
    pub(super) async fn acquire_owner(
        &self,
        id: &AgentId,
        instance: &str,
    ) -> Result<Option<OwnershipLease>, OwnershipError> {
        let membership = self.membership().await?;
        // Seed once, outside the transaction. Duplicate concurrent inserts do
        // not grant authority. This row and its epoch are never TTL-deleted.
        if let Err(error) = self.owners.update_one(doc! {"_id": id.as_str()}, doc! {
            "$setOnInsert": {
                "document_schema_version": 1_i64, "protocol": i64::from(PROTOCOL_VERSION),
                "epoch": 0_i64, "instance_id": mongodb::bson::Bson::Null,
                "expires_at": mongodb::bson::DateTime::from_millis(0), "effect_revision": 0_i64,
            },
        }).upsert(true).await {
            if !is_duplicate_key(&error) {
                return Err(unavailable(error));
            }
        }
        self.change(Change::Acquire(id, instance, &membership))
            .await
    }

    pub(super) async fn renew_owner(
        &self,
        token: &OwnershipToken,
    ) -> Result<OwnershipLease, OwnershipError> {
        self.change(Change::Renew(token))
            .await?
            .ok_or(OwnershipError::Stale)
    }

    pub(super) async fn release_owner(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.change(Change::Release(token)).await.map(|_| ())
    }

    async fn change(&self, change: Change<'_>) -> Result<Option<OwnershipLease>, OwnershipError> {
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = self.client.start_session().await.map_err(unavailable)?;
            start_transaction(&mut session).await.map_err(unavailable)?;
            let result = match change {
                Change::Acquire(id, instance, membership) => {
                    self.acquire_attempt(id, instance, membership, &mut session)
                        .await
                }
                Change::Renew(token) => self.renew_attempt(token, &mut session).await,
                Change::Release(token) => self.release_attempt(token, &mut session).await,
            };
            let result = match result {
                Ok(result) => result,
                Err(AttemptError::Domain(error)) => {
                    let _ = session.abort_transaction().await;
                    return Err(error);
                }
                Err(AttemptError::Driver(error)) => {
                    let _ = session.abort_transaction().await;
                    if is_transient(&error) {
                        tokio::task::yield_now().await;
                        continue;
                    }
                    return Err(unavailable(error));
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(result),
                CommitOutcome::Retry(_) => tokio::task::yield_now().await,
                // Never expose a token whose majority commit is uncertain.
                CommitOutcome::Unknown(error) | CommitOutcome::Failed(error) => {
                    return Err(unavailable(error));
                }
            }
        }
        Err(unavailable(
            "Agent ownership transaction remained contended",
        ))
    }

    async fn acquire_attempt(
        &self,
        id: &AgentId,
        instance: &str,
        membership: &Membership,
        session: &mut ClientSession,
    ) -> Result<Option<OwnershipLease>, AttemptError> {
        let row = self
            .owners
            .find_one(doc! {"_id": id.as_str()})
            .session(&mut *session)
            .await?
            .ok_or(OwnershipError::Stale)?;
        super::super::sandbox_mongo::require_version(&row).map_err(invalid)?;
        if row.get_i64("protocol").map_err(invalid)? != i64::from(PROTOCOL_VERSION) {
            return Err(OwnershipError::IncompatibleProtocol.into());
        }
        let epoch = row.get_i64("epoch").map_err(invalid)?;
        let previous = row.get_str("instance_id").ok();
        // A live owner is authoritative even if this process has no local link.
        if self
            .owners
            .find_one(doc! {
                "_id": id.as_str(), "instance_id": {"$type": "string"},
                "$expr": {"$gt": ["$expires_at", "$$NOW"]},
            })
            .session(&mut *session)
            .await?
            .is_some()
        {
            return if previous == Some(instance) {
                Ok(Some(owner_from(&row)?))
            } else {
                Ok(None)
            };
        }
        if candidates(id, membership, previous)?
            .first()
            .is_none_or(|member| member.instance_id != instance)
        {
            return Ok(None);
        }
        // The expired row still occupies one slot on its previous member. A
        // same-member reclaim replaces it atomically, even at the load limit.
        let reclaim = previous == Some(instance);
        let reserved = self
            .members
            .update_one(
                doc! {
                    "_id": instance, "protocol": i64::from(PROTOCOL_VERSION), "draining": false,
                    "$expr": {"$and": [
                        {"$gt": ["$expires_at", "$$NOW"]},
                        {if reclaim { "$lte" } else { "$lt" }: ["$owned_agents", "$max_owned_agents"]},
                    ]},
                },
                doc! {"$inc": {"owned_agents": if reclaim { 0_i64 } else { 1_i64 }, "renewal_revision": 1_i64}},
            )
            .session(&mut *session)
            .await?;
        if reserved.matched_count != 1 {
            return Ok(None);
        }
        let next_epoch = epoch
            .checked_add(1)
            .filter(|next| *next > 0)
            .ok_or_else(|| invalid("ownership epoch exhausted"))?;
        let acquired = self
            .owners
            .find_one_and_update(
                doc! {
                    "_id": id.as_str(), "epoch": epoch,
                    "$expr": {"$lte": ["$expires_at", "$$NOW"]},
                },
                vec![doc! {"$set": {
                    "instance_id": {"$literal": instance}, "epoch": next_epoch,
                    "expires_at": {"$add": ["$$NOW", DEFAULT_LEASE_MS as i64]},
                }}],
            )
            .return_document(ReturnDocument::After)
            .session(&mut *session)
            .await?
            .ok_or(OwnershipError::Stale)?;
        if let Some(previous) = previous.filter(|_| !reclaim) {
            self.decrement(previous, session).await?;
        }
        Ok(Some(owner_from(&acquired)?))
    }

    async fn renew_attempt(
        &self,
        token: &OwnershipToken,
        session: &mut ClientSession,
    ) -> Result<Option<OwnershipLease>, AttemptError> {
        // Writing the member row conflicts with drain and prevents renewing an
        // owner from a stale snapshot of membership.
        let member = self
            .members
            .update_one(
                doc! {
                    "_id": &token.instance_id, "draining": false,
                    "protocol": i64::from(PROTOCOL_VERSION),
                    "$expr": {"$gt": ["$expires_at", "$$NOW"]},
                },
                doc! {"$inc": {"renewal_revision": 1_i64}},
            )
            .session(&mut *session)
            .await?;
        if member.matched_count != 1 {
            return Err(OwnershipError::Stale.into());
        }
        let row = self
            .owners
            .find_one_and_update(
                token_filter(token)?,
                vec![doc! {"$set": {
                    "expires_at": {"$add": ["$$NOW", DEFAULT_LEASE_MS as i64]},
                }}],
            )
            .return_document(ReturnDocument::After)
            .session(&mut *session)
            .await?
            .ok_or(OwnershipError::Stale)?;
        Ok(Some(owner_from(&row)?))
    }

    async fn release_attempt(
        &self,
        token: &OwnershipToken,
        session: &mut ClientSession,
    ) -> Result<Option<OwnershipLease>, AttemptError> {
        let mut filter = token_filter(token)?;
        // An expired exact token can release its accounting too; a successor's
        // epoch never matches. Repeated/ambiguous releases are idempotent.
        filter.remove("$expr");
        let released = self
            .owners
            .update_one(
                filter,
                doc! {"$set": {
                    "instance_id": mongodb::bson::Bson::Null,
                    "expires_at": mongodb::bson::DateTime::from_millis(0),
                }},
            )
            .session(&mut *session)
            .await?;
        if released.matched_count == 1 {
            self.decrement(&token.instance_id, session).await?;
        }
        Ok(None)
    }

    async fn decrement(
        &self,
        instance: &str,
        session: &mut ClientSession,
    ) -> Result<(), AttemptError> {
        self.members
            .update_one(
                doc! {"_id": instance, "owned_agents": {"$gt": 0_i64}},
                doc! {"$inc": {"owned_agents": -1_i64}},
            )
            .session(session)
            .await?;
        Ok(())
    }
}
