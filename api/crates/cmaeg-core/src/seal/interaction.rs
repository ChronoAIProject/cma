//! Opaque blobs bound to their token kind and record identity: channel
//! definitions, interaction cards and connections. Unlike URL tokens they
//! are not limited to `MAX_TOKEN_LENGTH`.
use super::{
    aad, MasterKeyRing, OpenError, Opened, SealError, TokenKind, MAX_BLOB_BYTES,
    MAX_BLOB_WIRE_LENGTH, TOKEN_VERSION,
};
use base64::{engine::general_purpose, Engine as _};
use ring::{
    aead::{self, Aad, Nonce},
    rand::{SecureRandom, SystemRandom},
};

/// Seal an opaque channel-definition blob without the short URL-token limit.
/// The wire value still carries the key id and authenticated `cmaeg1.bl.<kid>`
/// AAD so CMA can retain and return it without learning its contents.
pub fn seal_blob(ring: &MasterKeyRing, plaintext: &[u8]) -> Result<Vec<u8>, SealError> {
    seal_bound_blob(ring, TokenKind::Blob, "", plaintext)
}

/// Retain the original channel plaintext under a kind ordinary readers refuse.
pub fn seal_retired_channel(
    ring: &MasterKeyRing,
    record_id: &str,
    stored_version: u64,
    plaintext: &[u8],
) -> Result<Vec<u8>, SealError> {
    seal_bound_blob(
        ring,
        TokenKind::RetiredChannel,
        &format!(":{record_id}:{stored_version}"),
        plaintext,
    )
}

/// Open an ordinary or retired channel for inspection or custody accounting.
/// A retired blob authenticates the post-retirement CAS version. Its receipt
/// names the immediately preceding version.
pub fn open_retained_channel(
    ring: &MasterKeyRing,
    record_id: &str,
    stored_version: u64,
    token: &[u8],
) -> Result<(Opened<Vec<u8>>, Option<u64>), OpenError> {
    if !token.starts_with(b"cmaeg1.rc.") {
        return open_blob(ring, token).map(|opened| (opened, None));
    }
    let admitted = stored_version
        .checked_sub(1)
        .filter(|version| *version > 0)
        .ok_or(OpenError::Format)?;
    let opened = open_bound_blob(
        ring,
        TokenKind::RetiredChannel,
        &format!(":{record_id}:{stored_version}"),
        token,
    )?;
    Ok((opened, Some(admitted)))
}

pub fn seal_interaction(
    ring: &MasterKeyRing,
    id: &str,
    plaintext: &[u8],
) -> Result<Vec<u8>, SealError> {
    seal_bound_blob(ring, TokenKind::Interaction, id, plaintext)
}

/// Seal a card owned by a non-default consumer under its own kind, `ic`.
///
/// The kind, not only the card's `consumer` field, records the owner: an
/// image that predates consumers can open only `in`, so after an emergency
/// rollback a non-default consumer's card is refused rather than opened as a
/// default card and its click routed to the default consumer.
pub fn seal_consumer_interaction(
    ring: &MasterKeyRing,
    id: &str,
    plaintext: &[u8],
) -> Result<Vec<u8>, SealError> {
    seal_bound_blob(ring, TokenKind::ConsumerInteraction, id, plaintext)
}

/// Connection data is bound to its namespace and record identity.
pub fn seal_connection(
    ring: &MasterKeyRing,
    id: &str,
    plaintext: &[u8],
) -> Result<Vec<u8>, SealError> {
    seal_bound_blob(ring, TokenKind::Connection, id, plaintext)
}

pub fn open_connection(
    ring: &MasterKeyRing,
    id: &str,
    token: &[u8],
) -> Result<Opened<Vec<u8>>, OpenError> {
    open_bound_blob(ring, TokenKind::Connection, id, token)
}

fn seal_bound_blob(
    ring: &MasterKeyRing,
    kind: TokenKind,
    id: &str,
    plaintext: &[u8],
) -> Result<Vec<u8>, SealError> {
    if plaintext.len() > MAX_BLOB_BYTES {
        return Err(SealError::TooLarge);
    }
    let kid = ring.current_kid();
    let aad = format!("{}{id}", aad(kind, kid));
    let mut nonce = [0_u8; 12];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(SealError::Random)?;
    let nonce_value = Nonce::assume_unique_for_key(nonce);
    let mut ciphertext = plaintext.to_vec();
    ring.key(kid)
        .expect("the ring always contains its current key")
        .seal_in_place_append_tag(nonce_value, Aad::from(aad.as_bytes()), &mut ciphertext)
        .map_err(|_| SealError::Random(ring::error::Unspecified))?;
    let mut encoded = Vec::with_capacity(12 + ciphertext.len());
    encoded.extend_from_slice(&nonce);
    encoded.extend_from_slice(&ciphertext);
    Ok(format!(
        "{TOKEN_VERSION}.{}.{kid}.{}",
        kind.wire_name(),
        general_purpose::URL_SAFE_NO_PAD.encode(encoded)
    )
    .into_bytes())
}

/// Open an opaque channel-definition blob and report whether it needs rekeying.
pub fn open_blob(ring: &MasterKeyRing, token: &[u8]) -> Result<Opened<Vec<u8>>, OpenError> {
    open_bound_blob(ring, TokenKind::Blob, "", token)
}

pub fn open_interaction(
    ring: &MasterKeyRing,
    id: &str,
    token: &[u8],
) -> Result<Opened<Vec<u8>>, OpenError> {
    open_bound_blob(ring, TokenKind::Interaction, id, token)
}

/// Open a card sealed by [`seal_consumer_interaction`]; an `in` card is refused.
pub fn open_consumer_interaction(
    ring: &MasterKeyRing,
    id: &str,
    token: &[u8],
) -> Result<Opened<Vec<u8>>, OpenError> {
    open_bound_blob(ring, TokenKind::ConsumerInteraction, id, token)
}

fn open_bound_blob(
    ring: &MasterKeyRing,
    kind: TokenKind,
    id: &str,
    token: &[u8],
) -> Result<Opened<Vec<u8>>, OpenError> {
    if token.len() > MAX_BLOB_WIRE_LENGTH {
        return Err(OpenError::Format);
    }
    let token = std::str::from_utf8(token).map_err(|_| OpenError::Format)?;
    let parts = token.split('.').collect::<Vec<_>>();
    if parts.len() != 4 || parts[0] != TOKEN_VERSION || parts[1] != kind.wire_name() {
        return Err(OpenError::Format);
    }
    let kid = parts[2].parse::<u8>().map_err(|_| OpenError::Format)?;
    // Token text is also a cache/rate-limit identity; equivalent numeric spellings
    // must not create new identities for the same authenticated ciphertext.
    if kid.to_string() != parts[2] {
        return Err(OpenError::Format);
    }
    let key = ring.key(kid).ok_or(OpenError::UnknownKid(kid))?;
    let mut bytes = general_purpose::URL_SAFE_NO_PAD
        .decode(parts[3])
        .map_err(|_| OpenError::Format)?;
    if bytes.len() < 12 + aead::MAX_TAG_LEN {
        return Err(OpenError::Format);
    }
    let nonce_bytes: [u8; 12] = bytes[..12].try_into().map_err(|_| OpenError::Format)?;
    let nonce = Nonce::assume_unique_for_key(nonce_bytes);
    let mut ciphertext = bytes.split_off(12);
    let plaintext = key
        .open_in_place(
            nonce,
            Aad::from(format!("{}{id}", aad(kind, kid)).as_bytes()),
            &mut ciphertext,
        )
        .map_err(|_| OpenError::Tampered)?;
    if plaintext.len() > MAX_BLOB_BYTES {
        return Err(OpenError::Format);
    }
    Ok(Opened {
        payload: plaintext.to_vec(),
        kid,
        needs_rekey: kid != ring.current_kid(),
    })
}
