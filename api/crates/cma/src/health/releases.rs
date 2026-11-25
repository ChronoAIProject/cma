use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ring::hmac;
use serde::Serialize;
use uuid::Uuid;

use crate::store::{released_runtimes::StoreReleasedRuntime, MongoStores};

pub(super) const CHALLENGE: &str = "x-cma-runtime-proof";
const SIGNATURE: &str = "x-cma-runtime-proof-signature";
const RESPONSE_CONTEXT: &[u8] = b"cma-runtime-proof-response-v1\n";

pub(crate) struct Proof {
    key: hmac::Key,
    source: Arc<StoreReleasedRuntime>,
    environment: String,
    build_digest: String,
    source_sha: &'static str,
    pod_uid: Uuid,
    incarnation: Uuid,
}

impl Proof {
    pub(super) fn configured(stores: &MongoStores) -> Result<Option<Self>, String> {
        let Ok(key) = std::env::var("CHRONO_RUNTIME_POINTER_PROOF_KEY") else {
            return Ok(None);
        };
        let key = URL_SAFE_NO_PAD
            .decode(key)
            .ok()
            .filter(|key| key.len() == 32)
            .ok_or("invalid runtime pointer proof key")?;
        let environment = std::env::var("CHRONO_ENVIRONMENT")
            .ok()
            .filter(|value| matches!(value.as_str(), "staging" | "production"))
            .ok_or("runtime pointer proof requires the deployment environment")?;
        let build_digest = std::env::var("CHRONO_AM_BUILD_DIGEST")
            .ok()
            .filter(|digest| crate::runtime_pointer::valid_digest(digest))
            .ok_or("runtime pointer proof requires the Agent Manager build digest")?;
        let source_sha = option_env!("CHRONO_BUILD_SHA")
            .filter(|source| lower_hex(source, 40))
            .ok_or("runtime pointer proof requires a compiled source revision")?;
        let pod_uid = std::env::var("CHRONO_AM_POD_UID")
            .ok()
            .and_then(|uid| Uuid::parse_str(&uid).ok())
            .ok_or("runtime pointer proof requires the Agent Manager Pod UID")?;
        let incarnation = stores
            .sandbox_model
            .agent_manager_instance()
            .and_then(|id| Uuid::parse_str(id).ok())
            .ok_or("runtime pointer proof requires the Agent Manager incarnation")?;
        let source = stores
            .runtime_pointer
            .clone()
            .ok_or("runtime pointer proof requires durable release authority")?;
        Ok(Some(Self {
            key: hmac::Key::new(hmac::HMAC_SHA256, &key),
            source,
            environment,
            build_digest,
            source_sha,
            pod_uid,
            incarnation,
        }))
    }

    pub(super) async fn observe(&self, headers: &HeaderMap, ready: bool) -> Response {
        let Some(challenge) = authenticate(headers, &self.key, &self.environment) else {
            return StatusCode::UNAUTHORIZED.into_response();
        };
        if !ready {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        match tokio::time::timeout(Duration::from_secs(2), self.read(challenge)).await {
            Ok(Some(observation)) => {
                let body = serde_json::to_vec(&observation).expect("serialize runtime proof");
                let mut mac = hmac::Context::with_key(&self.key);
                mac.update(RESPONSE_CONTEXT);
                mac.update(&body);
                let tag = URL_SAFE_NO_PAD.encode(mac.sign().as_ref());
                (
                    [
                        (header::CACHE_CONTROL.as_str(), "no-store"),
                        (header::CONTENT_TYPE.as_str(), "application/json"),
                        (SIGNATURE, tag.as_str()),
                    ],
                    body,
                )
                    .into_response()
            }
            _ => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        }
    }

    async fn read<'a>(&'a self, challenge: &'a str) -> Option<Observation<'a>> {
        let (agent, machine) = self.source.lanes.observation().await.ok()?;
        let agent = agent.active;
        Some(Observation {
            challenge,
            environment: &self.environment,
            build_digest: &self.build_digest,
            source_sha: self.source_sha,
            pod_uid: self.pod_uid,
            incarnation: self.incarnation,
            agent: Lane::new(agent.image, agent.revision, false)?,
            machine: machine
                .and_then(|record| Lane::new(record.active.image, record.active.revision, true)),
        })
    }
}

fn lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn single<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let values = headers.get_all(name);
    let mut values = values.iter();
    let first = values.next()?.to_str().ok()?;
    values.next().is_none().then_some(first)
}

fn authenticate<'a>(headers: &'a HeaderMap, key: &hmac::Key, environment: &str) -> Option<&'a str> {
    let challenge = single(headers, CHALLENGE)?;
    if challenge.len() > 224 {
        return None;
    }
    let mut fields = challenge.split('.');
    if fields.next()? != "v1"
        || fields.next()? != environment
        || !lower_hex(fields.next()?, 64)
        || fields.next()?.parse::<u64>().ok()? == 0
        || fields.next()?.parse::<u64>().ok()? == 0
        || !matches!(fields.next()?, "preflight" | "promotion")
        || !lower_hex(fields.next()?, 64)
    {
        return None;
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let expires = fields.next()?.parse::<u64>().ok()?;
    if expires <= now || expires - now > 30 || fields.next().is_some() {
        return None;
    }
    let tag = single(headers, SIGNATURE)?;
    if tag.len() != 43 {
        return None;
    }
    let tag = URL_SAFE_NO_PAD.decode(tag).ok()?;
    hmac::verify(key, challenge.as_bytes(), &tag).ok()?;
    Some(challenge)
}

#[derive(Serialize)]
struct Observation<'a> {
    challenge: &'a str,
    environment: &'a str,
    build_digest: &'a str,
    source_sha: &'a str,
    pod_uid: Uuid,
    incarnation: Uuid,
    agent: Lane,
    machine: Option<Lane>,
}

#[derive(Serialize)]
struct Lane {
    digest: String,
    revision: u64,
}

impl Lane {
    fn new(image: String, revision: u64, machine: bool) -> Option<Self> {
        let expected = if machine {
            "ghcr.io/chronoaiproject/cma-runtime-omarchy"
        } else {
            "ghcr.io/chronoaiproject/cma-runtime"
        };
        let (repository, digest) = image.split_once('@')?;
        (repository == expected && revision > 0 && crate::runtime_pointer::valid_digest(digest))
            .then(|| Self {
                digest: digest.to_owned(),
                revision,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_signature_binds_environment_receipt_and_fresh_nonce() {
        let key = hmac::Key::new(hmac::HMAC_SHA256, &[7; 32]);
        let expires = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 20;
        let challenge = format!(
            "v1.staging.{}.4.9.preflight.{}.{expires}",
            "a".repeat(64),
            "b".repeat(64)
        );
        let tag = URL_SAFE_NO_PAD.encode(hmac::sign(&key, challenge.as_bytes()).as_ref());
        let mut headers = HeaderMap::new();
        headers.insert(CHALLENGE, challenge.parse().unwrap());
        headers.insert(SIGNATURE, tag.parse().unwrap());
        assert_eq!(
            authenticate(&headers, &key, "staging"),
            Some(challenge.as_str())
        );
        assert_eq!(authenticate(&headers, &key, "production"), None);
        headers.insert(
            CHALLENGE,
            challenge.replace(".4.9.", ".4.10.").parse().unwrap(),
        );
        assert_eq!(authenticate(&headers, &key, "staging"), None);
        headers.insert(CHALLENGE, challenge.parse().unwrap());
        headers.append(CHALLENGE, challenge.parse().unwrap());
        assert_eq!(authenticate(&headers, &key, "staging"), None);
    }
}
