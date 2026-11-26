use super::*;
use crate::sandbox::lifecycle_failure::{LifecycleAttempt, LifecycleTaskFailure, Origin, Phase};
use std::cell::Cell;

tokio::task_local! { static ORIGIN: Cell<Origin>; }

pub(super) fn executing() {
    // Execution includes protected authority renewal; do not infer a finer cause.
    let _ = ORIGIN.try_with(|origin| origin.set(Origin::Execute));
}

impl SandboxWorker {
    pub(in crate::sandbox) fn run_one_diagnostic(
        &self,
        id: &SandboxId,
        now: SystemTime,
    ) -> Pin<Box<dyn Future<Output = Result<bool, LifecycleTaskFailure>> + Send + '_>> {
        let id = id.clone();
        Box::pin(async move {
            let attempt = LifecycleAttempt::new(Phase::Execution, None);
            ORIGIN
                .scope(Cell::new(Origin::Claim), async {
                    self.run_one_inner(&id, now)
                        .await
                        .map_err(|error| attempt.store(ORIGIN.with(Cell::get), error))
                })
                .await
        })
    }
}
