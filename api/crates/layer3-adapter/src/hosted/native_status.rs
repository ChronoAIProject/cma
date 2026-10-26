//! One fresh graph proof authorizes one status request after exclusive checkout.
use super::super::NativeStatusInspection;
use super::*;

impl ProviderHostedControlTransport {
    pub(super) async fn inspect_reused_native_status(
        &self,
        inspection: &NativeStatusInspection<'_>,
    ) -> crate::Result<Option<crate::RuntimeInspection>> {
        let RuntimeDiscovery::AgentSandbox(client) = &self.client else {
            return Ok(None);
        };
        // An independent client's graph cannot authorize this transport's Pod.
        if self.native_transport == NativeRuntimeTransport::Direct
            || !inspection.shares_client(client)
        {
            return Ok(None);
        }
        let handle = inspection.handle();
        let (identity, binding) = self.native_endpoint_state(handle).await;
        let Some(binding) = binding else {
            return Ok(None);
        };
        let (sandbox_id, request_sha256) = inspection.identity()?;
        if identity.sandbox_id.as_ref() != Some(&sandbox_id)
            || identity.request_sha256.as_ref() != Some(&request_sha256)
        {
            return Ok(None);
        }
        let Ok(_request) = self.native_http_budget.clone().try_acquire_owned() else {
            // Retain graph-before-capacity errors on the ordinary path.
            return Ok(None);
        };
        let address = std::net::SocketAddr::new(binding.pod_ip, SUPERVISOR_PORT);
        let endpoint =
            RuntimeEndpoint::parse("http://runtime.invalid", &format!("http://{address}"))?
                .with_control_token(client.control_token(&binding.sandbox_name))
                .with_native_binding(handle.clone(), binding.clone())
                .with_native_identity(identity);
        let slot = self.native_slot(&endpoint).await?;
        let mut cancelled = slot.cancellation();
        let exchange = async {
            let lease = self.native_checkout(&endpoint, slot.clone()).await?;
            if lease
                .connection
                .as_ref()
                .is_none_or(|connection| connection.sender.is_closed())
            {
                // New streams retain their existing pre/post-open observations.
                return Ok(None);
            }
            // No earlier observation or queued request can supply this proof.
            let Some(proof) = inspection.prove().await? else {
                return Ok(None);
            };
            if proof.pod() != &binding {
                self.discard_native_slot(&endpoint, &slot).await;
                return Err(AdapterError::Rejected(
                    "Sandbox Pod changed before HTTP reuse".into(),
                ));
            }
            lease.current().await?;
            let response = self
                .status_on_native_lease(&endpoint, proof.request(), lease)
                .await?;
            proof.complete(response).map(Some)
        };
        tokio::time::timeout(CONTROL_DEADLINE, async {
            tokio::select! { biased;
                _ = cancelled.wait_for(|cancelled| *cancelled) => Err(native_pool::retired()),
                result = exchange => result,
            }
        })
        .await
        .map_err(|_| AdapterError::Unavailable("runtime control request timed out".into()))?
    }
}

#[cfg(test)]
#[path = "native_status_test_support.rs"]
mod test_support;
