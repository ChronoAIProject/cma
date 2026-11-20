//! Defaults apply only before the first durable platform settings document exists.
use layer2_core::PlatformSettings;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct InitialSettings {
    visitor_two_sandboxes: bool,
}

impl InitialSettings {
    pub(crate) fn new(visitor_two_sandboxes: bool) -> Self {
        Self {
            visitor_two_sandboxes,
        }
    }

    pub(crate) fn platform(self) -> PlatformSettings {
        let mut settings = PlatformSettings::default();
        if self.visitor_two_sandboxes {
            settings.limits.visitor_max_sandboxes = 2;
        }
        settings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opt_in_changes_only_the_initial_visitor_sandbox_allowance() {
        let off = InitialSettings::default().platform();
        assert_eq!(off.limits.visitor_max_sandboxes, 1);
        assert_eq!(off, InitialSettings::new(false).platform());
        let mut on = InitialSettings::new(true).platform();
        assert_eq!(on.limits.visitor_max_sandboxes, 2);
        assert_eq!(on.limits.visitor_max_launches_per_day, 5);
        assert!(!on.model_budget.enabled);
        on.limits.visitor_max_sandboxes = 1;
        assert_eq!(on, off);
    }
}
