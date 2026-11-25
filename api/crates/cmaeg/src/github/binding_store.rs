use super::*;
use crate::transaction_terminal::GuardedTerminal as _;

impl Store {
    pub async fn update_binding(
        &self,
        owner: &str,
        expected_version: u64,
        binding: &crate::github::Binding,
        integration: &crate::github::AppIntegration,
        records: Vec<NewRecord>,
    ) -> Result<(), Error> {
        let expected = i64::try_from(expected_version).map_err(|_| Error::ManagementConflict)?;
        let row = self.row(
            &binding.binding_id,
            owner,
            "binding",
            expected.checked_add(1).ok_or(Error::Unavailable)?,
            binding,
        )?;
        let rows = records
            .iter()
            .map(|record| self.row(&record.id, owner, record.kind, 1, &record.value))
            .collect::<Result<Vec<_>, _>>()?;
        let mut session = self
            .database
            .client()
            .start_session()
            .await
            .map_err(transaction_error)?;
        crate::transaction_start::start_write(&self.database, &mut session, None)
            .await
            .map_err(transaction_error)?;
        let updated = self
            .records
            .update_one(
                doc! {"_id":&binding.binding_id,"owner":owner,"kind":"binding","version":expected},
                guarded_update(row),
            )
            .session(&mut session)
            .await
            .map_err(transaction_error)?;
        if updated.matched_count != 1 {
            return Err(Error::Retry);
        }
        self.records
            .insert_many(rows)
            .session(&mut session)
            .await
            .map_err(transaction_error)?;
        self.guard_integration(owner, integration, &mut session)
            .await?;
        session.commit_guarded().await.map_err(transaction_error)?;
        Ok(())
    }

    pub(super) async fn guard_integration(
        &self,
        owner: &str,
        integration: &crate::github::AppIntegration,
        session: &mut mongodb::ClientSession,
    ) -> Result<(), Error> {
        let version = i64::try_from(integration.version).map_err(|_| Error::Unavailable)?;
        // An actual write makes retirement/rotation conflict with this transaction.
        // A snapshot read alone would permit both transactions to commit.
        let guarded = self.records.update_one(doc! {"_id":&integration.app_ref,"owner":owner,"kind":"app_integration","version":version}, doc! {"$inc":{"guard_revision":1_i64}})
            .session(session).await.map_err(transaction_error)?;
        if guarded.matched_count != 1 {
            return Err(Error::Retry);
        }
        Ok(())
    }
}
