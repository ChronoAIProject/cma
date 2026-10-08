//! Idempotency keys of NyxID initiated sends (F2).
//!
//! Every part of a message gets its own key, so a multi-part message is
//! never cut short by a key NyxID already holds, and the gateway never
//! presents one key for two different texts: NyxID refuses that with a 409
//! and sends nothing. A retry of one part after a 429 reuses that part's
//! key, so NyxID can tell it from a new message.
use ring::rand::{SecureRandom, SystemRandom};
use sha2::{Digest, Sha256};

/// The byte that separates the fields of a key's digest input (ASCII US).
const SEPARATOR: u8 = 0x1f;
/// Domain separation of tool keys, versioned so a change of derivation can
/// never collide with keys an earlier release issued.
const TOOL_DOMAIN: &[u8] = b"cmaeg.tool-part.v1";
/// The prefix of a tool key; 5 + 60 characters, within NyxID's 128.
const TOOL_PREFIX: &str = "tool_";
const TOOL_DIGEST_CHARS: usize = 60;

/// What a send's per-part keys derive from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartKeys {
    /// A tool command: 32 random bytes drawn once per command, so every new
    /// tool call gets new keys. They make the transport correct; they do not
    /// deduplicate tool replies, which this release does not offer.
    Tool { seed: [u8; 32] },
    /// An automatic output of one consumer's turn. `seq` counts that
    /// consumer's automatic outputs of the event, so two committed messages
    /// get distinct keys.
    Output {
        record_id: String,
        event_id: String,
        consumer: String,
        kind: &'static str,
        seq: u32,
    },
}

impl PartKeys {
    /// The keys of one tool command, from a fresh system-random seed.
    pub fn tool() -> Result<Self, ring::error::Unspecified> {
        let mut seed = [0; 32];
        SystemRandom::new().fill(&mut seed)?;
        Ok(Self::Tool { seed })
    }

    /// The key of part `part`, whose text is `text`. A tool key is `tool_`
    /// and the first 60 hex digits of
    /// `sha256("cmaeg.tool-part.v1" ␟ seed ␟ part ␟ hex(sha256(text)))`; an
    /// output key is the 64 hex digits of
    /// `sha256(record_id ␟ event_id ␟ consumer ␟ kind ␟ seq ␟ part)`.
    pub fn key(&self, part: usize, text: &str) -> String {
        let part = part.to_string();
        match self {
            Self::Tool { seed } => {
                let text_digest = format!("{:x}", Sha256::digest(text.as_bytes()));
                let digest = digest(&[TOOL_DOMAIN, seed, part.as_bytes(), text_digest.as_bytes()]);
                format!("{TOOL_PREFIX}{}", &digest[..TOOL_DIGEST_CHARS])
            }
            Self::Output {
                record_id,
                event_id,
                consumer,
                kind,
                seq,
            } => digest(&[
                record_id.as_bytes(),
                event_id.as_bytes(),
                consumer.as_bytes(),
                kind.as_bytes(),
                seq.to_string().as_bytes(),
                part.as_bytes(),
            ]),
        }
    }
}

/// The lowercase hex SHA-256 of `fields` joined by [`SEPARATOR`].
fn digest(fields: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            hasher.update([SEPARATOR]);
        }
        hasher.update(field);
    }
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(seq: u32) -> PartKeys {
        PartKeys::Output {
            record_id: "record-1".into(),
            event_id: "msg-1".into(),
            consumer: "default".into(),
            kind: "answer",
            seq,
        }
    }

    #[test]
    fn tool_keys_are_prefixed_bounded_and_distinct_per_part_text_and_command() {
        let keys = PartKeys::Tool { seed: [7; 32] };
        let first = keys.key(0, "one");
        assert!(first.starts_with("tool_"));
        assert_eq!(first.len(), 65);
        assert!(first[5..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
        assert_eq!(first, keys.key(0, "one"), "a retried part reuses its key");
        assert_ne!(first, keys.key(1, "one"));
        assert_ne!(first, keys.key(0, "two"));
        assert_ne!(first, PartKeys::Tool { seed: [8; 32] }.key(0, "one"));
        let (drawn, again) = (PartKeys::tool().unwrap(), PartKeys::tool().unwrap());
        assert_ne!(drawn, again, "every command draws its own seed");
        assert_ne!(drawn.key(0, "one"), again.key(0, "one"));
    }

    #[test]
    fn output_keys_are_64_hex_and_distinct_per_sequence_kind_and_part() {
        let key = output(0).key(0, "ignored");
        assert_eq!(key.len(), 64);
        assert!(key.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(key, output(0).key(0, "other text"));
        assert_ne!(key, output(1).key(0, "ignored"));
        assert_ne!(key, output(0).key(1, "ignored"));
        let mut failure = output(0);
        if let PartKeys::Output { kind, .. } = &mut failure {
            *kind = "failure";
        }
        assert_ne!(key, failure.key(0, "ignored"));
        // Fields are separated, so moving a boundary changes the key.
        let shifted = PartKeys::Output {
            record_id: "record-1msg".into(),
            event_id: "-1".into(),
            consumer: "default".into(),
            kind: "answer",
            seq: 0,
        };
        assert_ne!(key, shifted.key(0, "ignored"));
    }
}
