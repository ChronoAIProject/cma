//! Gateway turn execution components.

pub mod delivery;

pub mod turns {
    pub mod routed_scheduler;
    pub mod sequencer;
}

pub fn channel_hash(id: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(id.as_bytes());
    hash[..8].iter().map(|byte| format!("{byte:02x}")).collect()
}
