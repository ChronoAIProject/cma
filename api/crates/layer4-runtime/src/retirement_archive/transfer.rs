//! A durable hold spans source transitions. No source callback can run before
//! the retained bytes and permanent replay-refusal indexes have been verified.
use rusqlite::params;

use super::{
    existing, invalid, storage_error, verification, Archive, GroupProof, Receipt, SourceRecord,
};
use crate::failure::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Direction {
    Deactivate,
    RestoreHeld,
}

impl Archive {
    /// The caller owns exclusion of source writers and verifies the complete
    /// source selection. The archive's writer lock serializes compatible
    /// maintenance/runtime processes; it cannot exclude an older raw writer.
    ///
    /// Any in-process writer guard must be owned inside `move_source`: the
    /// storage job may outlive cancellation of its awaiting caller.
    /// `move_source` must make an exact, durable, idempotent source transition.
    /// A returned error leaves the group pending and refuses serving startup.
    /// Restore retains that hold and never removes permanent replay refusal.
    pub(crate) async fn transfer_group(
        &self,
        proof: GroupProof,
        direction: Direction,
        mut move_source: impl FnMut(&SourceRecord) -> Result<()> + Send + 'static,
    ) -> Result<Receipt> {
        self.verify()?;
        super::require_reserve(&self.path, self.anchor.as_deref())?;
        if !super::record::digest(&proof.key)
            || !super::record::digest(&proof.source_sha256)
            || !super::record::digest(&proof.observation_sha256)
            || proof.records == 0
            || proof.records > 65_536
        {
            return Err(invalid());
        }
        let config = self.config.clone();
        let result = self.storage.call(move |db| {
            let receipt = existing(db, &proof)?.ok_or_else(invalid)?;
            verification::group(db, &config, &proof, &receipt)?;
            // This commit must precede even the first source transition. A
            // crash during restore can never leave a completed serving marker.
            let tx = db.transaction().map_err(|_| storage_error())?;
            if tx.execute("UPDATE retired_groups SET complete=0 WHERE group_key=? AND source_sha256=?",
                params![proof.key, proof.source_sha256]).map_err(|_| storage_error())? != 1 {
                return Err(invalid());
            }
            tx.commit().map_err(|_| storage_error())?;
            verification::visit(db, &config, &proof, &receipt, |record| move_source(record))?;
            if direction == Direction::Deactivate {
                let tx = db.transaction().map_err(|_| storage_error())?;
                if tx.execute("UPDATE retired_groups SET complete=1 WHERE group_key=? AND source_sha256=?",
                    params![proof.key, proof.source_sha256]).map_err(|_| storage_error())? != 1 {
                    return Err(invalid());
                }
                tx.commit().map_err(|_| storage_error())?;
            }
            existing(db, &proof)?.ok_or_else(invalid)
        }).await?;
        self.verify()?;
        Ok(result)
    }
}
