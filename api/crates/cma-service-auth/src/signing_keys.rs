//! Endpoint-keyed cache of parsed signing keys with one independent refresh per endpoint.
//! Each caller supplies endpoint validation, a key-set parser, TTL and error vocabulary.
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::watch, time::Instant};

pub trait KeySet: Send + Sync + 'static {
    fn contains(&self, kid: &str) -> bool;
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub ttl: Duration,
    /// Keep this longer than the fetch deadline to coalesce a burst into one fetch.
    /// Joiners may retry when a refresh outlasts the cooldown.
    pub cooldown: Duration,
    pub max_endpoints: usize,
    pub max_body_bytes: usize,
}
impl Config {
    pub fn new(ttl: Duration, max_endpoints: usize) -> Self {
        Self {
            ttl,
            cooldown: Duration::from_secs(5),
            max_endpoints,
            max_body_bytes: 32_768,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportFailure {
    Request,
    Status,
    Bounds,
    Body,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome<E> {
    UnknownKey,
    Throttled,
    Refresh(E),
    FlightLost,
    Capacity,
}

pub type FetchFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<u8>, TransportFailure>> + Send + 'a>>;

/// Fetch bytes within the supplied bound and a finite deadline.
/// The built-in HTTP implementation also
/// disables redirects and enforces one-second connect and three-second total timeouts.
pub trait Fetcher: Send + Sync + 'static {
    fn fetch<'a>(&'a self, url: &'a str, max_body_bytes: usize) -> FetchFuture<'a>;
}

struct HttpFetcher(reqwest::Client);
impl Fetcher for HttpFetcher {
    fn fetch<'a>(&'a self, url: &'a str, max_body_bytes: usize) -> FetchFuture<'a> {
        Box::pin(async move {
            let mut response = self
                .0
                .get(url)
                .send()
                .await
                .map_err(|_| TransportFailure::Request)?;
            if !response.status().is_success() {
                return Err(TransportFailure::Status);
            }
            if response
                .content_length()
                .is_some_and(|size| size > max_body_bytes as u64)
            {
                return Err(TransportFailure::Bounds);
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| TransportFailure::Body)? {
                if bytes.len().saturating_add(chunk.len()) > max_body_bytes {
                    return Err(TransportFailure::Bounds);
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(bytes)
        })
    }
}

type Refresh<K, E> = watch::Receiver<Option<Result<Arc<K>, E>>>;
type Entry<K, E> = Arc<Mutex<Cached<K, E>>>;
struct Cached<K, E> {
    keys: Option<(Arc<K>, Instant)>,
    next_refresh: Option<Instant>,
    flight: Option<Refresh<K, E>>,
}
impl<K, E> Default for Cached<K, E> {
    fn default() -> Self {
        Self {
            keys: None,
            next_refresh: None,
            flight: None,
        }
    }
}

pub struct Cache<K, E> {
    config: Config,
    fetcher: Arc<dyn Fetcher>,
    parse: fn(&[u8]) -> Result<K, E>,
    transport_error: fn(TransportFailure) -> E,
    entries: Mutex<HashMap<String, Entry<K, E>>>,
}
impl<K: KeySet, E: Clone + Send + Sync + 'static> Cache<K, E> {
    pub fn new(
        config: Config,
        parse: fn(&[u8]) -> Result<K, E>,
        transport_error: fn(TransportFailure) -> E,
    ) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(1))
            .timeout(Duration::from_secs(3))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self::with_fetcher(
            config,
            parse,
            transport_error,
            Arc::new(HttpFetcher(client)),
        ))
    }

    pub fn with_fetcher(
        config: Config,
        parse: fn(&[u8]) -> Result<K, E>,
        transport_error: fn(TransportFailure) -> E,
        fetcher: Arc<dyn Fetcher>,
    ) -> Self {
        Self {
            config,
            fetcher,
            parse,
            transport_error,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Look up a key at a caller-validated URL within a Tokio runtime.
    pub async fn keys_for(&self, url: &str, kid: &str) -> Result<Arc<K>, Outcome<E>> {
        let entry = self.entry(url)?;
        loop {
            let (mut refresh, started) = {
                let mut cache = entry
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let now = Instant::now();
                let current = cache.keys.as_ref().filter(|(_, until)| *until > now);
                if let Some((keys, _)) = current {
                    if keys.contains(kid) {
                        return Ok(keys.clone());
                    }
                }
                if let Some(refresh) = &cache.flight {
                    (refresh.clone(), false)
                } else {
                    if cache.next_refresh.is_some_and(|after| after > now) {
                        return Err(if current.is_some() {
                            Outcome::UnknownKey
                        } else {
                            Outcome::Throttled
                        });
                    }
                    cache.next_refresh = Some(now + self.config.cooldown);
                    (self.start(url, &entry, &mut cache), true)
                }
            };
            let result = refresh
                .wait_for(|result| result.is_some())
                .await
                .map(|result| result.as_ref().unwrap().clone());
            let result = result.map_err(|_| {
                let mut cache = entry
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if cache
                    .flight
                    .as_ref()
                    .is_some_and(|flight| flight.same_channel(&refresh))
                {
                    cache.flight = None;
                }
                Outcome::FlightLost
            })?;
            if started {
                let keys = result.map_err(Outcome::Refresh)?;
                return if keys.contains(kid) {
                    Ok(keys)
                } else {
                    Err(Outcome::UnknownKey)
                };
            }
            // A joiner observes the current keys and cooldown, even after failure.
        }
    }

    fn start(&self, url: &str, entry: &Entry<K, E>, cache: &mut Cached<K, E>) -> Refresh<K, E> {
        let (sender, receiver) = watch::channel(None);
        cache.flight = Some(receiver.clone());
        let (entry, fetcher, url) = (entry.clone(), self.fetcher.clone(), url.to_owned());
        let (config, parse, transport_error) = (self.config, self.parse, self.transport_error);
        // The task owns the refresh; dropping any request cannot cancel it.
        tokio::spawn(async move {
            let result = async {
                let bytes = fetcher
                    .fetch(&url, config.max_body_bytes)
                    .await
                    .map_err(transport_error)?;
                if bytes.len() > config.max_body_bytes {
                    return Err(transport_error(TransportFailure::Bounds));
                }
                parse(&bytes).map(Arc::new)
            }
            .await;
            let mut cache = entry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Ok(keys) = &result {
                cache.keys = Some((keys.clone(), Instant::now() + config.ttl));
            }
            cache.flight = None;
            sender.send_replace(Some(result));
        });
        receiver
    }

    fn entry(&self, url: &str) -> Result<Entry<K, E>, Outcome<E>> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = entries.get(url) {
            return Ok(entry.clone());
        }
        if entries.len() >= self.config.max_endpoints {
            let now = Instant::now();
            entries.retain(|_, entry| {
                if Arc::strong_count(entry) > 1 {
                    return true;
                }
                let cache = entry
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                cache.flight.is_some()
                    || cache.next_refresh.is_some_and(|after| after > now)
                    || cache.keys.as_ref().is_some_and(|(_, until)| *until > now)
            });
            if entries.len() >= self.config.max_endpoints {
                return Err(Outcome::Capacity);
            }
        }
        let entry = Arc::new(Mutex::new(Cached::default()));
        entries.insert(url.to_owned(), entry.clone());
        Ok(entry)
    }
}

#[cfg(test)]
#[path = "signing_keys_tests.rs"]
mod tests;
