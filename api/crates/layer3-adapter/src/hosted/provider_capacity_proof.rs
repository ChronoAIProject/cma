use super::*;
use crate::agent_capacity_recovery::{
    CapacityRecoveryError as Error, CapacityRecoveryNativeProof, CapacityRecoveryRequest,
};
use futures_util::StreamExt as _;

const PROOF_BYTES: usize = 256 * 1024;

impl ProviderHostedControlTransport {
    pub(super) async fn capacity_recovery_proof(
        &self,
        endpoint: &RuntimeEndpoint,
        request: &CapacityRecoveryRequest,
    ) -> Result<CapacityRecoveryNativeProof, Error> {
        let exchange = async {
            let url = endpoint
                .capacity_recovery(&request.target.agent_id)
                .map_err(|_| Error::Unavailable)?;
            if endpoint.native_binding().is_some() {
                let body = serde_json::to_vec(request).map_err(|_| Error::IdentityConflict)?;
                let reply = self
                    .native_http(endpoint, http::Method::POST, url, body, PROOF_BYTES)
                    .await
                    .map_err(|_| Error::Unavailable)?;
                require_success(reply.status)?;
                let proof = decode(&reply.bytes, request)?;
                reply.recycle().await.map_err(|_| Error::Unavailable)?;
                return Ok(proof);
            }
            let (header, token) = self.auth_header(endpoint);
            let response = self
                .http
                .post(url)
                .header(header, token)
                .json(request)
                .send()
                .await
                .map_err(|_| Error::Unavailable)?;
            require_success(response.status())?;
            if response
                .content_length()
                .is_some_and(|length| length > PROOF_BYTES as u64)
            {
                return Err(Error::StateUnconfirmed);
            }
            let mut body = response.bytes_stream();
            let mut bytes = Vec::new();
            while let Some(chunk) = body.next().await {
                let chunk = chunk.map_err(|_| Error::Unavailable)?;
                if chunk.len() > PROOF_BYTES.saturating_sub(bytes.len()) {
                    return Err(Error::StateUnconfirmed);
                }
                bytes.extend_from_slice(&chunk);
            }
            decode(&bytes, request)
        };
        tokio::time::timeout(CONTROL_DEADLINE, exchange)
            .await
            .map_err(|_| Error::Unavailable)?
    }
}

fn require_success(status: http::StatusCode) -> Result<(), Error> {
    if status.is_success() {
        return Ok(());
    }
    Err(match status {
        http::StatusCode::NOT_FOUND | http::StatusCode::NOT_IMPLEMENTED => Error::Unsupported,
        http::StatusCode::CONFLICT | http::StatusCode::BAD_REQUEST => Error::IdentityConflict,
        _ => Error::Unavailable,
    })
}

fn decode(
    bytes: &[u8],
    request: &CapacityRecoveryRequest,
) -> Result<CapacityRecoveryNativeProof, Error> {
    let proof: CapacityRecoveryNativeProof =
        serde_json::from_slice(bytes).map_err(|_| Error::StateUnconfirmed)?;
    proof
        .confirmation
        .validate_for(request)
        .map_err(|_| Error::StateUnconfirmed)?;
    Ok(proof)
}
