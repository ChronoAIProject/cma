//! Whole-transaction retries and original-proof checks before every commit.
use super::*;
use layer2_core::bridge::ingress::CallbackAcceptance;

pub(crate) struct ChannelControlAcceptance {
    pub acceptance: CallbackAcceptance,
    pub admitted: Option<AdmittedChannelControl>,
}

impl MongoChannelControlStore {
    pub(crate) async fn admit(
        &self,
        request: ChannelControlReservation,
    ) -> Result<ChannelControlAcceptance, Error> {
        for attempt in 0..8 {
            request.require_current()?;
            let mut session = self
                .invokes
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| Error::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| Error::Unavailable)?;
            let result = match self.transaction(&request, &mut session).await {
                Ok(result) => result,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    match error {
                        MutationError::Driver(error)
                            if is_transient(&error) || is_duplicate_key(&error) =>
                        {
                            retry_pause(attempt).await;
                            continue;
                        }
                        other => return Err(other.core()),
                    }
                }
            };
            // Covers new work, old dispositions and provider aliases alike.
            // A successful earlier branch cannot refresh the original proof.
            if let Err(error) = request.require_current() {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(result),
                CommitOutcome::Retry(_) => retry_pause(attempt).await,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    // Expiry while reconciling never proves non-acceptance.
                    return match self.recover_acceptance(request.proof().write()).await {
                        Ok(Some(result)) => Ok(result),
                        Err(Error::Conflict) => Err(Error::Conflict),
                        _ => Err(Error::Unavailable),
                    };
                }
            }
        }
        Err(Error::Unavailable)
    }

    /// Shared permanent receipts are resolved before fresh source eligibility.
    /// New aliases and their complete intent binding must be known committed.
    pub(crate) async fn recover_acceptance(
        &self,
        write: &ChannelControlWrite,
    ) -> Result<Option<ChannelControlAcceptance>, Error> {
        let recovery = ChannelControlRecovery::from_authenticated(write.inbox())?;
        self.recover_authenticated_acceptance(&recovery).await
    }

    /// Recovery is independent of current reply/group admission. This input
    /// can only return a complete prior receipt and retain its exact alias.
    pub(crate) async fn recover_authenticated_acceptance(
        &self,
        write: &ChannelControlRecovery<'_>,
    ) -> Result<Option<ChannelControlAcceptance>, Error> {
        self.resolve_callback(write, false).await
    }

    /// Current disabled/partition refusal and any raced prior v2 recovery must
    /// share one transaction, including the complete canonical intent graph.
    pub(crate) async fn refuse_fresh_or_recover(
        &self,
        write: &ChannelControlRecovery<'_>,
    ) -> Result<ChannelControlAcceptance, Error> {
        self.resolve_callback(write, true)
            .await?
            .ok_or(Error::Unavailable)
    }

    async fn resolve_callback(
        &self,
        write: &ChannelControlRecovery<'_>,
        allow_fresh_refusal: bool,
    ) -> Result<Option<ChannelControlAcceptance>, Error> {
        for attempt in 0..8 {
            write.require_current()?;
            let mut session = self
                .invokes
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| Error::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| Error::Unavailable)?;
            let result = match self
                .recover_transaction(write, &mut session, allow_fresh_refusal)
                .await
            {
                Ok(result) => result,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    match error {
                        MutationError::Driver(error)
                            if is_transient(&error) || is_duplicate_key(&error) =>
                        {
                            retry_pause(attempt).await;
                            continue;
                        }
                        other => return Err(other.core()),
                    }
                }
            };
            if let Err(error) = write.require_current() {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(result),
                CommitOutcome::Retry(_) => retry_pause(attempt).await,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(Error::Unavailable)
                }
            }
        }
        Err(Error::Unavailable)
    }
}

async fn retry_pause(attempt: u32) {
    tokio::time::sleep(std::time::Duration::from_millis(10_u64 << attempt.min(5))).await;
}
