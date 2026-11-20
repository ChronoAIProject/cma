use super::*;

impl AdmittedConversationInputReceiver {
    pub async fn run<F, Fut>(mut self, mut handler: F)
    where
        F: FnMut(OwnedWork<ConversationInput>) -> Fut,
        Fut: Future<Output = Result<(), AdmittedError>>,
    {
        let mut cancelled = self.cancelled.clone();
        tokio::select! {
            biased;
            _ = async {
                loop {
                    if cancelled.latest() { return; }
                    if cancelled.changed().await.is_err() {
                        std::future::pending::<()>().await;
                    }
                }
            } => (),
            _ = self.drive(&mut handler) => (),
        }
    }

    async fn drive<F, Fut>(&mut self, handler: &mut F)
    where
        F: FnMut(OwnedWork<ConversationInput>) -> Fut,
        Fut: Future<Output = Result<(), AdmittedError>>,
    {
        use AdmittedConversationSendOutcome::{Ambiguous, PreWriteFailure, WriteConfirmed};
        while let Some(mut pending) = self.pending.recv().await {
            let delivered = pending.delivered.as_mut().expect("pending delivery");
            if delivered.is_closed() || pending.state.load(Ordering::Acquire) == CANCELLED {
                continue;
            }
            if let Some(guard) = &pending.guard {
                let class = match pending.input.as_deref() {
                    Some(
                        ConversationInput::Cancel | ConversationInput::PermissionAnswer { .. },
                    ) => IngressClass::Control,
                    _ => IngressClass::Data,
                };
                let checked = tokio::select! {
                    biased;
                    _ = delivered.closed() => continue,
                    checked = check_guard(&**guard, &self.policy, class) => checked,
                };
                if let Err(error) = checked {
                    pending.finish(PreWriteFailure(error));
                    continue;
                }
            }
            if pending
                .state
                .compare_exchange(PENDING, WRITING, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                continue;
            }
            let input = pending.input.take().expect("one input");
            let configuring = matches!(
                &*input,
                ConversationInput::Configure { .. }
                    | ConversationInput::RefreshContext { .. }
                    | ConversationInput::Steer(_)
            );
            let refreshing = matches!(&*input, ConversationInput::RefreshContext { .. });
            let prompt = matches!(
                &*input,
                ConversationInput::Prompt(_) | ConversationInput::RecoverablePrompt { .. }
            );
            let result = match handler(input).await {
                Err(error) if !error.belongs_to(&self.policy) => {
                    Err(AdmissionRefusal::InvalidPlan {
                        phase: "input outcome ownership",
                    }
                    .into())
                }
                result => result,
            };
            let refused = result.as_ref().err().is_some_and(|error| {
                configuring
                    && (matches!(error, AdmittedError::Unsupported(_))
                        || matches!(error.as_adapter(), Some(AdapterError::Unsupported(_))))
                    || refreshing
                        && matches!(
                            error.as_adapter(),
                            Some(AdapterError::OperationIdentityConflict(_))
                        )
            });
            let failed = result.is_err() && !refused;
            let outcome = match result {
                Err(error)
                    if prompt
                        && matches!(
                            error.as_adapter(),
                            Some(AdapterError::PromptNotWritten(_))
                        ) =>
                {
                    PreWriteFailure(error)
                }
                Err(error) if refused => PreWriteFailure(error),
                Ok(()) => WriteConfirmed,
                Err(error) => Ambiguous(error),
            };
            pending.finish(outcome);
            if failed {
                self.pending.close();
                break;
            }
        }
    }
}

async fn check_guard(
    guard: &dyn ConversationInputGuard,
    policy: &IngressOwnershipPolicy,
    class: IngressClass,
) -> Result<(), AdmittedError> {
    let storage = policy.admit(class, guard.admitted_plan()?)?;
    let scope = IngressWorkScope::new(storage);
    match scope.run(guard.check()).await.transpose() {
        Ok(_) => Ok(()),
        Err(error) => Err(error.into_shared_admitted()?.into()),
    }
}
