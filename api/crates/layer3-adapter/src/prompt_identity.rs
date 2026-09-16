use sha2::{Digest as _, Sha256};

/// Exact native prompt correlation, reproducible from an already admitted turn.
/// It does not grant authority or authorize another submission of that turn.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PromptIdentity(String);

impl PromptIdentity {
    pub fn new(conversation_id: &str, runtime_generation: u64, turn_sequence: u64) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"cma-native-prompt-v1\0");
        digest.update((conversation_id.len() as u64).to_be_bytes());
        digest.update(conversation_id.as_bytes());
        digest.update(runtime_generation.to_be_bytes());
        digest.update(turn_sequence.to_be_bytes());
        Self(format!("cma-prompt-v1-{:x}", digest.finalize()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
