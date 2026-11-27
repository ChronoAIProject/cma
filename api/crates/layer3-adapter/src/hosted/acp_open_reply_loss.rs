use super::acp::Bridge;
use super::acp_open_completion::{binding_digest_with_policy, HostedOpenCompletionProtocol};
use crate::ingress_budget::diagnostic::text;
use crate::AdapterError;

/// Operator-supplied one-shot claim for a disposable staging Sandbox.
/// A claim must be shared across Agent Manager replicas.
#[async_trait::async_trait]
pub trait HostedOpenReplyLoss: Send + Sync {
    async fn claim(&self) -> bool;
}

impl Bridge {
    pub(super) async fn drop_proven_open_reply(
        &self,
        claim: &dyn HostedOpenReplyLoss,
        protocol: HostedOpenCompletionProtocol,
        conversation: &str,
        cwd: &str,
        tool_servers: &[serde_json::Value],
    ) -> crate::Result<()> {
        // The reply has reached this attachment, but its identity has not been
        // published to the worker. Leave the native bridge and its proof intact.
        let Ok(digest) = binding_digest_with_policy(cwd, tool_servers, self.ingress.as_ref())
        else {
            return Ok(());
        };
        if self
            .read_open_completion(protocol, conversation, &digest)
            .await
            .is_err()
        {
            tracing::warn!("staging ACP open reply retained because completion was unproved");
            return Ok(());
        }
        if claim.claim().await {
            tracing::info!(
                agent_correlation_digest = %self.agent_correlation_digest,
                "staging ACP open reply dropped after proven completion"
            );
            return Err(AdapterError::Unavailable(text(
                "staging ACP open reply dropped",
            )));
        }
        Ok(())
    }
}
