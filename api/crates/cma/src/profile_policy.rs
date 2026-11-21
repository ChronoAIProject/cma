//! Build-owned validation policy for profile drafts and publication.

use layer1_public::rest::ProfileRoutePolicy;
use layer2_core::profiles::ProfileCapacityBounds;
use std::sync::Arc;

mod secret_scanner;

/// Resource range accepted by this deployment's supported sandbox provider.
/// The default profile reservation sits within this range, but does not define
/// it; changing the reservation must not silently widen publication policy.
const PROFILE_CAPACITY_BOUNDS: ProfileCapacityBounds = ProfileCapacityBounds {
    min_cpu_millis: 100,
    max_cpu_millis: 8_000,
    min_memory_mib: 128,
    max_memory_mib: 16_384,
};

pub(crate) fn profile_route_policy() -> ProfileRoutePolicy {
    ProfileRoutePolicy {
        capacity_bounds: PROFILE_CAPACITY_BOUNDS,
        public_value_scanner: Arc::new(secret_scanner::GitleaksScanner::production()),
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
        assert!(format!("{:?}", policy.public_value_scanner).contains("GitleaksScanner"));
    }
}
