use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ring::hmac;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Mutex, time::SystemTime};

use super::{valid_text, PATH};

pub(super) const PROOF_HEADER: &str = "x-cma-binding-observation";
const MAX_PROOF_BYTES: usize = 4096;
const MAX_NONCES: usize = 1024;
const LIFETIME_MS: u64 = 5_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Proof {
    pub sender: String,
    pub recipient: String,
    pub principal: String,
    nonce: String,
    issued_at: u64,
    expires_at: u64,
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

    pub fn request(
        &self,
        sender: &str,
        recipient: &str,
        principal: &str,
    ) -> Result<(Proof, String), ()> {
        let issued_at = now_ms()?;
        let proof = Proof {
            sender: sender.into(),
            recipient: recipient.into(),
            principal: principal.into(),
            nonce: uuid::Uuid::new_v4().to_string(),
            issued_at,
            expires_at: issued_at.checked_add(LIFETIME_MS).ok_or(())?,
        };
        proof.validate(issued_at)?;
        let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&proof).map_err(|_| ())?);
        let signature = hmac::sign(&self.key, &message(b"request", encoded.as_bytes(), &[]));
        Ok((
            proof,
            format!("{encoded}.{}", URL_SAFE_NO_PAD.encode(signature.as_ref())),
        ))
    }

    pub fn verify_request(&self, raw: &str, recipient: &str) -> Result<Proof, ()> {
        self.verify_request_at(raw, recipient, now_ms()?)
    }

    fn verify_request_at(&self, raw: &str, recipient: &str, now: u64) -> Result<Proof, ()> {
        if raw.len() > MAX_PROOF_BYTES {
            return Err(());
        }
        let (encoded, signature) = raw.split_once('.').ok_or(())?;
        let signature = URL_SAFE_NO_PAD.decode(signature).map_err(|_| ())?;
        hmac::verify(
            &self.key,
            &message(b"request", encoded.as_bytes(), &[]),
            &signature,
        )
        .map_err(|_| ())?;
        let proof: Proof =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(encoded).map_err(|_| ())?)
                .map_err(|_| ())?;
        proof.validate(now)?;
        if proof.recipient != recipient {
            return Err(());
        }
        let mut nonces = self.nonces.lock().map_err(|_| ())?;
        nonces.retain(|_, until| *until > now);
        if nonces.len() >= MAX_NONCES || nonces.contains_key(&proof.nonce) {
            return Err(());
        }
        nonces.insert(proof.nonce.clone(), proof.expires_at);
        Ok(proof)
    }

    pub fn response(&self, proof: &Proof, body: &[u8]) -> Result<String, ()> {
        proof.validate(now_ms()?)?;
        let encoded = serde_json::to_vec(proof).map_err(|_| ())?;
        Ok(URL_SAFE_NO_PAD
            .encode(hmac::sign(&self.key, &message(b"response", &encoded, body)).as_ref()))
    }

    pub fn verify_response(&self, proof: &Proof, body: &[u8], raw: &str) -> Result<(), ()> {
        proof.validate(now_ms()?)?;
        if raw.len() > 128 {
            return Err(());
        }
        let signature = URL_SAFE_NO_PAD.decode(raw).map_err(|_| ())?;
        let encoded = serde_json::to_vec(proof).map_err(|_| ())?;
        hmac::verify(&self.key, &message(b"response", &encoded, body), &signature).map_err(|_| ())
    }
}

impl Proof {
    fn validate(&self, now: u64) -> Result<(), ()> {
        if !valid_text(&self.sender, 128)
            || !valid_text(&self.recipient, 128)
            || !valid_text(&self.principal, 512)
            || uuid::Uuid::parse_str(&self.nonce).is_err()
            || self.issued_at > now
            || self.expires_at <= now
            || self.expires_at.checked_sub(self.issued_at) != Some(LIFETIME_MS)
        {
            return Err(());
        }
        Ok(())
    }
}

fn message(domain: &[u8], proof: &[u8], body: &[u8]) -> Vec<u8> {
    let mut bytes = b"chrono.binding-diagnostics.v1\0".to_vec();
    bytes.extend_from_slice(domain);
    bytes.extend_from_slice(b"\0GET\0");
    bytes.extend_from_slice(PATH.as_bytes());
    bytes.extend_from_slice(b"\0");
    bytes.extend_from_slice(proof);
    bytes.extend_from_slice(b"\0");
    bytes.extend_from_slice(ring::digest::digest(&ring::digest::SHA256, body).as_ref());
    bytes
}

pub(super) fn now_ms() -> Result<u64, ()> {
    u64::try_from(
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|_| ())?
            .as_millis(),
    )
    .map_err(|_| ())
}

#[cfg(test)]
#[path = "tests_authentication.rs"]
mod tests;
