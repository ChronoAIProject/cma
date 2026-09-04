//! Build-owned validation policy for profile drafts and publication.

use layer1_public::rest::ProfileRoutePolicy;
use layer2_core::profiles::ProfileCapacityBounds;

/// Resource range accepted by this deployment's supported sandbox provider.
/// The default profile reservation sits within this range, but does not define
/// it; changing the reservation must not silently widen publication policy.
const PROFILE_CAPACITY_BOUNDS: ProfileCapacityBounds = ProfileCapacityBounds {
    min_cpu_millis: 100,
    max_cpu_millis: 8_000,
    min_memory_mib: 128,
    max_memory_mib: 16_384,
};

/// High-confidence credential markers mirrored from the gitleaks default rule
/// vocabulary. Public environment values are plain configuration, so false
/// positives are intentionally resolved by changing the value, never by an
/// allowlist.
const PUBLIC_PROFILE_SECRET_PATTERNS: &[&str] = &[
    "-----BEGIN PRIVATE KEY-----",
    "-----BEGIN RSA PRIVATE KEY-----",
    "AKIA",
    "ASIA",
    "AIza",
    "github_pat_",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "glpat-",
    "sk_live_",
    "sk-proj-",
    "xoxb-",
    "xoxp-",
];

pub(crate) fn profile_route_policy() -> ProfileRoutePolicy {
    ProfileRoutePolicy {
        capacity_bounds: PROFILE_CAPACITY_BOUNDS,
        secret_patterns: PUBLIC_PROFILE_SECRET_PATTERNS
            .iter()
            .map(|pattern| (*pattern).to_owned())
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deployment_policy_is_bounded_and_never_disables_secret_detection() {
        let policy = profile_route_policy();
        assert!(policy.capacity_bounds.min_cpu_millis > 0);
        assert!(policy.capacity_bounds.min_memory_mib > 0);
        assert!(policy.capacity_bounds.min_cpu_millis <= policy.capacity_bounds.max_cpu_millis);
        assert!(policy.capacity_bounds.min_memory_mib <= policy.capacity_bounds.max_memory_mib);
        assert!(!policy.secret_patterns.is_empty());
        assert!(policy
            .secret_patterns
            .iter()
            .all(|pattern| !pattern.is_empty()));
    }
}
