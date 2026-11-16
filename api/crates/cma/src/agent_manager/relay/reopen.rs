//! Streaming lifecycle recovery stays on the Agent owner, including output ACKs.
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
    ) -> layer3_adapter::Result<AgentWorkReceipt> {
        self.reopen_call(sandbox, id).await.map_err(|_| {
            layer3_adapter::AdapterError::Unavailable(
                "Agent owner lifecycle reopen unavailable".into(),
            )
        })
    }

    async fn reopen_call(&self, sandbox: &Sandbox, id: &AgentId) -> Result<AgentWorkReceipt, ()> {
        let _permit = self.slots.clone().try_acquire_owned().map_err(|_| ())?;
        let destination = self.control_destination(id).await?;
        let call = ReopenCall {
            sandbox_id: sandbox.id.clone(),
            revision: sandbox.revision,
            lease: sandbox.work.lease.clone().ok_or(())?,
        };
        let reply = if destination.member.instance_id == self.coordinator.instance_id {
            let worker = self.reopen.get().ok_or(())?;
            let receipt = tokio::time::timeout(
                Duration::from_secs(60),
                destination.authority.run(worker.reopen_at_owner(id, &call)),
            )
            .await
            .map_err(|_| ())?
            .map_err(|_| ())?
            .map_err(|_| ())?;
            Reply {
                owner: destination.token.clone(),
                receipt,
            }
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
            let proof = self.authentication.sign(
                &self.coordinator.instance_id,
                &destination.token,
                &axum::http::Method::POST,
                &uri,
                &headers,
                &body,
            )?;
            let url = format!(
                "{}{}",
                destination.member.relay_address.trim_end_matches('/'),
                uri
            );
            tokio::time::timeout(Duration::from_secs(60), async {
                let mut response = self
                    .client
                    .post(url)
                    .headers(headers)
                    .header(PROOF, proof)
                    .body(body)
                    .send()
                    .await
                    .map_err(|_| ())?;
                if !response.status().is_success() {
                    return Err(());
                }
                let mut bytes = Vec::new();
                while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
                    if chunk.len() > 16_384_usize.saturating_sub(bytes.len()) {
                        return Err(());
                    }
                    bytes.extend_from_slice(&chunk);
                }
                serde_json::from_slice::<Reply>(&bytes).map_err(|_| ())
            })
            .await
            .map_err(|_| ())??
        };
        if reply.owner.agent_id != *id
            || reply.owner.instance_id != destination.member.instance_id
            || (destination.token.epoch != 0 && reply.owner != destination.token)
        {
            return Err(());
        }
        bounded(self.coordinator.store.check(&reply.owner))
            .await
            .map_err(|_| ())?;
        layer2_core::agent_ownership::observe_transport(reply.owner).map_err(|_| ())?;
        Ok(reply.receipt)
    }
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
        Duration::from_secs(60),
        worker.reopen_at_owner(&authority.token.agent_id, &call),
    )
    .await
    {
        Ok(Ok(receipt)) => Json(Reply {
            owner: authority.token.clone(),
            receipt,
        })
        .into_response(),
        _ => http::unavailable(),
    }
}
