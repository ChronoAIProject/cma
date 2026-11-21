//! A shared parent must stay available until allocation-owned setup settles.
//! All callers perform their transition write on the same root that reuse
//! admission/startup fences, so an absent sidecar read cannot race its insertion.
use super::{durability::majority_collection, MongoSandboxStore};
use layer2_core::sandbox::{Sandbox, SandboxStatus};
use mongodb::{
    bson::{doc, Document},
    ClientSession,
};

impl MongoSandboxStore {
    pub(in crate::store) async fn legacy_reuse_blocks_quiescence(
        &self,
        current: &Sandbox,
        next: &Sandbox,
        session: &mut ClientSession,
    ) -> Result<bool, mongodb::error::Error> {
        if current.status != SandboxStatus::Active
            || !matches!(
                next.status,
                SandboxStatus::Pausing | SandboxStatus::Rotating
            )
        {
            return Ok(false);
        }
        let rows = majority_collection::<Document>(
            &self.client.database(&self.sandboxes.namespace().db),
            "bridge_allocations",
        );
        // An allocation merely reserved before setup may wait on a paused
        // parent and request ordinary revival. Once setup starts, even an
        // interrupted/retired allocation may hold effects needing reconciliation.
        Ok(rows
            .find_one(doc! {
                "record.placement.target.sandbox_id":current.id.as_str(),
                "channel_reuse":{"$exists":true},
                "channel_reuse.phase":{"$ne":"ready"},
            })
            .projection(doc! {"_id":1})
            .session(session)
            .await?
            .is_some())
    }
}
