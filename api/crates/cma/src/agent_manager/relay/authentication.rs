use super::*;
use axum::http::{HeaderMap, HeaderValue, Method, Uri};
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

const REQUEST_ID: &str = "x-request-id";
// Peers already accept opaque, signed nonces. A prefix negotiates the reply
// without adding a proof field or trusting a header from the public request.
const FILE_HANDOFF_NONCE: &str = "file-handoff:";
const FILE_WRITE_HANDOFF_NONCE: &str = "file-write-handoff:";
const WRITE_NOT_ENTERED: &[u8] = b"cma-file-write-not-entered-v1\0";

pub(super) struct WriteNotEntered {
    key: hmac::Key,
    message: Vec<u8>,
}

impl WriteNotEntered {
    pub(super) fn verify(&self, headers: &HeaderMap) -> bool {
        let mut values = headers.get_all(file_write::NOT_ENTERED_HEADER).iter();
        let Some(tag) = values.next().filter(|_| values.next().is_none()) else {
            return false;
        };
        URL_SAFE_NO_PAD
            .decode(tag.as_bytes())
            .is_ok_and(|tag| hmac::verify(&self.key, &self.message, &tag).is_ok())
    }
}

impl Proof {
    pub(super) fn reports_file_handoff(&self) -> bool {
        self.nonce.starts_with(FILE_HANDOFF_NONCE)
    }

    pub(super) fn reports_file_write_handoff(&self) -> bool {
        self.nonce.starts_with(FILE_WRITE_HANDOFF_NONCE)
    }
}

pub(super) struct Authentication {
    key: hmac::Key,
    nonces: Mutex<BTreeMap<String, u64>>,
    file_write_handoff: std::sync::OnceLock<()>,
}

impl Authentication {
    pub fn new(key: hmac::Key) -> Self {
        Self {
            key,
            nonces: Mutex::default(),
            file_write_handoff: Default::default(),
        }
    }

    pub(super) fn enable_file_write_handoff(&self) {
        self.file_write_handoff.get_or_init(|| ());
    }

    pub(super) fn file_write_handoff_enabled(&self) -> bool {
        self.file_write_handoff.get().is_some()
    }

    pub(super) fn write_not_entered_tag(&self, proof: &[u8]) -> HeaderValue {
        let mut context = hmac::Context::with_key(&self.key);
        context.update(WRITE_NOT_ENTERED);
        context.update(proof);
        let encoded = URL_SAFE_NO_PAD.encode(context.sign().as_ref());
        HeaderValue::from_str(&encoded).expect("base64 header")
    }

    pub(super) fn write_not_entered_verifier(&self, proof: &[u8]) -> WriteNotEntered {
        WriteNotEntered {
            key: self.key.clone(),
            message: [WRITE_NOT_ENTERED, proof].concat(),
        }
    }

    /// The exact headers an internal request sends: `headers`, the request ID
    /// the receiver will see, and a proof over both.
    ///
    /// `x-request-id` is signed, and the receiving replica's request context
    /// normalizes it or assigns a fresh one before the relay verifies the
    /// proof. A sender that signed without it was refused as forged, so every
    /// sender fixes the receiver's view of it here, signs exactly that map and
    /// must send exactly the returned map.
    pub fn signed_headers(
        &self,
        sender: &str,
        owner: &OwnershipToken,
        method: &Method,
        uri: &Uri,
        mut headers: HeaderMap,
        body: &[u8],
    ) -> Result<HeaderMap, ()> {
        let id = layer1_public::rest::request_id_from_headers(&headers)
            .map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_owned);
        headers.remove(REQUEST_ID);
        headers.insert(REQUEST_ID, HeaderValue::from_str(&id).map_err(|_| ())?);
        headers.remove(PROOF);
        let proof = self.proof(sender, owner, method, uri, &headers, body)?;
        headers.insert(PROOF, HeaderValue::from_str(&proof).map_err(|_| ())?);
        Ok(headers)
    }

    /// Sign arbitrary headers; tests use it to construct exact or forged proofs.
    #[cfg(test)]
    pub fn sign(
        &self,
        sender: &str,
        owner: &OwnershipToken,
        method: &Method,
        uri: &Uri,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<String, ()> {
        self.proof(sender, owner, method, uri, headers, body)
    }

    fn proof(
        &self,
        sender: &str,
        owner: &OwnershipToken,
        method: &Method,
        uri: &Uri,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<String, ()> {
        let nonce = uuid::Uuid::new_v4();
        let nonce = if file_read::eligible(uri.path(), method) {
            format!("{FILE_HANDOFF_NONCE}{nonce}")
        } else if self.file_write_handoff_enabled()
            && file_write::eligible(uri.path(), method, body)
        {
            format!("{FILE_WRITE_HANDOFF_NONCE}{nonce}")
        } else {
            nonce.to_string()
        };
        let proof = Proof {
            sender: sender.into(),
            owner: owner.clone(),
            nonce,
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

#[cfg(test)]
#[path = "authentication_tests.rs"]
mod tests;
