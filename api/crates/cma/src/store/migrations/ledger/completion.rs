use super::*;

impl Ledger {
    pub(in crate::store::migrations) async fn finish(
        &self,
        lease: &Lease,
        outcome: Result<i64>,
    ) -> Result<()> {
        self.finish_inner(lease, outcome, None).await
    }

    // Only the abort continuation can settle: either before dispatch, or after
    // an acknowledged terminal command. Lease expiry never proves either case.
    pub(in crate::store::migrations) async fn settle_abort(
        &self,
        lease: &Lease,
        dispatched: bool,
        reason: &'static str,
    ) -> Result<()> {
        self.finish_inner(
            lease,
            Err(reason),
            Some(if dispatched { "draining" } else { "pending" }),
        )
        .await
    }

    async fn finish_inner(
        &self,
        lease: &Lease,
        outcome: Result<i64>,
        abort_state: Option<&str>,
    ) -> Result<()> {
        let mut session = self
            .client
            .start_session()
            .await
            .map_err(|_| "migration session unavailable")?;
        start_transaction(&mut session)
            .await
            .map_err(|_| "migration transaction unavailable")?;
        let barrier = self.abort_barrier(Some(&mut session)).await?;
        let mut owned = Self::owned(lease);
        if let Some(state) = abort_state {
            let result = self
                .rows
                .update_one(
                    self.abort_filter(lease, state),
                    doc! {"$set":{"state":"settled"}},
                )
                .session(&mut session)
                .await
                .map_err(|_| "destructive operation settlement unavailable")?;
            if result.matched_count != 1 {
                return Err("destructive operation settlement lost");
            }
            // The durable barrier prevents handoff even after expiry. The sole
            // dispatching continuation may finish once its effect has settled.
            owned.remove("$expr");
        } else if barrier.is_some_and(|row| row.get_str("state").ok() != Some("settled")) {
            return Err(abort::UNSETTLED);
        }
        let result = self
            .rows
            .update_one(owned, vec![doc! {"$set":{"expires_at":"$$NOW"}}])
            .session(&mut session)
            .await
            .map_err(|_| "migration completion lease unavailable")?;
        if result.matched_count != 1 {
            return Err("migration lease lost before completion");
        }
        let state = match outcome {
            Ok(_) => "completed",
            Err(_) => "failed",
        };
        let result = self
            .rows
            .update_one(
                doc! {"_id":&lease.attempt,"schema":1_i64,
                "kind":"attempt","fence":lease.fence,"outcome":"running",
                "migration":self.step.id(),"checksum":self.step.checksum()},
                doc! {"$set":{"outcome":state,"changed":outcome.ok(),"reason":outcome.err()},
                "$currentDate":{"finished_at":true}},
            )
            .session(&mut session)
            .await
            .map_err(|_| "migration receipt unavailable")?;
        if result.matched_count != 1 {
            return Err("migration attempt drift");
        }
        if outcome.is_ok() {
            let result = self
                .rows
                .update_one(
                    doc! {"_id":self.step.manifest(),"schema":1_i64,
                    "checksum":self.step.checksum(),"completed":false},
                    doc! {"$set":{"completed":true,"completion_attempt":&lease.attempt}},
                )
                .session(&mut session)
                .await
                .map_err(|_| "migration completion unavailable")?;
            if result.matched_count != 1 {
                return Err("migration completion drift");
            }
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => (),
            CommitOutcome::Unknown(_) => {
                let found = self
                    .read_at(
                        session.operation_time(),
                        doc! {"_id":&lease.attempt,
                        "schema":1_i64,"kind":"attempt","migration":self.step.id(),
                        "checksum":self.step.checksum(),"fence":lease.fence,
                        "outcome":state,"changed":outcome.ok(),"reason":outcome.err()},
                    )
                    .await?;
                if found.is_none() {
                    return Err("migration completion commit unknown; inspect status");
                }
            }
            _ => return Err("migration completion transaction failed"),
        }
        Ok(())
    }
}
