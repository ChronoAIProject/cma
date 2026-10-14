//! Single-use media download capabilities sealed as `cmaeg1.md` tokens.
//!
//! A link is a standalone bearer: it reaches the model provider inside a tool
//! result and CMA's durable tool-call titles inside a shell command line. It
//! therefore carries no reply authority, expires within minutes and never
//! after its event, and is formatted only where a request header is built.
use super::{
    open as open_token, seal as seal_token, EventRefPayload, MasterKeyRing, OpenError,
    ReplyAddress, SealError, TokenKind, MAX_TOKEN_LENGTH,
};
use crate::{
    activity::MAX_ATTACHMENTS,
    channel::Secret,
    ids::{BindingId, ProviderSlug},
};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

/// The longest a link may live, whatever the configuration says.
pub const MAX_MEDIA_LINK_TTL: i64 = 600;
const PREFIX: &str = "cmaeg1.md.";
const VERSION: u8 = 1;
const JTI_BYTES: usize = 16;

/// What a media link authorizes: one attachment of one event, for the binding
/// and agent key that minted it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaLinkPayload {
    pub v: u8,
    /// 16 random bytes as 32 lowercase hex characters; the redemption key.
    pub jti: String,
    pub provider: ProviderSlug,
    pub binding_id: BindingId,
    /// SHA-256 of the agent key at mint, as 64 lowercase hex characters, so
    /// rotating the key revokes the link.
    pub agent_key_hash: String,
    pub issued_at: i64,
    pub expires_at: i64,
    /// The event, with any relay reply token blanked.
    pub event: EventRefPayload,
    pub index: u8,
}

/// The jti and the key hash identify a live capability, and the event names
/// the conversation, so only the window and the index are shown.
impl fmt::Debug for MediaLinkPayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MediaLinkPayload")
            .field("v", &self.v)
            .field("index", &self.index)
            .field("issued_at", &self.issued_at)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

impl MediaLinkPayload {
    /// The only constructor minting uses. It draws a fresh jti, blanks the
    /// event's relay reply token so the link never carries reply authority,
    /// and ends the window at `issued_at + ttl` (never past
    /// [`MAX_MEDIA_LINK_TTL`]) or at the event's own expiry, whichever is
    /// first.
    pub fn new(
        provider: ProviderSlug,
        binding_id: BindingId,
        agent_key_hash: &[u8; 32],
        mut event: EventRefPayload,
        index: u8,
        issued_at: i64,
        ttl: i64,
    ) -> Result<Self, SealError> {
        let mut jti = [0_u8; JTI_BYTES];
        SystemRandom::new()
            .fill(&mut jti)
            .map_err(SealError::Random)?;
        if let ReplyAddress::NyxidRelay { reply_token, .. } = &mut event.reply {
            *reply_token = Secret::new("");
        }
        let expires_at = issued_at
            .saturating_add(ttl.clamp(0, MAX_MEDIA_LINK_TTL))
            .min(event.expires_at);
        Ok(Self {
            v: VERSION,
            jti: hex(&jti),
            provider,
            binding_id,
            agent_key_hash: hex(agent_key_hash),
            issued_at,
            expires_at,
            event,
            index,
        })
    }

    /// Every invariant a minted payload holds, in the order `open` checks
    /// them after the kind; expiry is checked separately.
    fn check(&self, now: i64) -> Result<(), OpenError> {
        let relay_token_blank = match &self.event.reply {
            ReplyAddress::NyxidRelay { reply_token, .. } => reply_token.as_str().is_empty(),
            _ => true,
        };
        let valid = self.v == VERSION
            && lower_hex(&self.jti, 2 * JTI_BYTES)
            && lower_hex(&self.agent_key_hash, 64)
            && self.issued_at <= now
            && self.expires_at.saturating_sub(self.issued_at) <= MAX_MEDIA_LINK_TTL
            && self.expires_at <= self.event.expires_at
            && self.event.activity_sha256.is_some()
            && self.event.validate().is_ok()
            && usize::from(self.index) < MAX_ATTACHMENTS
            && relay_token_blank;
        valid.then_some(()).ok_or(OpenError::Format)
    }
}

/// A sealed media link. It has no `Display` and no serializer, so it cannot
/// be formatted into a log line or a JSON body by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct MediaLinkRef(String);

impl fmt::Debug for MediaLinkRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MediaLinkRef([redacted])")
    }
}

impl FromStr for MediaLinkRef {
    type Err = OpenError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        if !raw.starts_with(PREFIX) || raw.len() > MAX_TOKEN_LENGTH {
            return Err(OpenError::Format);
        }
        Ok(Self(raw.to_owned()))
    }
}

impl MediaLinkRef {
    /// The token itself, for building the one `Authorization` header that
    /// carries it. Nothing else may read it.
    pub fn expose_header_value(&self) -> &str {
        &self.0
    }
}

/// Seal `payload`. A payload whose token would exceed [`MAX_TOKEN_LENGTH`],
/// such as one whose event has a large custom partition key, is
/// [`SealError::TooLarge`] and is never truncated.
pub fn seal(ring: &MasterKeyRing, payload: &MediaLinkPayload) -> Result<MediaLinkRef, SealError> {
    seal_token(ring, TokenKind::MediaLink, payload).map(MediaLinkRef)
}

/// Open `token` at `now`. Any failure to authenticate or any broken
/// invariant is [`OpenError::Format`]; a valid link past its expiry is
/// [`OpenError::Expired`].
pub fn open(
    ring: &MasterKeyRing,
    token: &MediaLinkRef,
    now: i64,
) -> Result<MediaLinkPayload, OpenError> {
    let payload = open_token::<MediaLinkPayload>(ring, TokenKind::MediaLink, &token.0)
        .map_err(|_| OpenError::Format)?
        .payload;
    payload.check(now)?;
    if now >= payload.expires_at {
        return Err(OpenError::Expired);
    }
    Ok(payload)
}

fn lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
