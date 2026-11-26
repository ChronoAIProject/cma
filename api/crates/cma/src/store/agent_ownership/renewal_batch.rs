use super::*;
use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::agent_ownership::{RenewalBatch, MAX_RENEWAL_BATCH, STORE_DEADLINE_MS};
use mongodb::bson::Bson;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

struct Chunk<'a> {
    instance: &'a str,
    tokens: &'a [OwnershipToken],
    owners: Document,
}

impl<'a> Chunk<'a> {
    fn new(tokens: &'a [OwnershipToken]) -> Result<Self, OwnershipError> {
        let first = tokens
            .first()
            .ok_or_else(|| invalid("empty renewal batch"))?;
        if tokens.len() > MAX_RENEWAL_BATCH || first.instance_id.is_empty() {
            return Err(invalid("invalid renewal batch size or member"));
        }
        let mut ids = BTreeSet::new();
        let mut predicates = Vec::with_capacity(tokens.len());
        for token in tokens {
            if token.instance_id != first.instance_id || !ids.insert(&token.agent_id) {
                return Err(invalid(
                    "batch renewal requires distinct Agents for one member",
                ));
            }
            predicates.push(token_filter(token)?);
        }
        Ok(Self {
            instance: &first.instance_id,
            tokens,
            owners: doc! {"$or": predicates},
        })
    }
}

enum Attempt {
    Updated(Vec<OwnershipLease>),
    PartialMatch,
}

enum AttemptError {
    Domain(OwnershipError),
    Driver(mongodb::error::Error),
}

impl From<mongodb::error::Error> for AttemptError {
    fn from(error: mongodb::error::Error) -> Self {
        Self::Driver(error)
    }
}

impl MongoAgentOwnership {
    pub(super) async fn renew_owner_batch(
        &self,
        tokens: &[OwnershipToken],
    ) -> Result<RenewalBatch, OwnershipError> {
        let chunk = Chunk::new(tokens)?;
        tokio::time::timeout(
            Duration::from_millis(STORE_DEADLINE_MS),
            self.renew_batch_with_retry(&chunk),
        )
        .await
        .map_err(|_| unavailable("Agent ownership batch exceeded its deadline"))?
    }

    async fn renew_batch_with_retry(
        &self,
        chunk: &Chunk<'_>,
    ) -> Result<RenewalBatch, OwnershipError> {
        let mut delay = Duration::from_millis(5);
        loop {
            let mut session = self.client.start_session().await.map_err(unavailable)?;
            let attempt = match start_transaction(&mut session).await {
                Ok(()) => self.renew_batch_attempt(chunk, &mut session).await,
                Err(error) => Err(AttemptError::Driver(error)),
            };
            let aborted = match attempt {
                Ok(Attempt::PartialMatch) => {
                    confirm_partial_abort(&mut session).await?;
                    return Ok(RenewalBatch::PartialMatchAborted);
                }
                Ok(Attempt::Updated(leases)) => match commit_transaction(&mut session).await {
                    CommitOutcome::Committed => return Ok(RenewalBatch::Committed(leases)),
                    CommitOutcome::Retry(error) => error,
                    CommitOutcome::Unknown(error) | CommitOutcome::Failed(error) => {
                        return Err(unavailable(error));
                    }
                },
                Err(AttemptError::Domain(error)) => {
                    close_transaction(&mut session).await;
                    return Err(error);
                }
                Err(AttemptError::Driver(error)) => {
                    close_transaction(&mut session).await;
                    if !known_aborted(&error) {
                        return Err(unavailable(error));
                    }
                    error
                }
            };
            let conflict = is_write_conflict(&aborted);
            metrics::counter!("cma_agent_ownership_batch_retry_total", "write_conflict" => conflict.to_string()).increment(1);
            tracing::debug!(
                write_conflict = conflict,
                "Agent ownership renewal batch aborted and will retry"
            );
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(Duration::from_millis(50));
        }
    }

    async fn renew_batch_attempt(
        &self,
        chunk: &Chunk<'_>,
        session: &mut ClientSession,
    ) -> Result<Attempt, AttemptError> {
        self.open_with_read(chunk.instance, session).await?;
        let member = self.members.update_one(
            doc! {"_id": chunk.instance, "draining": false, "protocol": i64::from(PROTOCOL_VERSION),
                "$expr": {"$gt": ["$expires_at", "$$NOW"]}},
            doc! {"$inc": {"renewal_revision": 1_i64}},
        ).session(&mut *session).await?;
        if member.matched_count != 1 {
            return Err(AttemptError::Domain(OwnershipError::Stale));
        }
        let updated = self
            .owners
            .update_many(
                chunk.owners.clone(),
                vec![doc! {"$set": {"expires_at": {"$add": ["$$NOW", DEFAULT_LEASE_MS as i64]}}}],
            )
            .session(&mut *session)
            .await?;
        if updated.matched_count != chunk.tokens.len() as u64 {
            return Ok(Attempt::PartialMatch);
        }
        let mut cursor = self
            .owners
            .find(chunk.owners.clone())
            .session(&mut *session)
            .await?;
        let mut leases = BTreeMap::new();
        while let Some(row) = cursor.next(&mut *session).await.transpose()? {
            let lease = owner_from(&row).map_err(AttemptError::Domain)?;
            if leases.insert(lease.token.agent_id.clone(), lease).is_some() {
                return Err(AttemptError::Domain(invalid(
                    "duplicate batch renewal result",
                )));
            }
        }
        let mut ordered = Vec::with_capacity(chunk.tokens.len());
        for token in chunk.tokens {
            let lease = leases
                .remove(&token.agent_id)
                .filter(|lease| lease.token == *token)
                .ok_or_else(|| {
                    AttemptError::Domain(invalid("missing exact batch renewal result"))
                })?;
            ordered.push(lease);
        }
        if !leases.is_empty() {
            return Err(AttemptError::Domain(invalid(
                "unexpected batch renewal result",
            )));
        }
        Ok(Attempt::Updated(ordered))
    }
}

async fn confirm_partial_abort(session: &mut ClientSession) -> Result<(), OwnershipError> {
    // Driver abort hides command failures. Fallback must follow a reply from
    // the same session and transaction, before its driver state is closed.
    let reply = session
        .client()
        .database("admin")
        .run_command(doc! {"abortTransaction": 1_i32})
        .session(&mut *session)
        .await;
    close_transaction(session).await;
    let reply = reply.map_err(unavailable)?;
    let succeeded = matches!(reply.get("ok"), Some(Bson::Int32(1) | Bson::Int64(1)))
        || matches!(reply.get("ok"), Some(Bson::Double(value)) if *value == 1.0);
    if succeeded && !reply.contains_key("writeConcernError") && !reply.contains_key("errorLabels") {
        Ok(())
    } else {
        Err(unavailable("partial renewal abort was not acknowledged"))
    }
}

async fn close_transaction(session: &mut ClientSession) {
    if let Err(error) = session.abort_guarded().await {
        tracing::debug!(%error, "Agent ownership batch abort could not close the session");
    }
}

fn is_write_conflict(error: &mongodb::error::Error) -> bool {
    use mongodb::error::{ErrorKind, WriteFailure};
    match error.kind.as_ref() {
        ErrorKind::Command(error) => error.code == 112,
        ErrorKind::Write(WriteFailure::WriteError(error)) => error.code == 112,
        _ => false,
    }
}
