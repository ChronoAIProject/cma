//! Overlap only unauthenticated stream preparation with the actual fenced Open.
use super::*;

impl ProviderHostedControlTransport {
    pub(super) async fn open_and_connect(
        &self,
        handle: &RuntimeHandle,
        endpoint: &RuntimeEndpoint,
        launch: &FencedRequest<AgentLaunch>,
        ownership: Option<&ownership::ReceiverLease>,
        timing: &mut super::super::startup_timing::StartupTiming,
        class: RuntimeWorkloadClass,
    ) -> crate::Result<(FencedReceipt<AgentOpenReceipt>, Socket)> {
        let opening = async {
            let response = match ownership {
                Some(lease) => self.open_on_receiver_lease(endpoint, launch, lease).await?,
                None => {
                    self.exchange_at(handle, endpoint, RequestFrame::OpenAgent(launch.clone()))
                        .await?
                }
            };
            let receipt = match response {
                ResponseFrame::OpenAgent(receipt) => receipt,
                ResponseFrame::Failure(failure) => {
                    return Err(super::super::provider_http::runtime_failure(failure));
                }
                _ => unreachable!("control correlation verifies open response operation"),
            };
            timing.completed("runtime_open");
            Ok(receipt)
        };
        let (RuntimeDiscovery::AgentSandbox(client), Some((runtime, binding))) =
            (&self.client, endpoint.native_binding())
        else {
            let receipt = class.scope(opening).await?;
            return Ok((
                receipt,
                self.connect(endpoint, &launch.request.agent_id).await?,
            ));
        };

        let (authorize, authorized) = tokio::sync::oneshot::channel();
        let opening = async {
            let receipt = class.scope(opening).await?;
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
                        if class == RuntimeWorkloadClass::Continuity {
                            RuntimeStreamKind::ContinuityConversation
                        } else {
                            RuntimeStreamKind::Conversation
                        },
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
        let (receipt, prepared) = tokio::try_join!(opening, preparation)?;
        let socket = self
            .connect_with_prepared_stream(endpoint, &launch.request.agent_id, Some(prepared?))
            .await?;
        Ok((receipt, socket))
    }
}
