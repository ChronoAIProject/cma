//! Domain-separated server-keyed custody comparison without exposing key material.
use super::MasterKeyRing;

impl MasterKeyRing {
    /// Pin a credential to one internal purpose and canonical custody context.
    /// The retained key ID makes ordinary master-key rotation deterministic.
    pub fn custody_fingerprint(
        &self,
        kid: u8,
        purpose: &str,
        context: &[u8],
        credential: &[u8],
    ) -> Option<String> {
        if purpose.is_empty()
            || purpose.len() > 128
            || context.len() > 16384
            || credential.len() > 65536
        {
            return None;
        }
        let mut tag = ring::hmac::Context::with_key(self.hmac_key(kid)?);
        tag.update(b"cmaeg.custody-fingerprint.v1\0");
        for part in [purpose.as_bytes(), context, credential] {
            tag.update(&(part.len() as u64).to_be_bytes());
            tag.update(part);
        }
        Some(
            tag.sign()
                .as_ref()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn custody_fingerprint_separates_purpose_context_secret_and_retained_key() {
        let old = MasterKeyRing::parse("1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap();
        let rotated = MasterKeyRing::parse("2:AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=,1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap();
        let tag = old
            .custody_fingerprint(1, "binding", b"channel-one", b"low-entropy-fixture")
            .unwrap();
        assert_eq!(
            Some(tag.clone()),
            rotated.custody_fingerprint(1, "binding", b"channel-one", b"low-entropy-fixture")
        );
        for other in [
            rotated.custody_fingerprint(2, "binding", b"channel-one", b"low-entropy-fixture"),
            old.custody_fingerprint(1, "another-purpose", b"channel-one", b"low-entropy-fixture"),
            old.custody_fingerprint(1, "binding", b"channel-two", b"low-entropy-fixture"),
            old.custody_fingerprint(1, "binding", b"channel-one", b"rotated-fixture"),
        ] {
            assert_ne!(Some(tag.clone()), other);
        }
        assert!(old
            .custody_fingerprint(2, "binding", b"context", b"secret")
            .is_none());
        assert!(old
            .custody_fingerprint(1, "", b"context", b"secret")
            .is_none());
        assert_eq!(tag.len(), 64);
    }
}
