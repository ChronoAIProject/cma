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

    /// Parse only identities emitted by this adapter.  JSON-RPC response IDs
    /// are otherwise opaque and must never be interpreted as prompt results.
    pub fn parse(value: &str) -> Option<Self> {
        let digest = value.strip_prefix("cma-prompt-v1-")?;
        (digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .then(|| Self(value.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::PromptIdentity;

    #[test]
    fn parser_accepts_only_adapter_prompt_namespace() {
        let identity = PromptIdentity::new("conversation", 4, 9);
        assert_eq!(
            PromptIdentity::parse(identity.as_str()),
            Some(identity.clone())
        );
        assert!(PromptIdentity::parse("initialize").is_none());
        assert!(PromptIdentity::parse("cma-prompt-v1-abc").is_none());
        assert!(PromptIdentity::parse(&format!("{}x", identity.as_str())).is_none());
    }
}
