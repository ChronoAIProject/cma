use super::*;

impl ProviderFundingAdapter {
    pub(super) async fn adopt_pending(
        &self,
        state: &mut SandboxBillingState,
        request: &ReserveRequest,
        receipt: FundingWindow,
    ) -> Result<(), FundingError> {
        if state.pending.as_ref() != Some(request)
            || state
                .current
                .as_ref()
                .is_some_and(|current| current.window_id == receipt.window_id)
            || state.windows.iter().any(|retained| {
                retained.window.request_id == receipt.request_id
                    || retained.window.window_id == receipt.window_id
            })
        {
            return Err(FundingError::Invalid(
                "pending receipt is not a new retained window".into(),
            ));
        }
        validate_receipt(request, &receipt, state.current.as_ref())?;
        let mut next = state.clone();
        if next
            .current
            .as_ref()
            .is_none_or(|previous| previous.runtime_generation != receipt.runtime_generation)
        {
            next.coverage_start = receipt.covered_from;
        }
        next.current = Some(receipt.clone());
        next.windows.push(RetainedWindow {
            window: receipt,
            evidence: None,
        });
        next.pending = None;
        next.attempts = 0;
        next.last_error = None;
        self.state.save(&mut next).await?;
        *state = next;
        Ok(())
    }

    pub(super) async fn reconcile_missing(
        &self,
        mut state: SandboxBillingState,
        now: SystemTime,
    ) -> Result<(), FundingError> {
        state.stopped = true;
        state.retained_disk_gib = 0;
        state.retry_at = seconds(now).saturating_add(60);
        state.last_error = unresolved(&state);
        self.state.save(&mut state).await?;

        // Previously frozen windows can progress even when pending lookup is unavailable.
        if let Err(error) = self.emit_evidence(&mut state, seconds(now)).await {
            if let FundingError::RateLimited(delay) = error {
                self.extend_missing_retry(&state, now, delay).await?;
                return Err(FundingError::RateLimited(delay));
            }
            return Err(error);
        }
        if let Some(request) = checked_pending(&state)?.cloned() {
            let receipt = match self.provider.recover(&request.request_id).await {
                Ok(Some(receipt)) => receipt,
                result => {
                    let error = result.err().unwrap_or(FundingError::NotFound);
                    if let FundingError::RateLimited(delay) = &error {
                        state.retry_at = seconds(now)
                            .saturating_add((*delay).max(60))
                            .min(i64::MAX as u64);
                    }
                    state.attempts = state.attempts.saturating_add(1);
                    state.last_error = Some(error.to_string());
                    self.state.save(&mut state).await?;
                    return Err(error);
                }
            };
            self.adopt_pending(&mut state, &request, receipt).await?;
        }
        state.last_error = unresolved(&state);
        self.state.save(&mut state).await
    }

    async fn extend_missing_retry(
        &self,
        observed: &SandboxBillingState,
        now: SystemTime,
        delay: u64,
    ) -> Result<(), FundingError> {
        let mut durable = self
            .state
            .load(&SandboxId(observed.sandbox_id.clone()))
            .await?
            .ok_or(FundingError::Conflict)?;
        if durable.revision != observed.revision {
            return Err(FundingError::Conflict);
        }
        durable.retry_at = durable.retry_at.max(
            seconds(now)
                .saturating_add(delay.max(60))
                .min(i64::MAX as u64),
        );
        self.state.save(&mut durable).await
    }
}

fn unresolved(state: &SandboxBillingState) -> Option<String> {
    (!state.windows.is_empty() || state.pending.is_some())
        .then(|| "purged Sandbox has unresolved funding or retained evidence".into())
}

fn checked_pending(state: &SandboxBillingState) -> Result<Option<&ReserveRequest>, FundingError> {
    let Some(request) = &state.pending else {
        return Ok(None);
    };
    let predecessor = state.current.as_ref();
    if request.owner_id != state.owner_id
        || request.sandbox_id != state.sandbox_id
        || request.runtime_generation != state.runtime_generation
        || request.operation_id != state.operation_id
        || request.request_id.is_empty()
        || request.operation_id.is_empty()
        || request.quote_id.is_empty()
        || request.previous_window_id.as_deref()
            != predecessor.map(|window| window.window_id.as_str())
        || predecessor.is_some_and(|window| {
            window.owner_id != state.owner_id
                || window.sandbox_id != state.sandbox_id
                || window.runtime_generation > state.runtime_generation
                || window.request_id == request.request_id
        })
        || state.quote.as_ref().is_none_or(|quote| {
            quote.quote_id != request.quote_id
                || quote.owner_id != state.owner_id
                || quote.sandbox_id != state.sandbox_id
        })
    {
        return Err(FundingError::Invalid(
            "pending funding identity differs from retained state".into(),
        ));
    }
    Ok(Some(request))
}
