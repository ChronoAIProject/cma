//! Bounded diagnostics for a relay callback envelope that failed to parse.
//!
//! serde messages embed offending values (`invalid type: string "…"`), and an
//! authenticated NyxID body carries user content and credentials, so only a
//! validated field name and the error category are ever logged; the error's
//! text never is. The field name is what an operator needs to compare NyxID's
//! serializer with the envelope after a rejection, which NyxID never retries.
use super::VerificationError;

/// The longest field name worth logging; longer names are reported as `-`.
const MAX_FIELD_NAME: usize = 64;

pub(super) fn rejected_envelope(error: &serde_json::Error) -> VerificationError {
    let message = error.to_string();
    tracing::warn!(
        field = %unknown_field(&message).unwrap_or("-"),
        category = ?error.classify(),
        "relay callback envelope rejected"
    );
    VerificationError("payload_invalid")
}

/// The name in serde's ``unknown field `name` `` message, only when it is a plain
/// identifier, so no sender-controlled text reaches the log.
fn unknown_field(message: &str) -> Option<&str> {
    let (name, _) = message.strip_prefix("unknown field `")?.split_once('`')?;
    let plain = !name.is_empty()
        && name.len() <= MAX_FIELD_NAME
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    plain.then_some(name)
}

#[cfg(test)]
mod tests {
    use super::{rejected_envelope, unknown_field};

    #[test]
    fn only_a_plain_unknown_field_name_is_reported() {
        let message = "unknown field `surprise`, expected one of `message_id` at line 1 column 9";
        assert_eq!(unknown_field(message), Some("surprise"));
        let longest = "f".repeat(64);
        let message = format!("unknown field `{longest}`, expected `a`");
        assert_eq!(unknown_field(&message), Some(longest.as_str()));
    }

    #[test]
    fn names_that_are_not_plain_identifiers_are_withheld() {
        for message in [
            "unknown field `two words`, expected `a`".to_owned(),
            format!("unknown field `{}`, expected `a`", "f".repeat(65)),
            "unknown field ``, expected `a`".to_owned(),
            "unknown field `unterminated".to_owned(),
            "unknown field `a-b`, expected `a`".to_owned(),
            "invalid type: string \"unknown field `x`\", expected u64".to_owned(),
            "missing field `message_id` at line 1 column 2".to_owned(),
        ] {
            assert_eq!(unknown_field(&message), None, "{message}");
        }
    }

    #[test]
    fn every_parse_failure_is_payload_invalid() {
        for body in [r#"{"surprise":1}"#, "not json", r#"{"message_id":7}"#] {
            let error = serde_json::from_str::<super::super::payload::CallbackPayload>(body)
                .err()
                .unwrap();
            assert_eq!(rejected_envelope(&error).0, "payload_invalid");
        }
    }
}
