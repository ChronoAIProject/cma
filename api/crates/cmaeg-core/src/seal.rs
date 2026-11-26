//! Versioned, stateless tokens used between CMAEG replicas.

mod custody;

use std::fmt;

use base64::{engine::general_purpose, Engine as _};
use ring::{
    aead::{self, Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM},
    hkdf, hmac,
    rand::{SecureRandom, SystemRandom},
};
use serde::{de::DeserializeOwned, Serialize};

mod console_login;
pub mod event_ref;
mod interaction;
pub mod lark_effect;
pub mod lineage;
pub mod media_link;
pub mod message_ref;
pub mod native_lark_effect;
pub mod native_media_edit;
pub mod pointer;
pub mod received_reaction;
pub mod reply_target;
pub mod thread_ref;
pub use thread_ref::{ThreadRef, ThreadRefPayload};
mod telegram_thread_ref;
pub use telegram_thread_ref::TelegramThreadRefPayload;
mod history_cursor;
pub use history_cursor::{
    HistoryCursor, HistoryCursorPayload, HistoryOrigin, NativeLarkHistoryCursorPayload,
};
pub mod telegram_effect;
pub mod whatsapp_reaction;

pub use console_login::{open_console_login, seal_console_login, ConsoleLogin, CONSOLE_LOGIN_TTL};
pub use event_ref::{EventRef, EventRefPayload, ReplyAddress};
pub use interaction::{
    open_blob, open_connection, open_consumer_interaction, open_interaction, open_retained_channel,
    seal_blob, seal_connection, seal_consumer_interaction, seal_interaction, seal_retired_channel,
};
pub use lineage::{Lineage, RootId};
pub use media_link::{MediaLinkPayload, MediaLinkRef, MAX_MEDIA_LINK_TTL};
pub use message_ref::{
    DiscordEffectWindow, DiscordMessageDestination, MessageRefPayload, RelayMessageDestination,
};
pub use pointer::ChannelPointer;
pub use reply_target::{ReplyTargetPayload, ReplyTargetRef, REPLY_TARGET_TTL};
pub use telegram_effect::{TelegramAdmissionProof, TelegramEffectAuthority, TELEGRAM_EFFECT_TTL};

pub const MAX_TOKEN_LENGTH: usize = 4_096;
/// Maximum plaintext with space for the wire prefix, nonce, tag and base64 expansion.
pub const MAX_BLOB_BYTES: usize = (MAX_BLOB_WIRE_LENGTH - 14) / 4 * 3 - 28;
const MAX_BLOB_WIRE_LENGTH: usize = 65_536;
const TOKEN_VERSION: &str = "cmaeg1";
const HKDF_SALT: &[u8] = b"cmaeg.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Channel,
    EventRef,
    MessageRef,
    Blob,
    RetiredChannel,
    Interaction,
    /// An interaction card owned by a non-default consumer.
    ConsumerInteraction,
    ReplyTarget,
    ThreadRef,
    HistoryCursor,
    Connection,
    ConnectionSetup,
    MediaLink,
    ConsoleLogin,
}

impl TokenKind {
    fn wire_name(self) -> &'static str {
        match self {
            Self::Channel => "ch",
            Self::EventRef => "ev",
            Self::MessageRef => "mr",
            Self::Blob => "bl",
            Self::RetiredChannel => "rc",
            Self::Interaction => "in",
            Self::ConsumerInteraction => "ic",
            Self::ReplyTarget => "rt",
            Self::ThreadRef => "tr",
            Self::HistoryCursor => "hc",
            Self::Connection => "cn",
            Self::ConnectionSetup => "cs",
            Self::MediaLink => "md",
            Self::ConsoleLogin => "cl",
        }
    }
}

/// A key ring with an independently movable derivation key.
pub struct MasterKeyRing {
    keys: Vec<(u8, LessSafeKey)>,
    hmac: Vec<(u8, hmac::Key)>,
    current: u8,
    derivation: u8,
}

impl fmt::Debug for MasterKeyRing {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MasterKeyRing")
            .field(
                "kids",
                &self.keys.iter().map(|(kid, _)| kid).collect::<Vec<_>>(),
            )
            .field("current", &self.current)
            .field("derivation", &self.derivation)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyRingError {
    Empty,
    TooMany,
    InvalidEntry,
    DuplicateKid(u8),
    InvalidKey(u8),
    InvalidBase64(u8),
}

impl fmt::Display for KeyRingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for KeyRingError {}

impl MasterKeyRing {
    /// Parse `kid:base64-secret[,kid:base64-secret...]`, first entry current.
    pub fn parse(spec: &str) -> Result<Self, KeyRingError> {
        let entries = spec.split(',').collect::<Vec<_>>();
        if entries.is_empty() || entries.iter().any(|entry| entry.is_empty()) {
            return Err(KeyRingError::Empty);
        }
        if entries.len() > 8 {
            return Err(KeyRingError::TooMany);
        }
        let mut keys = Vec::with_capacity(entries.len());
        let mut hmac_keys = Vec::with_capacity(entries.len());
        for entry in entries {
            let (kid, encoded) = entry.split_once(':').ok_or(KeyRingError::InvalidEntry)?;
            let kid = kid.parse::<u8>().map_err(|_| KeyRingError::InvalidEntry)?;
            if keys.iter().any(|(known, _)| *known == kid) {
                return Err(KeyRingError::DuplicateKid(kid));
            }
            let bytes = general_purpose::STANDARD
                .decode(encoded)
                .or_else(|_| general_purpose::STANDARD_NO_PAD.decode(encoded))
                .map_err(|_| KeyRingError::InvalidBase64(kid))?;
            if bytes.len() != 32 {
                return Err(KeyRingError::InvalidKey(kid));
            }
            let unbound =
                UnboundKey::new(&AES_256_GCM, &bytes).map_err(|_| KeyRingError::InvalidKey(kid))?;
            let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, HKDF_SALT);
            let prk = salt.extract(&bytes);
            let okm = prk
                .expand(&[b"hmac"], hkdf::HKDF_SHA256)
                .map_err(|_| KeyRingError::InvalidKey(kid))?;
            let mut hmac_bytes = [0_u8; 32];
            okm.fill(&mut hmac_bytes)
                .map_err(|_| KeyRingError::InvalidKey(kid))?;
            keys.push((kid, LessSafeKey::new(unbound)));
            hmac_keys.push((kid, hmac::Key::new(hmac::HMAC_SHA256, &hmac_bytes)));
        }
        let current = keys[0].0;
        Ok(Self {
            keys,
            hmac: hmac_keys,
            current,
            derivation: current,
        })
    }

    pub fn current_kid(&self) -> u8 {
        self.current
    }
    pub fn derivation_kid(&self) -> u8 {
        self.derivation
    }

    pub fn set_derivation_kid(&mut self, kid: u8) -> Result<(), KeyRingError> {
        if self.keys.iter().any(|(known, _)| *known == kid) {
            self.derivation = kid;
            Ok(())
        } else {
            Err(KeyRingError::DuplicateKid(kid))
        }
    }

    pub(crate) fn key(&self, kid: u8) -> Option<&LessSafeKey> {
        self.keys
            .iter()
            .find_map(|(known, key)| (*known == kid).then_some(key))
    }

    pub(crate) fn hmac_key(&self, kid: u8) -> Option<&hmac::Key> {
        self.hmac
            .iter()
            .find_map(|(known, key)| (*known == kid).then_some(key))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened<T> {
    pub payload: T,
    pub kid: u8,
    pub needs_rekey: bool,
}

#[derive(Debug)]
pub enum SealError {
    InvalidPayload,
    Serialize(serde_json::Error),
    Random(ring::error::Unspecified),
    TooLarge,
}

impl fmt::Display for SealError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPayload => formatter.write_str("invalid token payload"),
            Self::Serialize(error) => write!(formatter, "cannot serialize token payload: {error}"),
            Self::Random(_) => formatter.write_str("secure nonce generation failed"),
            Self::TooLarge => formatter.write_str("sealed payload exceeds its size limit"),
        }
    }
}

impl std::error::Error for SealError {}

#[derive(Debug)]
pub enum OpenError {
    Format,
    UnknownKid(u8),
    WrongKind,
    Tampered,
    Malformed(serde_json::Error),
    Expired,
}

impl fmt::Display for OpenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Format => formatter.write_str("malformed token"),
            Self::UnknownKid(kid) => write!(formatter, "unknown token key {kid}"),
            Self::WrongKind => formatter.write_str("token kind does not match the operation"),
            Self::Tampered => formatter.write_str("token authentication failed"),
            Self::Malformed(error) => write!(formatter, "malformed token payload: {error}"),
            Self::Expired => formatter.write_str("token has expired"),
        }
    }
}

impl std::error::Error for OpenError {}

pub fn seal<T: Serialize>(
    ring: &MasterKeyRing,
    kind: TokenKind,
    payload: &T,
) -> Result<String, SealError> {
    let value = serde_json::to_value(payload).map_err(SealError::Serialize)?;
    let mut plaintext = crate::profile::canonical_json(value).into_bytes();
    let kid = ring.current_kid();
    let aad = aad(kind, kid);
    let mut nonce = [0_u8; 12];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(SealError::Random)?;
    let nonce_value = Nonce::assume_unique_for_key(nonce);
    ring.key(kid)
        .expect("the ring always contains its current key")
        .seal_in_place_append_tag(nonce_value, Aad::from(aad.as_bytes()), &mut plaintext)
        .map_err(|_| SealError::Random(ring::error::Unspecified))?;
    let mut encoded = Vec::with_capacity(12 + plaintext.len());
    encoded.extend_from_slice(&nonce);
    encoded.extend_from_slice(&plaintext);
    let token = format!(
        "{TOKEN_VERSION}.{}.{}.{}",
        kind.wire_name(),
        kid,
        general_purpose::URL_SAFE_NO_PAD.encode(encoded)
    );
    if token.len() > MAX_TOKEN_LENGTH {
        return Err(SealError::TooLarge);
    }
    Ok(token)
}

pub fn open<T: DeserializeOwned>(
    ring: &MasterKeyRing,
    kind: TokenKind,
    token: &str,
) -> Result<Opened<T>, OpenError> {
    if token.len() > MAX_TOKEN_LENGTH {
        return Err(OpenError::Format);
    }
    let parts = token.split('.').collect::<Vec<_>>();
    if parts.len() != 4 || parts[0] != TOKEN_VERSION {
        return Err(OpenError::Format);
    }
    if parts[1] != kind.wire_name() {
        return Err(OpenError::WrongKind);
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
        .open_in_place(nonce, Aad::from(aad(kind, kid).as_bytes()), &mut ciphertext)
        .map_err(|_| OpenError::Tampered)?;
    let payload = serde_json::from_slice(plaintext).map_err(OpenError::Malformed)?;
    Ok(Opened {
        payload,
        kid,
        needs_rekey: kid != ring.current_kid(),
    })
}

fn aad(kind: TokenKind, kid: u8) -> String {
    format!("{TOKEN_VERSION}.{}.{}", kind.wire_name(), kid)
}

pub(crate) fn sign(ring: &MasterKeyRing, kid: u8, data: &[u8]) -> Vec<u8> {
    hmac::sign(
        ring.hmac_key(kid).expect("requested derivation key exists"),
        data,
    )
    .as_ref()
    .to_vec()
}
