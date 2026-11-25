//! A newer source head reads only appended intervals, then replays the exact fold.
use super::*;

impl DayBucket {
    pub fn delta_revision(&self, state: &SandboxUsage, range: Range) -> Result<Option<u64>> {
        // This also enforces identity, baseline and monotonic source metadata.
        // An older projected tail is deliberately not reusable by itself.
        let _complete_projection = self.validate_source(state)?;
        if range.from < self.day || range.to > self.day + DAY || range.from >= range.to {
            return Err(MetricsError::Unavailable);
        }
        let Some(input) = &self.boundary else {
            return Ok(None);
        };
        if self.complete_lifecycle != state.complete_lifecycle
            || self.revision >= state.revision
            || !input.scope_valid
            || input.queried_to < self.last_observed_at.min(self.day + DAY)
        {
            return Ok(None);
        }
        Ok(Some(self.revision))
    }

    pub fn project_delta(
        &self,
        state: &SandboxUsage,
        appended: &[UsageEvent],
        range: Range,
    ) -> Result<Option<allocation::Projection>> {
        let revision = self
            .delta_revision(state, range)?
            .ok_or(MetricsError::Unavailable)?;
        if appended.windows(2).any(|p| p[0].revision >= p[1].revision)
            || appended.iter().any(|e| {
                e.sandbox_id != state.sandbox_id
                    || e.revision <= revision
                    || e.revision > state.revision
                    || e.started_at > e.ended_at
                    || e.ended_at > state.last_observed_at
            })
        {
            return Err(MetricsError::Unavailable);
        }
        if appended
            .iter()
            .any(|e| e.started_at < self.last_observed_at)
        {
            // A source outside the append-only writer contract needs the full
            // raw fold so overlap/incomplete semantics remain authoritative.
            return Ok(None);
        }
        // Supported source writers append at the old watermark. Keep the old
        // prefix in its original revision order and replay with the new tail;
        // merging old numeric totals would change f64 rounding and gap checks.
        let mut events = self
            .boundary
            .as_ref()
            .ok_or(MetricsError::Unavailable)?
            .events(state);
        events.extend_from_slice(appended);
        Ok(Some(allocation::project(state, &events, range)))
    }
}
