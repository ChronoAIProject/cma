//! Keys fetched only from the resolved channel's retained JWKS endpoint.
use super::{jwt::KeySet, VerificationError};
use crate::channels::resolver::ResolvedChannel;
use cma_service_auth::signing_keys::{self, Cache, Config, Outcome, TransportFailure};
use cmaeg_core::channel::Source;
use std::{sync::Arc, time::Duration};

pub struct JwksCache {
    cache: Cache<KeySet, VerificationError>,
}
impl signing_keys::KeySet for KeySet {
    fn contains(&self, kid: &str) -> bool {
        self.contains(kid)
    }
}
impl JwksCache {
    pub fn new(ttl: Duration) -> Result<Self, reqwest::Error> {
        Ok(Self {
            cache: Cache::new(Config::new(ttl, 4096), KeySet::parse, |failure| {
                VerificationError(match failure {
                    TransportFailure::Bounds => "jwks_bounds",
                    TransportFailure::Request
                    | TransportFailure::Status
                    | TransportFailure::Body => "jwks_unavailable",
                })
            })?,
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
        self.cache
            .keys_for(jwks_url, kid)
            .await
            .map_err(|outcome| match outcome {
                Outcome::UnknownKey => VerificationError("signing_key_unknown"),
                Outcome::Throttled | Outcome::FlightLost => VerificationError("jwks_unavailable"),
                Outcome::Refresh(error) => error,
                Outcome::Capacity => VerificationError("jwks_capacity"),
            })
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
