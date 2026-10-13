//! Masking applied when a console observation is built, so raw identifiers
//! never enter the observation queue or any console structure.
use sha2::{Digest, Sha256};

/// Characters an actor or chat id keeps.
const ID_TAIL: usize = 4;
/// Characters a hash keeps after `sha256:…`.
const HASH_TAIL: usize = 8;
/// Characters a display name keeps.
const NAME_CHARS: usize = 32;
/// Hex characters of an event key.
const EVENT_KEY_HEX: usize = 16;
/// Bytes a producer's stripped `ce.source` keeps.
const SOURCE_BYTES: usize = 256;
/// Bytes a bounded label, such as a refusal reason, keeps.
const LABEL_BYTES: usize = 64;

/// `…` and the last `keep` characters of `value`; only `…` when the value is
/// not longer than that, so a short id is never shown whole.
fn tail(value: &str, keep: usize) -> String {
    let count = value.chars().count();
    if count <= keep {
        return "…".to_owned();
    }
    let kept: String = value.chars().skip(count - keep).collect();
    format!("…{kept}")
}

/// An actor id as `…<last 4>`.
pub fn actor(id: &str) -> String {
    tail(id, ID_TAIL)
}

/// A chat id as `…<last 4>`.
pub fn chat(id: &str) -> String {
    tail(id, ID_TAIL)
}

/// A display name cut to its first 32 characters.
pub fn display_name(name: &str) -> String {
    name.chars().take(NAME_CHARS).collect()
}

/// The first 16 hex characters of SHA-256(`event_id`): how the console names
/// an event without holding its id.
pub fn event_key(event_id: &str) -> String {
    let digest = Sha256::digest(event_id.as_bytes());
    let mut key = String::with_capacity(EVENT_KEY_HEX);
    for byte in &digest[..EVENT_KEY_HEX / 2] {
        key.push_str(&format!("{byte:02x}"));
    }
    key
}

/// A `sha256:<hex>` hash as `sha256:…<last 8>`.
pub fn hash(value: &str) -> String {
    let hex = value.strip_prefix("sha256:").unwrap_or(value);
    format!("sha256:{}", tail(hex, HASH_TAIL))
}

/// Cut `value` to at most `max` bytes on a character boundary.
pub fn truncate(value: &str, max: usize) -> String {
    let mut end = value.len().min(max);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

/// A `ce.source` without userinfo, query or fragment, cut to 256 bytes: the
/// producer's identity without any credential or parameter it may carry.
pub fn source_uri(raw: &str) -> String {
    let end = raw.find(['?', '#']).unwrap_or(raw.len());
    let reference = &raw[..end];
    // Only a hierarchical reference has an authority that can hold userinfo:
    // `scheme://user@host/…` or `//user@host/…`.
    let authority = reference.find("//").filter(|&start| {
        let scheme = &reference[..start];
        scheme.is_empty()
            || scheme.strip_suffix(':').is_some_and(|name| {
                name.starts_with(|first: char| first.is_ascii_alphabetic())
                    && name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
            })
    });
    let stripped = match authority {
        Some(start) => {
            let host_start = start + 2;
            let rest = &reference[host_start..];
            let host_end = rest.find('/').unwrap_or(rest.len());
            match rest[..host_end].rfind('@') {
                Some(at) => format!("{}{}", &reference[..host_start], &rest[at + 1..]),
                None => reference.to_owned(),
            }
        }
        None => reference.to_owned(),
    };
    truncate(&stripped, SOURCE_BYTES)
}

/// A gateway-chosen label kept as is when it is a bounded snake_case word,
/// otherwise `other`, so no free text from a platform or a sender is kept.
pub fn label(value: &str) -> String {
    let valid = !value.is_empty()
        && value.len() <= LABEL_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
    if valid {
        value.to_owned()
    } else {
        "other".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_keep_only_their_last_four_characters() {
        assert_eq!(actor("123456789"), "…6789");
        assert_eq!(chat("-1001234567890"), "…7890");
        assert_eq!(actor("ou_ünïcødé"), "…cødé");
        // A short id is never shown whole.
        assert_eq!(actor("1234"), "…");
        assert_eq!(chat(""), "…");
    }

    #[test]
    fn names_are_cut_to_32_characters() {
        assert_eq!(display_name("short"), "short");
        let long = "é".repeat(40);
        assert_eq!(display_name(&long).chars().count(), 32);
    }

    #[test]
    fn event_keys_are_16_hex_of_the_event_id_digest() {
        let key = event_key("evt-1");
        assert_eq!(key.len(), 16);
        assert!(key.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(key, event_key("evt-1"));
        assert_ne!(key, event_key("evt-2"));
        assert!(!key.contains("evt"));
    }

    #[test]
    fn hashes_keep_their_last_eight_hex_characters() {
        let hash_value = format!("sha256:{}", "ab".repeat(32));
        assert_eq!(hash(&hash_value), "sha256:…abababab");
        assert_eq!(hash("sha256:abc"), "sha256:…");
    }

    #[test]
    fn sources_lose_userinfo_query_and_fragment() {
        assert_eq!(
            source_uri("https://user:pw@sensors.example/a/b?token=x#frag"),
            "https://sensors.example/a/b"
        );
        assert_eq!(source_uri("//user@host/path?q"), "//host/path");
        assert_eq!(source_uri("/relative/path?q=1"), "/relative/path");
        assert_eq!(source_uri("urn:device:42#part"), "urn:device:42");
        // An `@` in the path is not userinfo.
        assert_eq!(
            source_uri("https://host.example/users/@me"),
            "https://host.example/users/@me"
        );
        assert_eq!(source_uri(&format!("/{}", "a".repeat(400))).len(), 256);
    }

    #[test]
    fn labels_keep_only_bounded_snake_case_words() {
        assert_eq!(label("group_not_addressed"), "group_not_addressed");
        assert_eq!(label("Telegram said: chat not found"), "other");
        assert_eq!(label(""), "other");
        assert_eq!(label(&"a".repeat(65)), "other");
    }

    #[test]
    fn truncation_respects_character_boundaries() {
        assert_eq!(truncate("héllo", 2), "h");
        assert_eq!(truncate("hello", 10), "hello");
    }
}
