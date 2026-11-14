use super::*;

impl RuntimeActivitySourceContext {
    pub fn validate(&self) -> Result<(), SandboxStoreError> {
        if self.serving_processes != 1
            || self.replica_mode
            || self.clock_uncertainty_ms > 1_000
            || self.coverage == RuntimeActivityCoverage::Unqualified
            || [
                &self.source_id,
                &self.adapter_version,
                &self.policy_id,
                &self.producer_id,
                &self.database_id,
                &self.api_image,
                &self.source_revision,
                &self.process_incarnation,
            ]
            .iter()
            .any(|value| {
                value.trim().is_empty() || value.len() > 512 || value.chars().any(char::is_control)
            })
        {
            return Err(invalid());
        }
        Ok(())
    }
}

impl RuntimeActivitySnapshot {
    pub fn validate(&self) -> Result<(), SandboxStoreError> {
        self.source.validate()?;
        self.vector.validate()?;
        if self.capture_started_at_ms <= 0
            || self.capture_completed_at_ms < self.capture_started_at_ms
            || self.capture_completed_at_ms - self.capture_started_at_ms > 5_000
            || self.watermark.seconds == 0
            || self.storage_incarnation.is_empty()
            || self.storage_incarnation.len() > 512
            || self.desktop.process_incarnation != self.source.process_incarnation
            || self.desktop.tracking_incarnation.is_empty()
            || self.desktop.tracking_incarnation.len() > 512
        {
            return Err(invalid());
        }
        Ok(())
    }

    pub fn has_activity(&self) -> bool {
        self.desktop.opening_or_open != 0 || self.vector.has_activity()
    }
}

impl RuntimeActivityProof {
    pub fn source_context(&self) -> &RuntimeActivitySourceContext {
        &self.final_observation.source
    }

    /// The caller supplies authoritative admission time. This method never
    /// refreshes observations or replaces their original context after drift.
    pub fn validate_at(&self, at_ms: i64) -> Result<(), SandboxStoreError> {
        self.initial.validate()?;
        self.final_observation.validate()?;
        let first = &self.initial;
        let last = &self.final_observation;
        let uncertainty = i64::from(last.source.clock_uncertainty_ms);
        let start_latest = first
            .capture_completed_at_ms
            .checked_add(uncertainty)
            .ok_or_else(invalid)?;
        let end_earliest = last
            .capture_started_at_ms
            .checked_sub(uncertainty)
            .ok_or_else(invalid)?;
        let end_latest = last
            .capture_completed_at_ms
            .checked_add(uncertainty)
            .ok_or_else(invalid)?;
        if self.schema_version != 1
            || first.source != last.source
            || first.vector != last.vector
            || first.desktop != last.desktop
            || first.storage_incarnation != last.storage_incarnation
            || first.watermark > last.watermark
            || first.has_activity()
            || last.has_activity()
            || end_earliest
                .checked_sub(start_latest)
                .is_none_or(|quiet| quiet < RUNTIME_ACTIVITY_QUIET_MS)
            || at_ms < end_latest
            || at_ms
                .checked_sub(end_earliest)
                .is_none_or(|age| age > RUNTIME_ACTIVITY_MAX_AGE_MS)
        {
            return Err(invalid());
        }
        Ok(())
    }

    pub fn compare_current(
        &self,
        current: &RuntimeActivitySnapshot,
        at_ms: i64,
    ) -> Result<(), SandboxStoreError> {
        self.validate_at(at_ms)?;
        current.validate()?;
        if current.source != *self.source_context()
            || current.vector != self.final_observation.vector
            || current.desktop != self.final_observation.desktop
            || current.storage_incarnation != self.final_observation.storage_incarnation
            || current.has_activity()
            || current.watermark < self.final_observation.watermark
        {
            return Err(invalid());
        }
        Ok(())
    }
}
