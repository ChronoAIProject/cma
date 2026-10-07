//! Single-use media links: minting for autonomous tool-bearing bindings, the
//! per-pod redemption set, and the stream slots every download holds.
//!
//! A link is a standalone bearer seen by the model provider and stored by CMA
//! as a tool-call title, so it must be worthless once redeemed or expired.
use super::{MediaDownload, ToolsState};
use crate::{
    channels::resolver::ResolvedChannel,
    telemetry::{self, names},
};
use cmaeg_core::{
    activity::Attachment,
    ids::ChannelId,
    seal::{media_link, EventRefPayload, MediaLinkPayload, SealError, MAX_MEDIA_LINK_TTL},
};
use dashmap::DashMap;
use governor::{DefaultDirectRateLimiter, Quota, RateLimiter};
use std::{
    num::NonZeroU32,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Links all channels together may mint per minute.
pub(crate) const GLOBAL_LINK_MINTS_PER_MINUTE: u32 = 600;
/// Live links are bounded by the global mint rate times the longest TTL, so
/// a capacity above that bound never evicts a jti before its link expires:
/// single use is exact for the pod's lifetime. At about 128 bytes an entry
/// the full set stays near 2 MiB.
const REDEEMED_CAPACITY: u64 = 16_384;
const _: () = assert!(
    REDEEMED_CAPACITY > GLOBAL_LINK_MINTS_PER_MINUTE as u64 * (MAX_MEDIA_LINK_TTL as u64 / 60)
);
/// Concurrent download streams per pod.
pub const DOWNLOAD_STREAMS_GLOBAL: usize = 16;
/// Concurrent download streams per channel, so one tenant cannot take them all.
pub const DOWNLOAD_STREAMS_PER_CHANNEL: usize = 2;
/// The share of the media response budget one download stream holds.
pub const DOWNLOAD_RESPONSE_WEIGHT: u64 = 262_144;
/// Idle per-channel slots are swept once every this many acquisitions.
const SWEEP_EVERY: u64 = 256;
const JTI_BYTES: usize = 16;
/// The configuration's default link lifetime.
const DEFAULT_LINK_TTL_SECONDS: i64 = 120;

/// Pod-local state of media links: minting limits, redemptions and stream
/// slots.
pub struct MediaDownloads {
    redeemed: moka::sync::Cache<[u8; JTI_BYTES], ()>,
    streams: Arc<Semaphore>,
    channel_streams: DashMap<ChannelId, Arc<Semaphore>>,
    acquisitions: AtomicU64,
    /// Bounds the links live at once, so the redemption set can never evict
    /// a jti whose link has not expired.
    mints: DefaultDirectRateLimiter,
    link_ttl: i64,
}

impl Default for MediaDownloads {
    fn default() -> Self {
        Self {
            redeemed: moka::sync::Cache::builder()
                .max_capacity(REDEEMED_CAPACITY)
                .time_to_live(Duration::from_secs(MAX_MEDIA_LINK_TTL.unsigned_abs()))
                .build(),
            streams: Arc::new(Semaphore::new(DOWNLOAD_STREAMS_GLOBAL)),
            channel_streams: DashMap::new(),
            acquisitions: AtomicU64::new(0),
            mints: RateLimiter::direct(Quota::per_minute(
                NonZeroU32::new(GLOBAL_LINK_MINTS_PER_MINUTE).unwrap_or(NonZeroU32::MIN),
            )),
            link_ttl: DEFAULT_LINK_TTL_SECONDS,
        }
    }
}

/// The global and per-channel stream slots one download holds until its body
/// ends.
pub(crate) struct StreamSlots {
    pub(crate) global: OwnedSemaphorePermit,
    pub(crate) channel: OwnedSemaphorePermit,
}

impl MediaDownloads {
    /// Links live `seconds`, which the configuration bounds to 30..=600; the
    /// sealed format refuses anything longer regardless.
    pub fn with_link_ttl(mut self, seconds: u64) -> Self {
        self.link_ttl = i64::try_from(seconds)
            .unwrap_or(MAX_MEDIA_LINK_TTL)
            .min(MAX_MEDIA_LINK_TTL);
        self
    }

    /// A lower pod-wide mint rate. Only a lower rate keeps the redemption
    /// set's bound, so a higher one is ignored.
    pub fn with_global_mint_rate(mut self, per_minute: NonZeroU32) -> Self {
        if per_minute.get() <= GLOBAL_LINK_MINTS_PER_MINUTE {
            self.mints = RateLimiter::direct(Quota::per_minute(per_minute));
        }
        self
    }

    /// Mark `jti` redeemed; true only the first time on this pod. A jti of
    /// the wrong shape never reaches here, because opening the link refuses it.
    pub(super) fn redeem(&self, jti: &str) -> bool {
        let Some(key) = hex_bytes::<JTI_BYTES>(jti) else {
            return false;
        };
        // Only `or_insert_with` coalesces simultaneous first insertions of one
        // key into a single fresh entry; `or_insert` is a get then an insert,
        // so every caller that misses the get reports fresh and one link
        // would be served to each racing request.
        self.redeemed.entry(key).or_insert_with(|| ()).is_fresh()
    }

    /// A global and a per-channel stream slot, without waiting.
    pub(crate) fn acquire(&self, channel: &ChannelId) -> Option<StreamSlots> {
        if self
            .acquisitions
            .fetch_add(1, Ordering::Relaxed)
            .is_multiple_of(SWEEP_EVERY)
        {
            // `retain` holds each shard's write lock while deciding, so no
            // clone can be taken from an entry while it is being removed; an
            // entry is idle when only the map holds it and every slot is free.
            self.channel_streams.retain(|_, semaphore| {
                !(Arc::strong_count(semaphore) == 1
                    && semaphore.available_permits() == DOWNLOAD_STREAMS_PER_CHANNEL)
            });
        }
        let global = self.streams.clone().try_acquire_owned().ok()?;
        let semaphore = self
            .channel_streams
            .entry(channel.clone())
            .or_insert_with(|| Arc::new(Semaphore::new(DOWNLOAD_STREAMS_PER_CHANNEL)))
            .clone();
        let channel = semaphore.try_acquire_owned().ok()?;
        Some(StreamSlots { global, channel })
    }

    /// The pod-wide stream slots, for observing or holding them.
    pub fn streams(&self) -> Arc<Semaphore> {
        self.streams.clone()
    }

    /// Free stream slots of `channel`.
    pub fn channel_streams_available(&self, channel: &ChannelId) -> usize {
        self.channel_streams
            .get(channel)
            .map_or(DOWNLOAD_STREAMS_PER_CHANNEL, |semaphore| {
                semaphore.available_permits()
            })
    }
}

/// Why no link was minted; each maps to one unavailable reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkRefusal {
    /// The binding is not autonomous or installs no gateway tools.
    NotPermitted,
    RateLimited,
    /// The link could not be produced, such as one too large to seal.
    Unavailable,
}

impl LinkRefusal {
    fn as_str(self) -> &'static str {
        match self {
            Self::NotPermitted => "not_permitted",
            Self::RateLimited => "rate_limited",
            Self::Unavailable => "unavailable",
        }
    }
}

impl ToolsState {
    /// Mint a single-use link to attachment `index` of `event` for
    /// `channel`'s binding and current agent key, as one ready `curl`
    /// command. The caller has already refused an attachment without a
    /// source. The token appears only inside `curl` and is never logged.
    pub fn mint_link(
        &self,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        attachment: &Attachment,
        index: u8,
    ) -> Result<MediaDownload, LinkRefusal> {
        let result = self.mint(channel, event, attachment, index);
        let outcome = result
            .as_ref()
            .map_or_else(|refusal| refusal.as_str(), |_| "minted");
        metrics::counter!(names::MEDIA_LINKS, names::OUTCOME => outcome).increment(1);
        result
    }

    fn mint(
        &self,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        attachment: &Attachment,
        index: u8,
    ) -> Result<MediaDownload, LinkRefusal> {
        // Only an autonomous binding whose provider installs the gateway
        // tools may save originals; media off permits nothing.
        let media = self.media.as_deref().ok_or(LinkRefusal::NotPermitted)?;
        if !media.audience(&channel.definition).downloads {
            return Err(LinkRefusal::NotPermitted);
        }
        let channel_hash = telemetry::channel_hash(channel.id.as_str());
        if !media.limiter().link.check(&channel.id) {
            tracing::info!(channel = %channel_hash, "media link refused: channel mint quota spent");
            return Err(LinkRefusal::RateLimited);
        }
        if self.downloads.mints.check().is_err() {
            tracing::warn!(channel = %channel_hash, "media link refused: pod mint quota spent");
            return Err(LinkRefusal::RateLimited);
        }
        let provider = &channel.definition.provider;
        let payload = MediaLinkPayload::new(
            provider.slug.clone(),
            provider.binding_id.clone(),
            &channel.agent_key_hash,
            event.clone(),
            index,
            time::OffsetDateTime::now_utc().unix_timestamp(),
            self.downloads.link_ttl,
        )
        .map_err(|_| {
            tracing::error!(channel = %channel_hash, "media link unavailable: no secure random jti");
            LinkRefusal::Unavailable
        })?;
        let token = match media_link::seal(&self.keys, &payload) {
            Ok(token) => token,
            Err(SealError::TooLarge) => {
                let payload_bytes = serde_json::to_vec(&payload).map_or(0, |bytes| bytes.len());
                tracing::warn!(channel = %channel_hash, payload_bytes, "media link exceeds the token limit");
                return Err(LinkRefusal::Unavailable);
            }
            Err(_) => {
                tracing::error!(channel = %channel_hash, "media link could not be sealed");
                return Err(LinkRefusal::Unavailable);
            }
        };
        let url = format!("{}/v1/media/content", self.public_origin);
        // Neither the token's alphabet nor a safe name contains `'`, so the
        // single-quoted words cannot be broken out of.
        let curl = format!(
            "curl -fsSL -H 'Authorization: Bearer {}' -o '{}' '{url}'",
            token.expose_header_value(),
            safe_name(attachment.filename.as_deref()),
        );
        tracing::debug!(
            channel = %channel_hash,
            lifetime = payload.expires_at - payload.issued_at,
            "media link minted"
        );
        Ok(MediaDownload {
            method: "GET".into(),
            url,
            expires_at: payload.expires_at,
            curl,
        })
    }
}

/// The bytes of `2 * N` hex characters.
pub(super) fn hex_bytes<const N: usize>(hex: &str) -> Option<[u8; N]> {
    if hex.len() != 2 * N {
        return None;
    }
    let mut bytes = [0_u8; N];
    for (byte, pair) in bytes.iter_mut().zip(hex.as_bytes().chunks(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(bytes)
}

/// A file name safe in a `Content-Disposition` header and in a single-quoted
/// shell word: only `[A-Za-z0-9._-]`, at most 64 bytes, never starting with
/// `.` or `-`, and `attachment.bin` when nothing is left.
pub(crate) fn safe_name(filename: Option<&str>) -> String {
    let kept: String = filename
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .take(64)
        .collect();
    if kept.is_empty() {
        "attachment.bin".to_owned()
    } else if kept.starts_with(['.', '-']) {
        format!("attachment-{kept}")
    } else {
        kept
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_names_keep_only_portable_characters() {
        assert_eq!(safe_name(Some("report.pdf")), "report.pdf");
        assert_eq!(
            safe_name(Some("../../etc/passwd")),
            "attachment-....etcpasswd"
        );
        assert_eq!(safe_name(Some("-rf")), "attachment--rf");
        assert_eq!(safe_name(Some(&"n".repeat(200))).len(), 64);
        assert_eq!(safe_name(Some("報告書")), "attachment.bin");
        assert_eq!(safe_name(None), "attachment.bin");
        assert_eq!(safe_name(Some("it's a \"name\"")), "itsaname");
    }

    #[test]
    fn hex_decodes_exactly_sized_input() {
        assert_eq!(hex_bytes::<2>("0aff"), Some([0x0a, 0xff]));
        assert_eq!(hex_bytes::<2>("0af"), None);
        assert_eq!(hex_bytes::<2>("0afg"), None);
    }

    #[test]
    fn a_jti_redeems_once() {
        let downloads = MediaDownloads::default();
        let jti = "0123456789abcdef0123456789abcdef";
        assert!(downloads.redeem(jti));
        assert!(!downloads.redeem(jti));
        assert!(downloads.redeem("fedcba9876543210fedcba9876543210"));
        assert!(!downloads.redeem("short"));
    }

    #[test]
    fn simultaneous_redemptions_of_one_jti_have_one_winner() {
        const THREADS: usize = 8;
        // Each round is one jti that every thread redeems at once; the race
        // is a narrow window, so many rounds make a regression show.
        const ROUNDS: u32 = 1_000;
        let downloads = MediaDownloads::default();
        for round in 0..ROUNDS {
            let jti = format!("{round:032x}");
            let barrier = std::sync::Barrier::new(THREADS);
            let winners = std::thread::scope(|scope| {
                let redemptions: Vec<_> = (0..THREADS)
                    .map(|_| {
                        scope.spawn(|| {
                            barrier.wait();
                            downloads.redeem(&jti)
                        })
                    })
                    .collect();
                redemptions
                    .into_iter()
                    .map(|redemption| redemption.join().unwrap())
                    .filter(|fresh| *fresh)
                    .count()
            });
            assert_eq!(winners, 1, "round {round}");
        }
    }

    #[test]
    fn stream_slots_are_bounded_per_channel_and_released() {
        let downloads = MediaDownloads::default();
        let channel = ChannelId::new("cmaeg1.ch.downloads").unwrap();
        let first = downloads.acquire(&channel).unwrap();
        let _second = downloads.acquire(&channel).unwrap();
        assert!(downloads.acquire(&channel).is_none());
        assert_eq!(downloads.channel_streams_available(&channel), 0);
        assert_eq!(
            downloads.streams().available_permits(),
            DOWNLOAD_STREAMS_GLOBAL - 2
        );
        drop(first);
        assert_eq!(downloads.channel_streams_available(&channel), 1);
        assert!(downloads.acquire(&channel).is_some());
    }
}
