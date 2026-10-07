//! Single-use media links: the per-pod redemption set and the stream slots
//! every download holds.
//!
//! A link is a standalone bearer seen by the model provider and stored by CMA
//! as a tool-call title, so it must be worthless once redeemed or expired.
use cmaeg_core::{ids::ChannelId, seal::MAX_MEDIA_LINK_TTL};
use dashmap::DashMap;
use std::{
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

/// Pod-local state of media downloads.
pub struct MediaDownloads {
    redeemed: moka::sync::Cache<[u8; JTI_BYTES], ()>,
    streams: Arc<Semaphore>,
    channel_streams: DashMap<ChannelId, Arc<Semaphore>>,
    acquisitions: AtomicU64,
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
    /// Mark `jti` redeemed; true only the first time on this pod. A jti of
    /// the wrong shape never reaches here, because opening the link refuses it.
    pub(super) fn redeem(&self, jti: &str) -> bool {
        let Some(key) = hex_bytes::<JTI_BYTES>(jti) else {
            return false;
        };
        self.redeemed.entry(key).or_insert(()).is_fresh()
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
