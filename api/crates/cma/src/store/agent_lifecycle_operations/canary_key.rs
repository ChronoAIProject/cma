//! Server-owned operation and child profile keys for runtime canary admission.

const OPERATION_PREFIX: &str = "runtime-canary/";
const INTERNAL_KEY_BYTES: usize = 256;
const OPERATION_ID_BYTES: usize = 32;

pub(crate) fn is_reserved(key: &str) -> bool {
    key.len() == INTERNAL_KEY_BYTES && key.starts_with(OPERATION_PREFIX)
}

pub(crate) fn operation_id(key: &str) -> Option<&str> {
    if !is_reserved(key) {
        return None;
    }
    let body = key.strip_prefix(OPERATION_PREFIX)?;
    let id = body.get(..OPERATION_ID_BYTES)?;
    let padding = body.get(OPERATION_ID_BYTES..)?;
    if id
        .bytes()
        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        && padding.bytes().all(|byte| byte == b'.')
    {
        Some(id)
    } else {
        None
    }
}

pub(crate) fn profile_key(operation_id: &str, kind: &str) -> String {
    format!("runtime-canary-profile:{operation_id}:{kind}")
}

pub(super) fn admission_key(id: &str) -> String {
    let prefix = format!("{OPERATION_PREFIX}{id}");
    let padding = ".".repeat(INTERNAL_KEY_BYTES - prefix.len());
    format!("{prefix}{padding}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canary_keys_are_outside_public_admission_and_child_keys_are_bounded() {
        let id = "d".repeat(OPERATION_ID_BYTES);
        let key = admission_key(&id);
        assert_eq!(operation_id(&key), Some(id.as_str()));
        assert!(!layer2_core::operation::valid_idempotency_key(
            key.as_bytes()
        ));
        assert!(!is_reserved(&format!("{OPERATION_PREFIX}{id}")));
        let mut damaged = key;
        damaged.pop();
        damaged.push('!');
        assert!(is_reserved(&damaged));
        assert_eq!(operation_id(&damaged), None);
        for kind in ["agent", "workspace", "sandbox"] {
            let key = profile_key(&id, kind);
            assert!(key.len() < 256);
            assert_eq!(key, format!("runtime-canary-profile:{id}:{kind}"));
        }
    }
}
