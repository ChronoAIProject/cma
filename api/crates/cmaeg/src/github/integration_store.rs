use super::*;
use crate::transaction_terminal::GuardedTerminal as _;

pub(in crate::github) struct RecordUpdate {
    id: String,
    owner: String,
    kind: &'static str,
    version: u64,
    value: serde_json::Value,
}
impl RecordUpdate {
    pub fn new(
        id: &str,
        owner: &str,
        kind: &'static str,
        version: u64,
        value: &impl Serialize,
    ) -> Result<Self, Error> {
        Ok(Self {
            id: id.into(),
            owner: owner.into(),
            kind,
            version,
            value: serde_json::to_value(value).map_err(|_| Error::Unavailable)?,
        })
    }
}

impl Store {
    pub async fn update_integration(
        &self,
        owner: &str,
        expected_version: u64,
        integration: &crate::github::AppIntegration,
        records: Vec<NewRecord>,
        updates: Vec<RecordUpdate>,
    ) -> Result<(), Error> {
        let expected = i64::try_from(expected_version).map_err(|_| Error::ManagementConflict)?;
        let next = expected.checked_add(1).ok_or(Error::ManagementConflict)?;
        let row = self.row(
            &integration.app_ref,
            owner,
            "app_integration",
            next,
            integration,
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
        let result = self.records.replace_one(doc! {"_id":&integration.app_ref,"owner":owner,"kind":"app_integration","version":expected}, row).session(&mut session).await.map_err(transaction_error)?;
        if result.matched_count != 1 {
            return Err(Error::Retry);
        }
        for update in updates {
            let expected = i64::try_from(update.version).map_err(|_| Error::Unavailable)?;
            let row = self.row(
                &update.id,
                owner,
                update.kind,
                expected.checked_add(1).ok_or(Error::Unavailable)?,
                &update.value,
            )?;
            let changed = self.records.replace_one(doc! {"_id":&update.id,"owner":&update.owner,"kind":update.kind,"version":expected},row)
                .session(&mut session).await.map_err(transaction_error)?;
            if changed.matched_count != 1 {
                return Err(Error::Retry);
            }
        }
        self.records
            .insert_many(rows)
            .session(&mut session)
            .await
            .map_err(transaction_error)?;
        session.commit_guarded().await.map_err(transaction_error)?;
        Ok(())
    }
}
