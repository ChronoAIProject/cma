//! Streaming lifecycle recovery stays on the Agent owner, including output ACKs.
use super::call_failure::CallFailure;
use super::*;
use axum::{
    extract::State,
    response::{IntoResponse, Response},
    Extension, Json,
};
use layer2_core::{
    sandbox::{AgentWorkReceipt, Sandbox, SandboxWorkLease},
    SandboxId,
};
use tokio::time::Instant;

const REOPEN_LIMIT: Duration = Duration::from_secs(layer3_adapter::DEFAULT_TOTAL_TIMEOUT_SECONDS);

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReopenCall {
    pub sandbox_id: SandboxId,
    pub revision: u64,
    pub lease: SandboxWorkLease,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    owner: OwnershipToken,
    receipt: AgentWorkReceipt,
}

impl Relay {
    pub(crate) fn install_reopen(
        self: &Arc<Self>,
        worker: Arc<crate::sandbox::worker::SandboxWorker>,
        agents: Arc<dyn layer2_core::sandbox::store::AgentStore>,
    ) {
        worker.install_reopen_relay(self, agents);
        assert!(self.reopen.set(worker).is_ok());
    }

    pub(crate) async fn reopen_agent(
        &self,
        sandbox: &Sandbox,
        id: &AgentId,
        verify_refusal: bool,
    ) -> layer3_adapter::Result<AgentWorkReceipt> {
        self.reopen_call(sandbox, id, verify_refusal)
            .await
            .map_err(|error| error.into_adapter("Agent owner lifecycle reopen unavailable"))
    }

    async fn reopen_call(
        &self,
        sandbox: &Sandbox,
        id: &AgentId,
        verify_refusal: bool,
    ) -> Result<AgentWorkReceipt, CallFailure> {
        let deadline = Instant::now() + REOPEN_LIMIT;
        let _permit = self
            .acquire(capacity::Class::Control)
            .map_err(|_| CallFailure::RelayCapacity)?;
        let destination = self.control_destination(id).await?;
        let call = ReopenCall {
            sandbox_id: sandbox.id.clone(),
            revision: sandbox.revision,
            lease: sandbox.work.lease.clone().ok_or(())?,
        };
        let reply = if destination.member.instance_id == self.coordinator.instance_id {
            let worker = self.reopen.get().ok_or(())?;
            tokio::time::timeout_at(
                deadline,
                destination.authority.run(worker.reopen_at_owner(id, &call)),
            )
            .await
            .map_err(|_| ())?
            .map_err(|_| ())?
            .map(|receipt| Reply {
                owner: destination.token.clone(),
                receipt,
            })
            .map_err(CallFailure::from)
        } else {
            let body = serde_json::to_vec(&call).map_err(|_| ())?;
            let uri: axum::http::Uri = format!("{PREFIX}/control/{id}/reopen")
                .parse()
                .map_err(|_| ())?;
            let mut headers = axum::http::HeaderMap::new();
            headers.insert(
                axum::http::header::CONTENT_TYPE,
                axum::http::HeaderValue::from_static("application/json"),
            );
            let headers = self.authentication.signed_headers(
                &self.coordinator.instance_id,
                &destination.token,
                &axum::http::Method::POST,
                &uri,
                headers,
                &body,
            )?;
            let url = format!(
                "{}{}",
                destination.member.relay_address.trim_end_matches('/'),
                uri
            );
            tokio::time::timeout_at(deadline, async {
                let mut response = self
                    .reopen_client
                    .post(url)
                    .headers(headers)
                    .body(body)
                    .send()
                    .await
                    .map_err(|_| ())?;
                if response.status() == reqwest::StatusCode::CONFLICT {
                    return Err(rejected_from(response).await);
                }
                if !response.status().is_success() {
                    return Err(CallFailure::remote(
                        response,
                        layer3_adapter::agent_sandbox::RuntimeWorkloadClass::Continuity,
                    )
                    .await);
                }
                let mut bytes = Vec::new();
                while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
                    if chunk.len() > 16_384_usize.saturating_sub(bytes.len()) {
                        return Err(CallFailure::Unavailable);
                    }
                    bytes.extend_from_slice(&chunk);
                }
                serde_json::from_slice::<Reply>(&bytes).map_err(|_| CallFailure::Unavailable)
            })
            .await
            .map_err(|_| ())?
        };
        let reply = verify_refused_owner(
            self.coordinator.store.as_ref(),
            &destination.token,
            verify_refusal,
            reply,
        )
        .await?;
        if reply.owner.agent_id != *id
            || reply.owner.instance_id != destination.member.instance_id
            || (destination.token.epoch != 0 && reply.owner != destination.token)
        {
            return Err(CallFailure::Unavailable);
        }
        bounded(self.coordinator.store.check(&reply.owner))
            .await
            .map_err(|_| ())?;
        layer2_core::agent_ownership::observe_transport(reply.owner).map_err(|_| ())?;
        Ok(reply.receipt)
    }
}

async fn verify_refused_owner<T>(
    store: &dyn layer2_core::agent_ownership::OwnershipStore,
    token: &OwnershipToken,
    verify_refusal: bool,
    result: Result<T, CallFailure>,
) -> Result<T, CallFailure> {
    if matches!(&result, Err(CallFailure::Runtime(error)) if output_refused(error)) {
        // A claim invitation proves no owner epoch. Re-dispatch after the peer
        // claims a real epoch before using its refusal to settle the Sandbox.
        if !verify_refusal || token.epoch == 0 {
            return Err(CallFailure::Unavailable);
        }
        bounded(store.check(token)).await.map_err(|_| ())?;
        layer2_core::agent_ownership::observe_transport(token.clone()).map_err(|_| ())?;
    }
    result
}

pub(super) async fn serve(
    State(relay): State<Arc<Relay>>,
    Extension(_peer): Extension<http::VerifiedPeer>,
    Json(call): Json<ReopenCall>,
) -> Response {
    let Some(authority) = layer2_core::agent_ownership::execution_authority() else {
        return http::unavailable();
    };
    let Some(worker) = relay.reopen.get() else {
        return http::unavailable();
    };
    match tokio::time::timeout(
        REOPEN_LIMIT,
        worker.reopen_at_owner(&authority.token.agent_id, &call),
    )
    .await
    {
        Ok(Ok(receipt)) => Json(Reply {
            owner: authority.token.clone(),
            receipt,
        })
        .into_response(),
        Ok(Err(error)) => failure_response(error),
        _ => http::unavailable(),
    }
}

/// Closed code for a terminal owner refusal of a relayed reopen (#3265). The
/// same-process path keeps the error class; the HTTP path must too, or the
/// caller retries a refusal forever. The error text never crosses the relay.
const REJECTED: &str = "owner_reopen_rejected";
// A caller that only knows #3265 treats this code as retryable. The caller
// must opt in after capturing its checkpoint before accepting the refusal.
const OUTPUT_DISCONTINUITY: &str = "owner_reopen_output_discontinuity";
pub(crate) const OUTPUT_REFUSED: &str =
    "durable reopen output was refused: runtime output receipt is not contiguous with its conversation";

pub(crate) fn output_refused(error: &layer3_adapter::AdapterError) -> bool {
    matches!(error, layer3_adapter::AdapterError::Rejected(detail) if detail == OUTPUT_REFUSED)
}

fn failure_response(error: layer3_adapter::AdapterError) -> Response {
    if output_refused(&error) {
        rejected_response(OUTPUT_DISCONTINUITY)
    } else if terminal(&error) {
        rejected_response(REJECTED)
    } else {
        call_failure::response(error)
    }
}

/// The refusals the lifecycle treats as terminal validation.
fn terminal(error: &layer3_adapter::AdapterError) -> bool {
    use layer3_adapter::AdapterError::*;
    matches!(
        error,
        Rejected(_) | Unsupported(_) | OperationIdentityConflict(_) | SkillUpgradeRefused(_)
    )
}

fn rejected_response(code: &'static str) -> Response {
    use axum::response::IntoResponse;
    (
        axum::http::StatusCode::CONFLICT,
        [("content-type", "application/problem+json")],
        Json(serde_json::json!({
            "type": "about:blank", "title": "Agent owner rejected the lifecycle reopen",
            "status": 409, "code": code,
        })),
    )
        .into_response()
}

/// An unknown 409 stays retryable, exactly as before this code existed.
async fn rejected_from(mut response: reqwest::Response) -> CallFailure {
    let mut bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) if chunk.len() <= 4096_usize.saturating_sub(bytes.len()) => {
                bytes.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            _ => return CallFailure::Unavailable,
        }
    }
    let Ok(body) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return CallFailure::Unavailable;
    };
    let detail = match body["code"].as_str() {
        Some(OUTPUT_DISCONTINUITY) => OUTPUT_REFUSED,
        Some(REJECTED) => "Agent owner rejected the lifecycle reopen",
        _ => return CallFailure::Unavailable,
    };
    CallFailure::Runtime(layer3_adapter::AdapterError::Rejected(detail.into()))
}

#[cfg(test)]
#[path = "reopen_tests.rs"]
mod tests;
