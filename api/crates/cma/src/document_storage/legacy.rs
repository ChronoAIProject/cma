//! Preserve the deployed online policy until resilience is explicitly enabled.
use super::{
    bounded, DocumentError, Method, RequestFailure, Response, StatusCode, StorageDocuments,
};
use serde::Deserialize;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

pub(super) type SerializedToken = Mutex<Option<(Zeroizing<String>, Instant)>>;

impl StorageDocuments {
    async fn serialized_token(
        &self,
        rejected: Option<&str>,
    ) -> Result<Zeroizing<String>, DocumentError> {
        let mut guard = self.legacy_token.lock().await;
        if let Some((token, expires)) = guard.as_ref() {
            if *expires > Instant::now() + Duration::from_secs(60)
                && rejected != Some(token.as_str())
            {
                return Ok(token.clone());
            }
        }
        let response = self
            .http
            .post(format!("{}/oauth/token", self.config.platform_origin))
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", self.config.client_id.as_str()),
                ("client_secret", self.config.client_secret.as_str()),
                (
                    "scope",
                    if self.read_only {
                        "proxy object:read object:download"
                    } else {
                        "proxy object:create object:read object:download"
                    },
                ),
            ])
            .send()
            .await
            .map_err(|_| DocumentError::Unavailable)?;
        if !response.status().is_success() {
            return Err(DocumentError::Unavailable);
        }
        let bytes = bounded(response, 16 * 1024).await?;
        let auth: TokenReply =
            serde_json::from_slice(&bytes).map_err(|_| DocumentError::Unavailable)?;
        if auth.access_token.is_empty()
            || auth.access_token.len() > 16 * 1024
            || auth.expires_in < 120
            || auth.expires_in > 86_400
        {
            return Err(DocumentError::Unavailable);
        }
        let token = Zeroizing::new(auth.access_token);
        *guard = Some((
            token.clone(),
            Instant::now() + Duration::from_secs(auth.expires_in),
        ));
        Ok(token)
    }
    pub(super) async fn legacy_send(
        &self,
        method: Method,
        url: url::Url,
        body: Option<Vec<u8>>,
    ) -> Result<Response, RequestFailure> {
        let mut token = self.serialized_token(None).await?;
        for attempt in 0..2 {
            let mut request = self
                .http
                .request(method.clone(), url.clone())
                .bearer_auth(token.as_str());
            if let Some(body) = &body {
                request = request
                    .header("content-type", "text/markdown; charset=utf-8")
                    .body(body.clone());
            }
            let response = request
                .send()
                .await
                .map_err(|_| DocumentError::Unavailable)?;
            if response.status() == StatusCode::UNAUTHORIZED && attempt == 0 {
                token = self.serialized_token(Some(&token)).await?;
                continue;
            }
            if !response.status().is_success() {
                return Err(DocumentError::Unavailable.into());
            }
            return Ok(response);
        }
        Err(DocumentError::Unavailable.into())
    }
}
#[derive(Deserialize)]
struct TokenReply {
    access_token: String,
    expires_in: u64,
}
