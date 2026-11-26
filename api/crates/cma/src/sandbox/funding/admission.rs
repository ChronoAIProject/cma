//! Persist the funding decision before a lifecycle effect or reservation.
use super::*;

impl ProviderFundingAdapter {
    pub(super) async fn recover_pending(
        &self,
        state: &mut SandboxBillingState,
    ) -> Result<(), FundingError> {
        let Some(request) = state.pending.clone() else {
            return Ok(());
        };
        // A 404 is not proof of non-commit: reserve replays the identical durable payload.
        let receipt = match self.provider.recover(&request.request_id).await? {
            Some(receipt) => receipt,
            None => self.provider.reserve(request.clone()).await?,
        };
        self.adopt_pending(state, &request, receipt).await
    }

    pub(super) async fn fund(
        &self,
        sandbox: &Sandbox,
        state: &mut SandboxBillingState,
        allocation: Allocation,
        now: u64,
    ) -> Result<(), FundingError> {
        // Finish any ambiguous mutation before deciding whether to create another one.
        if state.pending.is_some() {
            self.recover_pending(state).await?;
        }
        let target_generation = generation(sandbox);
        if state.unbilled_generation.is_some() {
            // No funded history exists for an excluded generation. A later owner
            // lifecycle may enroll prospectively, even after several free resumes.
            debug_assert!(
                state.current.is_none() && state.pending.is_none() && state.windows.is_empty()
            );
            state.runtime_generation = target_generation;
            state.operation_id = uuid::Uuid::new_v4().to_string();
            state.source_operation = source_operation(sandbox);
            state.unbilled_generation = None;
            state.stopped = false;
            state.quote = None;
            self.state.save(state).await?;
        }
        if target_generation != state.runtime_generation {
            if target_generation
                != state
                    .runtime_generation
                    .checked_add(1)
                    .ok_or_else(|| FundingError::Invalid("generation overflow".into()))?
            {
                return Err(FundingError::Invalid(
                    "NyxID requires the next consecutive funded runtime generation".into(),
                ));
            }
            if state
                .current
                .as_ref()
                .is_some_and(|w| w.covered_until > now)
                || !state.windows.is_empty()
            {
                return Err(FundingError::Retryable(
                    "prior coverage and retained evidence must finish before replacement".into(),
                ));
            }
            state.runtime_generation = target_generation;
            state.operation_id = uuid::Uuid::new_v4().to_string();
            state.source_operation = source_operation(sandbox);
            state.quote = None;
            state.stopped = false;
            self.state.save(state).await?;
        }
        if let Some(current) = state
            .current
            .as_ref()
            .filter(|w| w.runtime_generation == target_generation)
        {
            if allocation.cpu_millis > current.allocation.cpu_millis
                || allocation.memory_mib > current.allocation.memory_mib
                || allocation.disk_gib > current.allocation.disk_gib
            {
                return Err(FundingError::Unfunded("confirmed window does not cover the requested allocation; a new generation is required".into()));
            }
            if current.covered_until <= now {
                return Err(FundingError::Unfunded(
                    "coverage expired; a new authorized runtime generation is required".into(),
                ));
            }
            if current.covered_until > now.saturating_add(RENEWAL_MARGIN_SECONDS) {
                return Ok(());
            }
        }
        if state.windows.len() >= 32 {
            return Err(FundingError::Retryable(
                "retained evidence backlog prevents new coverage".into(),
            ));
        }
        // A quote persisted before a crash is not current cohort authority.
        // Until any reservation may exist, recheck money-free admission first.
        if state.quote.is_none() || state.current.is_none() {
            let maximum = maximum_allocation(sandbox);
            let quote = match self
                .provider
                .quote(QuoteRequest {
                    owner_id: state.owner_id.clone(),
                    sandbox_id: state.sandbox_id.clone(),
                    allocation: maximum.clone(),
                })
                .await
            {
                Ok(quote) => quote,
                Err(FundingError::NotEnrolled)
                    if state.current.is_none()
                        && state.pending.is_none()
                        && state.windows.is_empty() =>
                {
                    state.unbilled_generation = Some(target_generation);
                    state.quote = None;
                    state.retained_disk_gib = 0;
                    state.last_error = None;
                    state.attempts = 0;
                    self.state.save(state).await?;
                    tracing::info!(
                        "Sandbox generation admitted without billing: outside paid admission"
                    );
                    return Ok(());
                }
                Err(error) => return Err(error),
            };
            if quote.owner_id != state.owner_id
                || quote.sandbox_id != state.sandbox_id
                || quote.maximum_allocation != maximum
                || quote.accept_before <= now
            {
                return Err(FundingError::Invalid(
                    "quote differs from owner, Sandbox or maximum allocation".into(),
                ));
            }
            state.quote = Some(quote);
            self.state.save(state).await?;
        }
        let quote_id = state
            .quote
            .as_ref()
            .expect("persisted quote")
            .quote_id
            .clone();
        state.pending = Some(ReserveRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            quote_id,
            owner_id: state.owner_id.clone(),
            sandbox_id: state.sandbox_id.clone(),
            runtime_generation: state.runtime_generation,
            operation_id: state.operation_id.clone(),
            previous_window_id: state.current.as_ref().map(|w| w.window_id.clone()),
            seconds: WINDOW_SECONDS,
            allocation,
        });
        self.state.save(state).await?;
        self.recover_pending(state).await
    }
}
