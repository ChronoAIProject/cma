use super::*;

impl ProviderHostedControlTransport {
    pub(super) async fn control(
        &self,
        endpoint: &RuntimeEndpoint,
        request: &RequestFrame,
    ) -> crate::Result<ResponseFrame> {
        let _ownership = self.receiver_lease(endpoint).await?;
        let url = endpoint.control()?;
        let auth = self.auth_header(endpoint);
        let exchange = async {
            if endpoint.native_binding().is_some() {
                let body = serde_json::to_vec(request).map_err(|_| {
                    AdapterError::Rejected("runtime request could not be encoded".into())
                })?;
                let reply = self
                    .native_http(
                        endpoint,
                        http::Method::POST,
                        url,
                        body,
                        super::super::provider_http::CONTROL_RESPONSE_BYTES,
                    )
                    .await?;
                return self.complete_native_control(endpoint, request, reply).await;
            }
            let mut request_builder = self
                .http
                .post(url)
                .header(auth.0, auth.1)
                .header(
                    reqwest::header::ACCEPT,
                    format!(
                        "{}, {}",
                        crate::wire::AGENT_RESOURCES_STATUS_MEDIA_TYPE,
                        crate::wire::AGENT_COMPUTER_STATUS_MEDIA_TYPE,
                    ),
                )
                .json(request);
            if let Some(owner) = crate::owner_protocol::current_header()? {
                request_builder =
                    request_builder.header(crate::owner_protocol::OWNER_HEADER, owner);
            }
            let response = request_builder.send().await.map_err(|error| {
                AdapterError::Unavailable(format!("runtime control request failed: {error}"))
            })?;
            super::super::provider_http::decode_control(response).await
        };
        let response = tokio::time::timeout(CONTROL_DEADLINE, exchange)
            .await
            .map_err(|_| AdapterError::Unavailable("runtime control request timed out".into()))??;
        control_correlation::validate(request, &response)?;
        if let Some(authority) = crate::owner_protocol::current_authority() {
            authority.check().await?;
        }
        Ok(response)
    }

    pub(super) async fn status_on_native_lease(
        &self,
        endpoint: &RuntimeEndpoint,
        status: &crate::RuntimeStatusRequest,
        lease: native_pool::NativeLease,
    ) -> crate::Result<ResponseFrame> {
        let request = RequestFrame::RuntimeStatus(status.clone());
        let body = serde_json::to_vec(&request)
            .map_err(|_| AdapterError::Rejected("runtime request could not be encoded".into()))?;
        let reply = self
            .native_http_once(
                endpoint,
                http::Method::POST,
                endpoint.control()?,
                body,
                super::super::provider_http::CONTROL_RESPONSE_BYTES,
                lease,
            )
            .await?;
        self.complete_native_control(endpoint, &request, reply)
            .await
    }

    async fn complete_native_control(
        &self,
        endpoint: &RuntimeEndpoint,
        request: &RequestFrame,
        reply: native_transport::NativeHttpResponse,
    ) -> crate::Result<ResponseFrame> {
        let mut response =
            super::super::provider_http::decode_control_bytes(reply.status, &reply.bytes)?;
        let (_, token) = self.auth_header(endpoint);
        if let ResponseFrame::Failure(failure) = &mut response {
            if !token.is_empty() {
                failure.code = failure.code.replace(token, "[redacted]");
                failure.detail = failure.detail.replace(token, "[redacted]");
            }
        }
        control_correlation::validate(request, &response)?;
        reply.recycle().await?;
        Ok(response)
    }
}
