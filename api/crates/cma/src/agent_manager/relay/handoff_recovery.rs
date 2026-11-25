use super::*;
use axum::{extract::Request, middleware::Next, response::Response};
use layer2_core::sandbox::agent_filesystem::FileWriteTarget;
use relay_diagnostics::{RecoveryTrigger, RelayDiagnostic};
use tokio::time::Instant;

pub(super) struct Recovery<'a> {
    pub permit: capacity::Permit,
    pub deadline: Instant,
    pub diagnostic: Option<&'a RelayDiagnostic>,
    pub trigger: RecoveryTrigger,
    pub write_target: Option<FileWriteTarget>,
}

impl Recovery<'_> {
    pub(super) async fn run(
        self,
        relay: &Relay,
        id: &AgentId,
        mut request: Request,
        next: Next,
    ) -> Response {
        let Self {
            permit,
            deadline,
            diagnostic,
            trigger,
            write_target,
        } = self;
        if let Some(diagnostic) = diagnostic {
            diagnostic.recovery(trigger, None);
        }
        let response = tokio::time::timeout_at(deadline, async {
            if !relay.coordinator.is_ready() {
                return http::unavailable_with(diagnostic, Refusal::CoordinatorNotReady);
            }
            if let Err(refusal) =
                http::authorize_public(relay, id, &request, diagnostic, false).await
            {
                return refusal;
            }
            if let Some(target) = write_target {
                request.extensions_mut().insert(target);
            }
            next.run(request).await
        })
        .await
        .unwrap_or_else(|_| http::unavailable_with(diagnostic, Refusal::HandoffRecoveryDeadline));
        if let Some(diagnostic) = diagnostic {
            diagnostic.recovery(trigger, Some(response.status()));
        }
        http::retain_until(response, permit, Some(deadline))
    }
}
