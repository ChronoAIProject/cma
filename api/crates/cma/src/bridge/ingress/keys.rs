use std::{sync::Arc, time::Duration};

use futures_util::StreamExt;
use layer2_core::bridge::ingress::{CallbackError, CallbackKeySet};
use tokio::{sync::Mutex, time::Instant};

/// Deployment-pinned HTTPS JWKS. JWT jku/x5u and redirects cannot choose a URL.
/// Failed/expired refreshes never extend the previous key lease.
pub(crate) struct PinnedCallbackKeys {
    issuer: String,
    endpoint: String,
    client: reqwest::Client,
    cache: Mutex<Cache>,
}
#[derive(Default)]
struct Cache {
    value: Option<(Arc<CallbackKeySet>, Instant)>,
    next_refresh: Option<Instant>,
}

impl PinnedCallbackKeys {
    #[cfg(test)]
    pub(super) fn endpoint(&self) -> &str {
        &self.endpoint
    }

    #[cfg(test)]
    pub(crate) fn fixture() -> Self {
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut provider = Self::new(
            "https://nyx.example".into(),
            "https://nyx.example/jwks".into(),
        )
        .unwrap();
        let keys = CallbackKeySet::from_pinned_jwks(
            "https://nyx.example".into(),
            include_bytes!("../../../../layer2-core/src/bridge/ingress/fixtures/jwks.json"),
            deadline,
        )
        .unwrap();
        provider.cache = Mutex::new(Cache {
            value: Some((Arc::new(keys), deadline)),
            next_refresh: Some(deadline),
        });
        provider
    }

    pub(crate) fn new(issuer: String, endpoint: String) -> Result<Self, CallbackError> {
        for address in [&issuer, &endpoint] {
            let url = reqwest::Url::parse(address).map_err(|_| CallbackError::KeysUnavailable)?;
            if address.len() > 2048
                || url.scheme() != "https"
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.fragment().is_some()
                || url.query().is_some()
            {
                return Err(CallbackError::KeysUnavailable);
            }
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(1))
            .timeout(Duration::from_secs(2))
            .build()
            .map_err(|_| CallbackError::KeysUnavailable)?;
        Ok(Self {
            issuer,
            endpoint,
            client,
            cache: Mutex::new(Cache::default()),
        })
    }

    pub(crate) async fn get(&self, force: bool) -> Result<Arc<CallbackKeySet>, CallbackError> {
        let mut cache = self.cache.lock().await;
        let now = Instant::now();
        let available = cache
            .value
            .as_ref()
            .filter(|(_, deadline)| *deadline > now)
            .map(|(keys, _)| Arc::clone(keys));
        if !force && available.is_some() {
            return available.ok_or(CallbackError::KeysUnavailable);
        }
        if cache.next_refresh.is_some_and(|deadline| deadline > now) {
            return available.ok_or(CallbackError::KeysUnavailable);
        }
        cache.next_refresh = Some(now + Duration::from_secs(5));
        match self.fetch(now).await {
            Ok(value) => {
                let value = Arc::new(value);
                cache.value = Some((Arc::clone(&value), now + Duration::from_secs(60)));
                Ok(value)
            }
            Err(error) => {
                cache.value = None;
                Err(error)
            }
        }
    }

    async fn fetch(&self, started: Instant) -> Result<CallbackKeySet, CallbackError> {
        let response = self
            .client
            .get(&self.endpoint)
            .send()
            .await
            .map_err(|_| CallbackError::KeysUnavailable)?;
        if !response.status().is_success() || response.content_length().is_some_and(|n| n > 32768) {
            return Err(CallbackError::KeysUnavailable);
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| CallbackError::KeysUnavailable)?;
            if bytes.len() + chunk.len() > 32768 {
                return Err(CallbackError::KeysUnavailable);
            }
            bytes.extend_from_slice(&chunk);
        }
        CallbackKeySet::from_pinned_jwks(
            self.issuer.clone(),
            &bytes,
            started + Duration::from_secs(60),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn expired_cache_and_failed_refresh_never_extend_old_keys() {
        let keys = PinnedCallbackKeys::new(
            "https://nyx.example".into(),
            "https://127.0.0.1:1/jwks".into(),
        )
        .unwrap();
        let now = Instant::now();
        let keyset = CallbackKeySet::from_pinned_jwks(
            "https://nyx.example".into(),
            include_bytes!("../../../../layer2-core/src/bridge/ingress/fixtures/jwks.json"),
            now + Duration::from_secs(1),
        )
        .unwrap();
        *keys.cache.lock().await = Cache {
            value: Some((Arc::new(keyset), now + Duration::from_secs(1))),
            next_refresh: Some(now + Duration::from_secs(5)),
        };
        assert!(keys.get(false).await.is_ok());
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(matches!(
            keys.get(false).await,
            Err(CallbackError::KeysUnavailable)
        ));
        tokio::time::advance(Duration::from_secs(4)).await;
        assert!(matches!(
            keys.get(true).await,
            Err(CallbackError::KeysUnavailable)
        ));
        assert!(keys.cache.lock().await.value.is_none());
    }
    #[test]
    fn untrusted_and_credential_endpoints_are_rejected() {
        for endpoint in [
            "http://nyx.example/jwks",
            "https://user:secret@nyx.example/jwks",
            "https://nyx.example/jwks?token=secret",
        ] {
            assert!(
                PinnedCallbackKeys::new("https://nyx.example".into(), endpoint.into()).is_err()
            );
        }
    }
}
