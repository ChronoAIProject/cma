use super::*;

impl ProviderFundingAdapter {
    pub(super) async fn emit_evidence(
        &self,
        state: &mut SandboxBillingState,
        now: u64,
    ) -> Result<(), FundingError> {
        while let Some(first) = state.windows.first() {
            if first.window.covered_until > now {
                break;
            }
            if first.evidence.is_none() {
                let after = state.evidence_cursor.max(state.evidence_scan_cursor);
                let events = self
                    .state
                    .events(&SandboxId(state.sandbox_id.clone()), after)
                    .await?;
                let window = first.window.clone();
                // Skip only pre-enrollment history, never an acknowledged consumer range.
                // Bounded pages advance durably so an older Sandbox cannot starve here.
                if state.evidence_cursor == 0 {
                    let mut cursor = after;
                    for event in events
                        .iter()
                        .take_while(|e| e.ended_at < window.covered_from)
                    {
                        if cursor.checked_add(1) != Some(event.revision)
                            || event.sandbox_id.as_str() != window.sandbox_id
                            || event.owner.as_str() != window.owner_id
                        {
                            return Err(FundingError::Invalid(
                                "pre-enrollment ledger identity or revision gap".into(),
                            ));
                        }
                        cursor = event.revision;
                    }
                    if cursor != after {
                        state.evidence_scan_cursor = cursor;
                        self.state.save(state).await?;
                    }
                }
                let after = state.evidence_cursor.max(state.evidence_scan_cursor);
                let events: Vec<_> = events.into_iter().filter(|e| e.revision > after).collect();
                let Some(intervals) = intervals(&events, after, &window, now)? else {
                    break;
                };

                let payload = EvidenceRequest {
                    window_id: window.window_id.clone(),
                    owner_id: window.owner_id.clone(),
                    sandbox_id: window.sandbox_id.clone(),
                    runtime_generation: window.runtime_generation,
                    intervals,
                };
                state.windows[0].evidence = Some(payload);
                // Freeze before send: even additional same-second ledger events cannot
                // change a payload after NyxID may have accepted it.
                self.state.save(state).await?;
            }
            let first = &state.windows[0];
            let payload = first.evidence.clone().expect("frozen evidence");
            self.provider.settle(payload.clone()).await?;
            // Retain an observation straddling the next window for exact replay there.
            state.evidence_cursor = payload
                .intervals
                .iter()
                .rev()
                .find(|i| i.ended_at <= first.window.covered_until)
                .map_or(state.evidence_cursor, |i| i.revision);
            state.windows.remove(0);
            self.state.save(state).await?;
        }
        Ok(())
    }
}

pub(super) fn intervals(
    events: &[UsageEvent],
    after: u64,
    window: &FundingWindow,
    now: u64,
) -> Result<Option<Vec<EvidenceInterval>>, FundingError> {
    let mut expected = after
        .checked_add(1)
        .ok_or_else(|| FundingError::Invalid("usage revision overflow".into()))?;
    let mut until = window.covered_from;
    let mut previous_end = None;
    let mut selected = Vec::new();
    for event in events {
        if event.revision != expected
            || event.sandbox_id.as_str() != window.sandbox_id
            || event.owner.as_str() != window.owner_id
        {
            return Err(FundingError::Invalid(
                "retained usage revision or owner differs".into(),
            ));
        }
        expected = expected
            .checked_add(1)
            .ok_or_else(|| FundingError::Invalid("usage revision overflow".into()))?;
        let elapsed = event
            .ended_at
            .checked_sub(event.started_at)
            .ok_or_else(|| FundingError::Invalid("negative retained interval".into()))?;
        if event.allocation.usage(elapsed).ok().as_ref() != Some(&event.quantity) {
            return Err(FundingError::Invalid(
                "retained usage arithmetic differs".into(),
            ));
        }
        if event.ended_at < window.covered_from {
            continue;
        }
        if event.started_at >= window.covered_until {
            break;
        }
        if event.ended_at > now {
            return Err(FundingError::Invalid(
                "retained usage cannot attest the future".into(),
            ));
        }
        if previous_end.is_some_and(|end| end != event.started_at)
            || event.started_at.max(window.covered_from) != until
        {
            return Err(FundingError::Invalid(
                "retained usage leaves a gap or overlap".into(),
            ));
        }
        let end = event.ended_at.min(window.covered_until);
        if end > until
            && (event.allocation.cpu_millis > window.allocation.cpu_millis
                || event.allocation.memory_mib > window.allocation.memory_mib
                || event.allocation.disk_gib > window.allocation.disk_gib)
        {
            return Err(FundingError::Invalid(
                "observed allocation exceeds confirmed funding".into(),
            ));
        }
        selected.push(EvidenceInterval {
            revision: event.revision,
            started_at: event.started_at,
            ended_at: event.ended_at,
            allocation: event.allocation.clone(),
            quantity: event.quantity.clone(),
        });
        if selected.len() > 1024 {
            return Err(FundingError::Invalid(
                "funded window exceeds evidence interval limit".into(),
            ));
        }
        previous_end = Some(event.ended_at);
        until = end;
        if until == window.covered_until {
            return Ok(Some(selected));
        }
    }
    Ok(None)
}
