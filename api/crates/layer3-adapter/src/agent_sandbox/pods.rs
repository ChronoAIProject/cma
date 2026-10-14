//! The pinned upstream controller stamps this tracking label on every Pod,
//! including unclaimed warm members. Claim metadata reconciliation preserves it.
//! A hash is a discovery hint only; the Sandbox owner UID establishes identity.
pub(super) const TRACKING_LABEL: &str = "agents.x-k8s.io/sandbox-name-hash";

pub(super) fn name_hash(name: &str) -> String {
    // kubernetes-sigs/agent-sandbox controllers.NameHash uses 32-bit FNV-1a.
    let hash = name.bytes().fold(2_166_136_261u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(16_777_619)
    });
    format!("{hash:08x}")
}

pub(super) fn selector(name: &str) -> String {
    format!("{TRACKING_LABEL}={}", name_hash(name))
}

#[cfg(test)]
mod tests {
    #[test]
    fn matches_upstream_fnv1a_fixed_width_hex() {
        for (name, expected) in [("", "811c9dc5"), ("hello", "4f9f2cab"), ("a", "e40c292c")] {
            assert_eq!(super::name_hash(name), expected);
        }
    }
}
