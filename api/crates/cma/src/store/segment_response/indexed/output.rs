use super::*;
use layer2_core::sandbox::store::indexed::OUTPUT_PAGE_BYTES;
use mongodb::options::Hint;

impl Reader<'_> {
    pub(super) async fn output_changes(
        &mut self,
        from: u64,
        limit: u16,
    ) -> Result<Vec<AgentOutputChange>, SandboxStoreError> {
        self.captured
            .validate_snapshot(self.store, &self.session, capture::CaptureMode::Snapshot)
            .map_err(transaction)?;
        let index = self.metadata.index.as_ref().ok_or_else(corrupt)?;
        if limit == 0 || limit > OUTPUT_PAGE_CHANGES || from > index.changes {
            return Err(SandboxStoreError::InvalidPage);
        }
        let expected = (index.changes - from).min(u64::from(limit));
        if expected == 0 {
            return Ok(vec![]);
        }
        let head = self.captured.active_head();
        let query = doc! {"agent_id":&head.agent_id,"incarnation":&head.incarnation,
        "evidence_generation":&head.evidence_generation,"kind":"output_change",
        "turn_sequence":index.turn_sequence as i64,"change_ordinal":{"$gte":from as i64,"$lt":(from+expected) as i64}};
        let mut read =
            crate::store::mongo_read_telemetry::Read::find(self.store.segment_logs.evidence.name());
        let mut cursor = self
            .store
            .segment_logs
            .evidence
            .find(query)
            .hint(Hint::Name("agent_log_output_change_seek".into()))
            .sort(doc! {"change_ordinal":1})
            .limit(expected as i64 + 1)
            // Leave room for the excess-evidence probe in the initial batch.
            .batch_size(u32::from(limit) + 1)
            .max_time(std::time::Duration::from_secs(2))
            .read_concern(ReadConcern::snapshot())
            .session(&mut self.session)
            .await
            .map_err(driver)?;
        let mut result = Vec::new();
        let mut bytes = 0_usize;
        while let Some(row) = read
            .next_in_session(&mut cursor, &mut self.session)
            .await
            .map_err(driver)?
        {
            bytes = bytes
                .checked_add(
                    mongodb::bson::serialize_to_vec(&row)
                        .map_err(|_| corrupt())?
                        .len(),
                )
                .ok_or_else(corrupt)?;
            if bytes > OUTPUT_PAGE_BYTES || result.len() as u64 >= expected {
                return Err(corrupt());
            }
            let ordinal = from + result.len() as u64;
            let key = format!("{}:{ordinal}", index.turn_sequence);
            let record = projection::read_evidence(head, "output_change", &key, row)?;
            let projection::EvidenceRecord::OutputChange { change } = record else {
                return Err(corrupt());
            };
            if change.ordinal != ordinal
                || change.turn_sequence != index.turn_sequence
                || change.source_sequence > index.through_sequence
            {
                return Err(corrupt());
            }
            result.push(change);
        }
        if result.len() as u64 != expected
            || result.windows(2).any(|p| p[0].after != p[1].before)
            || (from == 0 && result[0].before != Default::default())
            || (from + expected == index.changes
                && result.last().ok_or_else(corrupt)?.after != index.totals)
        {
            return Err(corrupt());
        }
        read.complete();
        Ok(result)
    }
}
