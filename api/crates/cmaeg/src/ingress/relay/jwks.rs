//! Keys fetched only from the resolved channel's retained JWKS endpoint.
use super::{jwt::KeySet, VerificationError};
use crate::channels::resolver::ResolvedChannel;
use cmaeg_core::channel::Source;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex as EntryMutex},
    time::Duration,
};
use tokio::{sync::Mutex, time::Instant};

pub struct JwksCache {
    client: reqwest::Client,
    ttl: Duration,
    entries: EntryMutex<HashMap<String, Arc<Mutex<Cached>>>>,
}
#[derive(Default)]
struct Cached {
    keys: Option<(Arc<KeySet>, Instant)>,
    next_refresh: Option<Instant>,
}
impl JwksCache {
    pub fn new(ttl: Duration) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(1))
                .timeout(Duration::from_secs(3))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            ttl,
            entries: EntryMutex::new(HashMap::new()),
        })
    }
    pub(super) async fn keys_for(
        &self,
        channel: &ResolvedChannel,
        source_index: usize,
        kid: &str,
    ) -> Result<Arc<KeySet>, VerificationError> {
        let Some(Source::NyxidRelay { jwks_url, .. }) =
            channel.definition.sources.get(source_index)
        else {
            return Err(VerificationError("relay_source_missing"));
        };
        validate_url(jwks_url)?;
        let entry = self.entry(jwks_url)?;
        let mut cache = entry.lock().await;
        let now = Instant::now();
        let current = cache
            .keys
            .as_ref()
            .filter(|(_, expires)| *expires > now)
            .map(|(keys, _)| keys.clone());
        if let Some(keys) = &current {
            if keys.contains(kid) {
                return Ok(keys.clone());
            }
        }
        if cache.next_refresh.is_some_and(|after| after > now) {
            return Err(VerificationError(if current.is_some() {
                "signing_key_unknown"
            } else {
                "jwks_unavailable"
            }));
        }
        cache.next_refresh = Some(now + Duration::from_secs(5));
        // The mutex coalesces concurrent misses and unknown-key refresh requests.
        let keys = match self.fetch(jwks_url).await {
            Ok(keys) => Arc::new(keys),
            Err(error) => {
                cache.keys = None;
                return Err(error);
            }
        };
        cache.keys = Some((keys.clone(), Instant::now() + self.ttl));
        if !keys.contains(kid) {
            return Err(VerificationError("signing_key_unknown"));
        }
        Ok(keys)
    }
    fn entry(&self, url: &str) -> Result<Arc<Mutex<Cached>>, VerificationError> {
        let mut entries = self.entries.lock().unwrap();
        if let Some(entry) = entries.get(url) {
            return Ok(entry.clone());
        }
        // Retain active flights and refresh throttles; bound memory even when many
        // legitimate channel definitions point at different issuers over time.
        if entries.len() >= 4096 {
            let now = Instant::now();
            entries.retain(|_, entry| {
                if Arc::strong_count(entry) != 1 {
                    return true;
                }
                let Ok(cache) = entry.try_lock() else {
                    return true;
                };
                cache.next_refresh.is_some_and(|after| after > now)
                    || cache.keys.as_ref().is_some_and(|(_, until)| *until > now)
            });
            if entries.len() >= 4096 {
                return Err(VerificationError("jwks_capacity"));
            }
        }
        let entry = Arc::new(Mutex::new(Cached::default()));
        entries.insert(url.to_owned(), entry.clone());
        Ok(entry)
    }
    async fn fetch(&self, url: &str) -> Result<KeySet, VerificationError> {
        let mut response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|_| VerificationError("jwks_unavailable"))?;
        if !response.status().is_success() {
            return Err(VerificationError("jwks_unavailable"));
        }
        if response.content_length().is_some_and(|size| size > 32_768) {
            return Err(VerificationError("jwks_bounds"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| VerificationError("jwks_unavailable"))?
        {
            if bytes.len() + chunk.len() > 32_768 {
                return Err(VerificationError("jwks_bounds"));
            }
            bytes.extend_from_slice(&chunk);
        }
        KeySet::parse(&bytes)
    }
}
fn validate_url(raw: &str) -> Result<(), VerificationError> {
    let url = reqwest::Url::parse(raw).map_err(|_| VerificationError("jwks_url"))?;
    let loopback = url.host().is_some_and(|host| match host {
        url::Host::Domain(host) => host == "localhost",
        url::Host::Ipv4(ip) => ip.is_loopback(),
        url::Host::Ipv6(ip) => ip.is_loopback(),
    });
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
    {
        return Err(VerificationError("jwks_url"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capacity_preserves_live_flights_and_refresh_throttles() {
        let cache = JwksCache::new(Duration::from_secs(600)).unwrap();
        let entries: Vec<_> = (0..4096)
            .map(|i| {
                cache
                    .entry(&format!("https://issuer-{i}.example/jwks"))
                    .unwrap()
            })
            .collect();
        assert!(cache.entry("https://new.example/jwks").is_err());
        assert_eq!(cache.entries.lock().unwrap().len(), 4096);
        for entry in &entries {
            entry.try_lock().unwrap().next_refresh = Some(Instant::now() + Duration::from_secs(5));
        }
        drop(entries);
        assert!(cache.entry("https://new.example/jwks").is_err());
        for entry in cache.entries.lock().unwrap().values() {
            entry.try_lock().unwrap().next_refresh = Some(Instant::now() - Duration::from_secs(1));
        }
        assert!(cache.entry("https://new.example/jwks").is_ok());
        assert_eq!(cache.entries.lock().unwrap().len(), 1);
    }
}
