//! Held archival of exact indexed-store source columns. The caller must supply
//! a complete, audited selection and authoritative eligibility under writer
//! exclusion. This module never deactivates or restores active-store rows.
use rusqlite::params;
use sha2::{Digest, Sha256};

use super::{existing, invalid, record, storage_error, Archive, GroupProof, Receipt, Version};
use crate::failure::Result;
use crate::mutation_ledger::retirement_inventory::{Context, SourceRow};

#[path = "indexed_verification.rs"]
mod verification;

impl Archive {
    pub(crate) async fn prepare_indexed(
        &self,
        proof: GroupProof,
        context: Context,
        mut read: impl FnMut(u32) -> Result<SourceRow> + Send + 'static,
    ) -> Result<Receipt> {
        self.verify()?;
        check(self.version, &proof)?;
        let context_bytes = context.encode(&self.config)?;
        let config = context.scoped_config(&self.config)?;
        let directory = self.path.clone();
        let anchor = self.anchor.clone();
        let receipt = self.storage.call(move |db| {
            let tx = db.transaction().map_err(|_| storage_error())?;
            if let Some(receipt) = existing(&tx, &proof)? {
                verification::group(&tx, &config, &proof, &receipt, Some(&context))?;
                return Ok(receipt);
            }
            super::require_reserve(&directory, anchor.as_deref())?;
            tx.execute("INSERT INTO retired_groups VALUES (?,?,?,?,1,0)",
                params![proof.key, proof.source_sha256, proof.observation_sha256, proof.records])
                .map_err(|_| storage_error())?;
            tx.execute("INSERT INTO indexed_contexts VALUES (?,?,?)",
                params![proof.key, hash(&context_bytes), context_bytes])
                .map_err(|_| storage_error())?;
            let mut source = Sha256::new();
            context.hash_into(&mut source);
            let mut bytes = context_bytes.len() as u64;
            let mut previous = None;
            let mut operations = 0;
            let mut authority = 0;
            for index in 0..proof.records {
                let row = read(index)?;
                let payload = row.encode(&config)?;
                let ordering = row.ordering();
                if previous.as_ref().is_some_and(|previous| previous >= &ordering)
                    || row.group()?.is_some_and(|group| group != proof.key) {
                    return Err(invalid());
                }
                previous = Some(ordering.clone());
                operations += u32::from(ordering.0 == 3);
                authority += u32::from(ordering.0 == 4);
                row.hash_into(&mut source);
                bytes = bytes.checked_add(payload.len() as u64).ok_or_else(invalid)?;
                tx.execute("INSERT INTO indexed_records VALUES (?,?,?,?,?,?)",
                    params![proof.key, ordering.0, ordering.1, ordering.2, hash(&payload), payload])
                    .map_err(|_| storage_error())?;
                for operation in row.operation_ids()? {
                    tx.execute("INSERT INTO retired_operations VALUES (?,?,?) ON CONFLICT DO NOTHING",
                        params![row.key().0, operation, proof.key]).map_err(|_| storage_error())?;
                    let owner: String = tx.query_row(
                        "SELECT group_key FROM retired_operations WHERE scope=? AND operation_id=?",
                        params![row.key().0, operation], |r| r.get(0)).map_err(|_| storage_error())?;
                    if owner != proof.key {
                        return Err(invalid());
                    }
                }
            }
            if operations == 0 || authority == 0 || format!("{:x}", source.finalize()) != proof.source_sha256 {
                return Err(invalid());
            }
            tx.execute("UPDATE retired_groups SET bytes=? WHERE group_key=?",
                params![i64::try_from(bytes).map_err(|_| invalid())?, proof.key])
                .map_err(|_| storage_error())?;
            let receipt = existing(&tx, &proof)?.ok_or_else(invalid)?;
            verification::group(&tx, &config, &proof, &receipt, Some(&context))?;
            tx.commit().map_err(|_| storage_error())?;
            Ok(receipt)
        }).await?;
        self.verify()?;
        Ok(receipt)
    }

    pub(crate) async fn verify_indexed(&self, proof: GroupProof) -> Result<Receipt> {
        self.verify()?;
        check(self.version, &proof)?;
        let config = self.config.clone();
        let receipt = self
            .storage
            .call(move |db| {
                let receipt = existing(db, &proof)?.ok_or_else(invalid)?;
                verification::group(db, &config, &proof, &receipt, None)?;
                Ok(receipt)
            })
            .await?;
        self.verify()?;
        Ok(receipt)
    }
}

fn check(version: Version, proof: &GroupProof) -> Result<()> {
    if version != Version::Indexed
        || !record::digest(&proof.key)
        || !record::digest(&proof.source_sha256)
        || !record::digest(&proof.observation_sha256)
        || !(1..=65_536).contains(&proof.records)
    {
        return Err(invalid());
    }
    Ok(())
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
