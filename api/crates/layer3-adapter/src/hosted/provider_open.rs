//! Overlap only unauthenticated stream preparation with the actual fenced Open.
use super::*;

impl ProviderHostedControlTransport {
    pub(super) async fn open_and_connect(
        &self,
        endpoint: &RuntimeEndpoint,
        launch: &FencedRequest<AgentLaunch>,
        timing: &mut super::super::startup_timing::StartupTiming,
    ) -> crate::Result<(
        FencedReceipt<AgentOpenReceipt>,
        Socket,
        Option<ownership::ReceiverLease>,
    )> {
        let opening = async {
            let request = RequestFrame::OpenAgent(launch.clone());
            self.require_supported_operation(&request)?;
            // Open has no checkpoint import/export or CloseAgent link cleanup.
            // Keep the exact receiver guard used for this control in the link.
            let (response, ownership) = self.control_with_receiver(endpoint, &request).await?;
            let receipt = match response {
                ResponseFrame::OpenAgent(receipt) => receipt,
                ResponseFrame::Failure(failure) => {
                    return Err(super::super::provider_http::runtime_failure(failure));
                }
                _ => unreachable!("control correlation verifies open response operation"),
            };
            timing.completed("runtime_open");
            Ok((receipt, ownership))
        };
        let (RuntimeDiscovery::AgentSandbox(client), Some((runtime, binding))) =
            (&self.client, endpoint.native_binding())
        else {
            let (receipt, ownership) = opening.await?;
            return Ok((
                receipt,
                self.connect(endpoint, &launch.request.agent_id).await?,
                ownership,
            ));
        };

        let (authorize, authorized) = tokio::sync::oneshot::channel();
        let opening = async {
            let receipt = opening.await?;
            let _ = authorize.send(());
            Ok::<_, AdapterError>(receipt)
        };
        let preparation = async {
            // A preparation error must not cancel an in-flight Open whose result
            // could be ambiguous. Conversely, an Open failure cancels preparation
            // and drops its stream/permit without sending any application bytes.
            Ok::<_, AdapterError>(
                client
                    .runtime_stream_after(
                        runtime,
                        binding,
                        crate::agent_sandbox::RuntimeStreamKind::Conversation,
                        self.native_transport,
                        async {
                            authorized.await.map_err(|_| {
                                AdapterError::Unavailable(
                                    "runtime Open did not authorize the conversation".into(),
                                )
                            })
                        },
                    )
                    .await,
            )
        };
        let ((receipt, ownership), prepared) = tokio::try_join!(opening, preparation)?;
        let socket = self
            .connect_with_prepared_stream(endpoint, &launch.request.agent_id, Some(prepared?))
            .await?;
        Ok((receipt, socket, ownership))
    }
}
