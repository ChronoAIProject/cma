use super::*;
use axum::http::{HeaderMap, Method, Uri};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ring::hmac;
use std::{
    collections::BTreeMap,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Proof {
    pub sender: String,
    pub owner: OwnershipToken,
    nonce: String,
    until: u64,
}

pub(super) struct Authentication {
    key: hmac::Key,
    nonces: Mutex<BTreeMap<String, u64>>,
}

impl Authentication {
    pub fn new(key: hmac::Key) -> Self {
        Self {
            key,
            nonces: Mutex::default(),
        }
    }

    pub fn sign(
        &self,
        sender: &str,
        owner: &OwnershipToken,
        method: &Method,
        uri: &Uri,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<String, ()> {
        let proof = Proof {
            sender: sender.into(),
            owner: owner.clone(),
            nonce: uuid::Uuid::new_v4().to_string(),
            until: now()? + 10,
        };
        let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&proof).map_err(|_| ())?);
        let signature = hmac::sign(&self.key, &message(&encoded, method, uri, headers, body)?);
        Ok(format!(
            "{encoded}.{}",
            URL_SAFE_NO_PAD.encode(signature.as_ref())
        ))
    }

    pub fn verify(
        &self,
        raw: &str,
        method: &Method,
        uri: &Uri,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<Proof, ()> {
        if raw.len() > 4096 {
            return Err(());
        }
        let (encoded, signature) = raw.split_once('.').ok_or(())?;
        let signature = URL_SAFE_NO_PAD.decode(signature).map_err(|_| ())?;
        hmac::verify(
            &self.key,
            &message(encoded, method, uri, headers, body)?,
            &signature,
        )
        .map_err(|_| ())?;
        let proof: Proof =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(encoded).map_err(|_| ())?)
                .map_err(|_| ())?;
        let now = now()?;
        if proof.until <= now || proof.until > now + 10 {
            return Err(());
        }
        let mut nonces = self.nonces.lock().unwrap();
        nonces.retain(|_, until| *until > now);
        if nonces.len() >= 8192 || nonces.contains_key(&proof.nonce) {
            return Err(());
        }
        nonces.insert(proof.nonce.clone(), proof.until);
        Ok(proof)
    }
}

fn now() -> Result<u64, ()> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ())?
        .as_secs())
}

// These are precisely the forwarded headers. Transport-generated framing and Host
// are excluded; caller credentials, resource proofs and replay IDs are signed.
pub(super) fn forwarded(name: &str) -> bool {
    matches!(
        name,
        "authorization"
            | "cookie"
            | "content-type"
            | "accept"
            | "origin"
            | "last-event-id"
            | "idempotency-key"
            | "x-request-id"
            | "prefer"
            | "x-cma-browser-client"
            | "x-cma-document-share-token"
            | "x-nyxid-resource-request"
            | "x-nyxid-identity-token"
    ) || name.starts_with("x-cma-") && name != PROOF
}

fn message(
    encoded: &str,
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Vec<u8>, ()> {
    let mut signed = BTreeMap::<String, Vec<String>>::new();
    for name in headers.keys().filter(|name| forwarded(name.as_str())) {
        // Reqwest supplies this semantically empty default when Accept is absent.
        if name == axum::http::header::ACCEPT
            && headers.get_all(name).iter().count() == 1
            && headers.get(name).is_some_and(|value| value == "*/*")
        {
            continue;
        }
        let values = headers
            .get_all(name)
            .iter()
            .map(|value| URL_SAFE_NO_PAD.encode(value.as_bytes()))
            .collect();
        signed.insert(name.to_string(), values);
    }
    serde_json::to_vec(&(
        encoded,
        method.as_str(),
        uri.path_and_query().map(|value| value.as_str()),
        signed,
        URL_SAFE_NO_PAD.encode(ring::digest::digest(&ring::digest::SHA256, body).as_ref()),
    ))
    .map_err(|_| ())
}
