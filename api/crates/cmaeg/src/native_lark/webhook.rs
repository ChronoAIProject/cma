//! Authenticate exact encrypted bytes before decrypting or interpreting JSON.
use aes::Aes256;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use cmaeg_core::channel::{native_lark::safe_id, NativeLarkCredentials, NativeLarkIdentity};
use serde_json::Value;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
#[derive(Debug)]
pub(crate) enum Verified {
    Challenge(String),
    Event(Value),
}
pub(crate) fn verify(
    identity: &NativeLarkIdentity,
    credentials: &NativeLarkCredentials,
    timestamp: &str,
    nonce: &str,
    signature: &str,
    bytes: &[u8],
    now: i64,
) -> Result<Verified, ()> {
    let sent = timestamp.parse::<i64>().map_err(|_| ())?;
    if sent.to_string() != timestamp
        || now.abs_diff(sent) > 300
        || !safe_id(nonce)
        || signature.len() != 64
        || bytes.len() > 256 * 1024
    {
        return Err(());
    }
    let mut hash = Sha256::new();
    hash.update(timestamp);
    hash.update(nonce);
    hash.update(credentials.encrypt_key.as_str());
    hash.update(bytes);
    let expected = format!("{:x}", hash.finalize());
    if !bool::from(expected.as_bytes().ct_eq(signature.as_bytes())) {
        return Err(());
    }
    let outer: Value = serde_json::from_slice(bytes).map_err(|_| ())?;
    if outer.as_object().is_none_or(|o| o.len() != 1) {
        return Err(());
    }
    let encrypted = outer["encrypt"].as_str().ok_or(())?;
    let decoded = STANDARD.decode(encrypted).map_err(|_| ())?;
    if decoded.len() < 32 || decoded.len() % 16 != 0 {
        return Err(());
    }
    let key = Sha256::digest(credentials.encrypt_key.as_str().as_bytes());
    let mut buffer = decoded[16..].to_vec();
    let plaintext = cbc::Decryptor::<Aes256>::new(&key, (&decoded[..16]).into())
        .decrypt_padded_mut::<Pkcs7>(&mut buffer)
        .map_err(|_| ())?;
    let mut value: Value = serde_json::from_slice(plaintext).map_err(|_| ())?;
    let challenge = value["type"] == "url_verification";
    let token = if challenge {
        value["token"].as_str()
    } else {
        value["header"]["token"].as_str()
    }
    .ok_or(())?;
    if !bool::from(Sha256::digest(token.as_bytes()).ct_eq(&Sha256::digest(
        credentials.verification_token.as_str().as_bytes(),
    ))) {
        return Err(());
    }
    if challenge {
        let text = value["challenge"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 4096 && !s.chars().any(char::is_control))
            .ok_or(())?;
        return Ok(Verified::Challenge(text.into()));
    }
    if value["schema"] != "2.0"
        || value["header"]["app_id"].as_str() != Some(&identity.app_id)
        || value["header"]["tenant_key"].as_str() != Some(&identity.tenant_key)
    {
        return Err(());
    }
    value["header"].as_object_mut().ok_or(())?.remove("token");
    Ok(Verified::Event(value))
}
