//! Bounded admission enrichment after the Ready publication.
use super::*;

impl AgentWorker {
    fn completion_admission(&self, agent: Agent) -> CompletionAdmissionFuture {
        #[cfg(test)]
        if let Some(lookup) = self
            .completion_admission_lookup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        {
            return lookup(agent);
        }
        let store = self.dependencies.store.clone();
        Box::pin(async move { store.agent_create_admission(&agent).await })
    }

    #[cfg(test)]
    pub(crate) fn set_completion_admission_lookup<F, Fut>(&self, lookup: F)
    where
        F: Fn(Agent) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Option<AgentCreateAdmission>, SandboxStoreError>>
            + Send
            + 'static,
    {
        let lookup: CompletionAdmissionLookup = Arc::new(move |agent| Box::pin(lookup(agent)));
        *self
            .completion_admission_lookup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(lookup);
    }

    #[cfg(test)]
    pub(crate) fn schedule_completion_event_for_test(&self, agent: Agent) {
        self.schedule_completion_event(
            agent,
            "ordinary",
            "unknown",
            create_completion::CompletionTiming {
                admission_ms: None,
                admitted_desktop: None,
                admitted_machine_included: None,
                ready_at: SystemTime::now(),
                publish_ready_ms: 0,
                startup_phases: None,
            },
        );
    }

    pub(super) fn schedule_completion_event(
        &self,
        agent: Agent,
        ready_path: &'static str,
        runtime_path: &'static str,
        timing: create_completion::CompletionTiming,
    ) {
        let permit = match Arc::clone(&self.completion_admission_slots).try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                tracing::warn!(
                    target: "agent_create_completion_lookup",
                    parent: None,
                    outcome = "admission_lookup_saturated",
                    "Agent create completion admission lookup was skipped"
                );
                create_completion::emit(&agent, ready_path, runtime_path, timing);
                return;
            }
        };
        let admission_lookup = self.completion_admission(agent.clone());
        tokio::spawn(async move {
            let _permit = permit;
            let admission =
                match tokio::time::timeout(CREATE_COMPLETION_ADMISSION_TIMEOUT, admission_lookup)
                    .await
                {
                    Ok(Ok(admission)) => admission,
                    Ok(Err(_)) => {
                        tracing::debug!(
                            target: "agent_create_completion_lookup",
                            parent: None,
                            outcome = "admission_lookup_failed",
                            "Agent create completion admission lookup failed"
                        );
                        None
                    }
                    Err(_) => {
                        tracing::debug!(
                            target: "agent_create_completion_lookup",
                            parent: None,
                            outcome = "admission_lookup_timeout",
                            "Agent create completion admission lookup timed out"
                        );
                        None
                    }
                };
            create_completion::emit(
                &agent,
                ready_path,
                runtime_path,
                create_completion::CompletionTiming {
                    admission_ms: admission.as_ref().map(|value| value.admitted_at_ms),
                    admitted_desktop: admission.as_ref().and_then(|value| value.desktop),
                    admitted_machine_included: admission
                        .as_ref()
                        .and_then(|value| value.machine_included),
                    ..timing
                },
            );
        });
    }
}
