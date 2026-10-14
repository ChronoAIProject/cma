use super::*;

impl ProviderHostedControlTransport {
    pub(super) async fn control(
        &self,
        endpoint: &RuntimeEndpoint,
        request: &RequestFrame,
    ) -> crate::Result<ResponseFrame> {
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
                let mut response =
                    super::super::provider_http::decode_control_bytes(reply.status, &reply.bytes)?;
                if let ResponseFrame::Failure(failure) = &mut response {
                    if !auth.1.is_empty() {
                        failure.code = failure.code.replace(auth.1, "[redacted]");
                        failure.detail = failure.detail.replace(auth.1, "[redacted]");
                    }
                }
                control_correlation::validate(request, &response)?;
                reply.recycle().await?;
                return Ok(response);
            }
            let response = self
                .http
                .post(url)
                .header(auth.0, auth.1)
                .json(request)
                .send()
                .await
                .map_err(|error| {
                    AdapterError::Unavailable(format!("runtime control request failed: {error}"))
                })?;
            super::super::provider_http::decode_control(response).await
        };
        let response = tokio::time::timeout(CONTROL_DEADLINE, exchange)
            .await
            .map_err(|_| AdapterError::Unavailable("runtime control request timed out".into()))??;
        control_correlation::validate(request, &response)?;
        Ok(response)
    }
}
