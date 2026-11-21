use super::*;

impl ActivityStore {
    /// The caller must revalidate channel ownership before checking this cursor.
    /// A missing cursor requests a full snapshot, including broadcast resets.
    pub async fn latest_if_changed(
        &self,
        owner: &str,
        record: &str,
        cursor: Option<&str>,
    ) -> Result<Option<ActivityPage>, RecordError> {
        if let Some(cursor) = cursor {
            self.cursor_reset(record, cursor)?;
            let parsed: Cursor =
                serde_json::from_slice(&URL_SAFE_NO_PAD.decode(cursor).map_err(invalid)?)
                    .map_err(invalid)?;
            if parsed
                .versions
                .iter()
                .any(|version| *version > i64::MAX as u64)
            {
                return Err(malformed());
            }
            let day = today();
            if parsed.day == day {
                let ids: Vec<_> = (0..RETENTION_DAYS)
                    .map(|offset| identifier(owner, record, day - offset))
                    .collect();
                if self
                    .records
                    .clone()
                    .interactions()
                    .local_versions(owner, &ids)
                    .await?
                    .is_some_and(|versions| versions == parsed.versions)
                    && today() == day
                {
                    return Ok(None);
                }
            }
        }
        let page = self.latest(owner, record).await?;
        Ok((cursor != Some(page.cursor.as_str())).then_some(page))
    }
}
