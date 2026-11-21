use std::collections::BTreeMap;

const HASH_KEYS: [&str; 3] = ["cma/image-sha256", "cma/spec-sha256", "cma/home-identity"];
const LABEL_PREFIX: &str = "h.";

fn is_hash(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

// Keep durable request fingerprints unchanged; only the provider wire needs label encoding.
pub(super) fn encode(metadata: &mut BTreeMap<String, String>) {
    for key in HASH_KEYS {
        if let Some(value) = metadata.get_mut(key) {
            if is_hash(value) && value.starts_with(['-', '_']) {
                *value = format!("{LABEL_PREFIX}{value}");
            }
        }
    }
}

pub(super) fn decode(metadata: &mut BTreeMap<String, String>) {
    for key in HASH_KEYS {
        if let Some(value) = metadata.get_mut(key) {
            if let Some(original) = value.strip_prefix(LABEL_PREFIX) {
                if is_hash(original) && original.starts_with(['-', '_']) {
                    *value = original.to_owned();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_hash_leading_characters_round_trip_without_changing_existing_labels() {
        for first in "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_".chars() {
            let hash = format!("{first}{}", "a".repeat(42));
            let original = BTreeMap::from([
                (HASH_KEYS[0].into(), hash.clone()),
                (HASH_KEYS[1].into(), hash.clone()),
                (HASH_KEYS[2].into(), hash.clone()),
                ("cma/sandbox".into(), "sbx_example".into()),
            ]);
            let mut wire = original.clone();
            encode(&mut wire);
            for value in wire.values() {
                assert!(value.len() <= 63);
                assert!(value.as_bytes()[0].is_ascii_alphanumeric());
                assert!(value.as_bytes()[value.len() - 1].is_ascii_alphanumeric());
            }
            if first.is_ascii_alphanumeric() {
                assert_eq!(wire, original);
            }
            decode(&mut wire);
            assert_eq!(wire, original);
            decode(&mut wire);
            assert_eq!(wire, original);
        }
    }

    #[test]
    fn original_home_identity_remains_stable_across_provider_label_encoding() {
        // The production canary's original home hash was rejected as a label.
        let identity = "-Unm6TY9hp2g63W90nzM978QwSNBSowv0CK2bVf3GxM";
        let original = BTreeMap::from([("cma/home-identity".into(), identity.into())]);
        let mut wire = original.clone();
        encode(&mut wire);
        assert_eq!(wire["cma/home-identity"], format!("h.{identity}"));
        decode(&mut wire);
        assert_eq!(wire, original);
    }
}
