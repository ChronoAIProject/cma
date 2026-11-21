//! A durable barrier for DDL that cannot consume the runner's lease fence.
use super::*;
use mongodb::ClientSession;

pub(super) const UNSETTLED: &str = "destructive index operation is unsettled; stop its client and reconcile server effects before reviewed forward recovery";
const BARRIER: &str = "destructive_ddl";

impl Ledger {
    pub(in crate::store::migrations) async fn claim_abort(
        &self,
        identity: &Identity,
        previous_fence: i64,
    ) -> Result<Lease> {
        self.claim_inner(identity, Some(previous_fence)).await
    }

    pub(super) async fn abort_barrier(
        &self,
        session: Option<&mut ClientSession>,
    ) -> Result<Option<Document>> {
        let find = self.rows.find_one(doc! {"_id":BARRIER}).max_time(IO);
        let row = match session {
            Some(session) => find.session(session).await,
            None => find.await,
        }
        .map_err(|_| "destructive operation barrier unavailable")?;
        if let Some(row) = &row {
            validate(row)?;
        }
        Ok(row)
    }

    pub(super) async fn register_abort(
        &self,
        lease: &Lease,
        session: &mut ClientSession,
    ) -> Result<()> {
        // The same transaction claims the runner and inserts the attempt. A
        // competing claim/finish must write that runner and cannot miss this barrier.
        self.rows
            .replace_one(
                doc! {"_id":BARRIER},
                doc! {"_id":BARRIER,"schema":1_i64,"kind":"index_abort",
                "migration":self.step.id(),"checksum":self.step.checksum(),
                "attempt":&lease.attempt,"fence":lease.fence,"state":"pending"},
            )
            .upsert(true)
            .session(session)
            .await
            .map_err(|_| "destructive operation barrier admission unavailable")?;
        Ok(())
    }

    pub(in crate::store::migrations) async fn begin_abort(&self, lease: &Lease) -> Result<()> {
        let result = self
            .rows
            .update_one(
                self.abort_filter(lease, "pending"),
                doc! {"$set":{"state":"draining"}},
            )
            .await
            .map_err(|_| UNSETTLED)?;
        if result.matched_count != 1 {
            return Err("destructive operation barrier lost before dispatch");
        }
        Ok(())
    }

    pub(super) fn abort_filter(&self, lease: &Lease, state: &str) -> Document {
        doc! {"_id":BARRIER,"schema":1_i64,"kind":"index_abort",
        "migration":self.step.id(),"checksum":self.step.checksum(),
        "attempt":&lease.attempt,"fence":lease.fence,"state":state}
    }
}

fn validate(row: &Document) -> Result<()> {
    let step = row
        .get_str("migration")
        .ok()
        .and_then(|id| Step::parse(id).ok())
        .ok_or("destructive operation migration unknown")?;
    if row.len() != 8
        || row.get_str("_id").ok() != Some(BARRIER)
        || row.get_i64("schema").ok() != Some(1)
        || row.get_str("kind").ok() != Some("index_abort")
        || row.get_str("checksum").ok() != Some(step.checksum().as_str())
        || !row
            .get_str("attempt")
            .is_ok_and(|id| id.starts_with("attempt:") && id.len() == 40)
        || !row.get_i64("fence").is_ok_and(|fence| fence > 0)
        || !matches!(
            row.get_str("state").ok(),
            Some("pending" | "draining" | "settled")
        )
    {
        return Err("destructive operation barrier checksum or shape drift");
    }
    Ok(())
}
