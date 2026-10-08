use sha2::{Digest, Sha256};
pub(super) type EventKey = [u8; 32];
pub(super) fn key(channel: &str, event: &str) -> EventKey {
    let mut digest = Sha256::new();
    digest.update(channel.as_bytes());
    digest.update([0]);
    digest.update(event.as_bytes());
    digest.finalize().into()
}
pub(super) fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
