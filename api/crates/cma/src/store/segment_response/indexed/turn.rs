use super::*;

impl Reader<'_> {
    pub(super) async fn load_turn(&mut self, turn: u64) -> Result<(), SandboxStoreError> {
        if turn == 0 || turn > self.captured.through() {
            return Err(SandboxStoreError::NotFound);
        }
        let head = self.captured.active_head();
        let record = projection::evidence(
            self.store,
            head,
            "turn",
            &turn.to_string(),
            &mut self.session,
        )
        .await
        .map_err(transaction)?
        .ok_or_else(corrupt)?;
        let projection::EvidenceRecord::Turn { facts } = record else {
            return Err(corrupt());
        };
        let output = projection::evidence(
            self.store,
            head,
            "turn_output",
            &turn.to_string(),
            &mut self.session,
        )
        .await
        .map_err(transaction)?
        .ok_or_else(corrupt)?;
        let projection::EvidenceRecord::TurnOutput { index } = output else {
            return Err(corrupt());
        };
        if index.agent_id != self.metadata.agent_id
            || index.turn_sequence != turn
            || facts.turn_sequence != turn
            || index.through_sequence > self.captured.through()
            || facts
                .end_sequence
                .is_some_and(|s| s > index.through_sequence)
        {
            return Err(corrupt());
        }
        let mut sequences = vec![turn];
        if index.through_sequence < self.captured.through() {
            sequences.push(index.through_sequence + 1);
        }
        let rows = self.source_points(&sequences).await?;
        if !matches!(&rows[0].kind,AgentLogKind::TurnStarted {turn_sequence,..} if *turn_sequence == turn)
            || rows.get(1).is_some_and(|row| !matches!(&row.kind,AgentLogKind::TurnStarted {turn_sequence,..} if *turn_sequence == row.seq))
        { return Err(corrupt()); }
        self.metadata.turn = Some(facts::convert(*facts)?);
        self.metadata.index = Some(index);
        Ok(())
    }
}
