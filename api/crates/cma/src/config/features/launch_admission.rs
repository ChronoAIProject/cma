use super::FeatureFlags;
use layer2_core::trigger_domain::LaunchAdmissionPolicy;

impl FeatureFlags {
    pub(crate) fn launch_admission_policy(self) -> LaunchAdmissionPolicy {
        if self.visitor_launch_admission {
            LaunchAdmissionPolicy::VisitorAllowance
        } else {
            LaunchAdmissionPolicy::Baseline
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visitor_admission_is_strict_independent_and_default_off() {
        const KEY: &str = "CHRONO_VISITOR_LAUNCH_ADMISSION_ENABLED";
        assert_eq!(
            FeatureFlags::default().launch_admission_policy(),
            LaunchAdmissionPolicy::Baseline
        );
        for (raw, expected) in [
            (None, LaunchAdmissionPolicy::Baseline),
            (Some("false"), LaunchAdmissionPolicy::Baseline),
            (Some("true"), LaunchAdmissionPolicy::VisitorAllowance),
        ] {
            let flags = FeatureFlags::from_lookup(&mut |key| {
                Ok((key == KEY).then_some(raw).flatten().map(str::to_owned))
            })
            .unwrap();
            assert_eq!(flags.launch_admission_policy(), expected);
            assert!(!flags.trigger_command_admission);
            assert!(!flags.admission_ceilings_writes_enabled);
        }
        for raw in ["", "1", "TRUE", " true "] {
            assert!(
                FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| raw.to_owned())))
                    .is_err()
            );
        }
    }
}
