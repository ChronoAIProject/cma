//! Stable wire names for retained authorization capabilities.

use layer2_core::{Authorization, Capability};

pub(crate) fn wire_name(capability: Capability) -> &'static str {
    match capability {
        Capability::ReadOwnResources => "read_own_resources",
        Capability::ManageOwnResources => "manage_own_resources",
        Capability::ReadActivity => "read_activity",
        Capability::ReadDirectory => "read_directory",
        Capability::ReadAllResources => "read_all_resources",
        Capability::ConfigurePlatform => "configure_platform",
    }
}

pub(crate) fn reported(authorization: &Authorization) -> Vec<&'static str> {
    authorization.iter().map(wire_name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_names_are_distinct() {
        let mut names = vec![
            Capability::ReadOwnResources,
            Capability::ManageOwnResources,
            Capability::ReadActivity,
            Capability::ReadDirectory,
            Capability::ReadAllResources,
            Capability::ConfigurePlatform,
        ]
        .into_iter()
        .map(wire_name)
        .collect::<Vec<_>>();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 6);
    }

    #[test]
    fn reported_authority_is_stable_and_exact() {
        let authority =
            Authorization::new([Capability::ReadDirectory, Capability::ReadOwnResources]);
        assert_eq!(
            reported(&authority),
            ["read_own_resources", "read_directory"]
        );
    }
}
