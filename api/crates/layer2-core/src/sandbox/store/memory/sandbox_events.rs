use super::*;

#[async_trait]
impl SandboxEventStore for InMemorySandboxStore {
    async fn lifecycle_retry_started_at(
        &self,
        sandbox_id: &SandboxId,
        admitted_at: &str,
    ) -> Result<Option<String>, SandboxStoreError> {
        let state = self.lock();
        let Some(events) = state.sandbox_events.get(sandbox_id) else {
            return Ok(None);
        };
        Ok(events
            .iter()
            .rev()
            .take_while(|event| {
                !(event.appended_at.as_str() >= admitted_at
                    && matches!(
                        event.kind,
                        SandboxEventKind::WorkStepCompleted {
                            step: crate::sandbox::SandboxWorkStep::RunLifecycleOperation
                        }
                    ))
            })
            .filter(|event| {
                event.appended_at.as_str() >= admitted_at
                    && matches!(
                        event.kind,
                        SandboxEventKind::WorkStepFailed {
                            step: crate::sandbox::SandboxWorkStep::RunLifecycleOperation,
                            ..
                        }
                    )
            })
            .last()
            .map(|event| event.appended_at.clone()))
    }

    async fn append_sandbox_event(
        &self,
        sandbox_id: &SandboxId,
        appended_at: String,
        kind: SandboxEventKind,
    ) -> Result<SandboxEvent, SandboxStoreError> {
        let mut state = self.lock();
        if !state.sandboxes.contains_key(sandbox_id) {
            return Err(SandboxStoreError::NotFound);
        }
        append_sandbox_event(&mut state, sandbox_id, appended_at, kind)
    }

    async fn sandbox_events(
        &self,
        sandbox_id: &SandboxId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<SandboxEvent>, SandboxStoreError> {
        bounded_events(
            self.lock().sandbox_events.get(sandbox_id),
            after,
            limit,
            |event| event.seq,
        )
    }
}
