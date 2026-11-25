use super::*;
use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::agent_ownership::{candidates, STORE_DEADLINE_MS};
use mongodb::options::ReturnDocument;
use std::time::Duration;

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
        self.ensure_owner_row(id).await?;
        self.change(Change::Acquire(id, instance, &membership))
            .await
    }

    async fn ensure_owner_row(&self, id: &AgentId) -> Result<(), OwnershipError> {
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
        Ok(())
    }

    pub(super) async fn reconcile_owner_acquisition(
        &self,
        id: &AgentId,
        instance: &str,
    ) -> Result<Option<OwnershipToken>, OwnershipError> {
        self.ensure_owner_row(id).await?;
        // This majority write conflicts with an outstanding acquisition on
        // the same row. A read alone may precede that transaction's commit.
        let row = self
            .owners
            .find_one_and_update(
                doc! {"_id": id.as_str(), "protocol": i64::from(PROTOCOL_VERSION)},
                doc! {"$inc": {"effect_revision": 1_i64}},
            )
            .return_document(ReturnDocument::After)
            .await
            .map_err(unavailable)?
            .ok_or(OwnershipError::IncompatibleProtocol)?;
        super::super::sandbox_mongo::require_version(&row).map_err(invalid)?;
        if row.get_str("instance_id").ok() == Some(instance) {
            Ok(Some(owner_from(&row)?.token))
        } else {
            Ok(None)
        }
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
        let mut last_conflict = None;
        match tokio::time::timeout(
            Duration::from_millis(STORE_DEADLINE_MS),
            self.change_with_retry(change, &mut last_conflict),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(unavailable(match last_conflict {
                Some(error) => format!(
                    "Agent ownership transaction exceeded its deadline after a known abort: {error}"
                ),
                None => "Agent ownership transaction exceeded its deadline".into(),
            })),
        }
    }

    async fn change_with_retry(
        &self,
        change: Change<'_>,
        last_conflict: &mut Option<String>,
    ) -> Result<Option<OwnershipLease>, OwnershipError> {
        let mut delay = Duration::from_millis(5);
        loop {
            let mut session = self.client.start_session().await.map_err(unavailable)?;
            if let Err(error) = start_transaction(&mut session).await {
                let _ = session.abort_guarded().await;
                if known_aborted(&error) {
                    wait_after_abort(error, &mut delay, last_conflict).await;
                    continue;
                }
                return Err(unavailable(error));
            }
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
                    let _ = session.abort_guarded().await;
                    return Err(error);
                }
                Err(AttemptError::Driver(error)) => {
                    let _ = session.abort_guarded().await;
                    if known_aborted(&error) {
                        wait_after_abort(error, &mut delay, last_conflict).await;
                        continue;
                    }
                    return Err(unavailable(error));
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(result),
                CommitOutcome::Retry(error) => {
                    wait_after_abort(error, &mut delay, last_conflict).await;
                }
                // Never expose a token whose majority commit is uncertain.
                CommitOutcome::Unknown(error) | CommitOutcome::Failed(error) => {
                    return Err(unavailable(error));
                }
            }
        }
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
        self.open_with_read(&token.instance_id, session).await?;
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
        self.open_with_read(&token.instance_id, session).await?;
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

// Another Agent's valid effect can hold the shared member document for longer
// than five immediate attempts. Retry definite aborts within the same absolute
// store deadline, with a short delay so the competing transaction can finish.
async fn wait_after_abort(
    error: mongodb::error::Error,
    delay: &mut Duration,
    last_conflict: &mut Option<String>,
) {
    *last_conflict = Some(error.to_string());
    tokio::time::sleep(*delay).await;
    *delay = (*delay * 2).min(Duration::from_millis(100));
}
