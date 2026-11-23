//! Isolated-validation policy. Disabled by default; this does not resize a pool.
use super::runtime_capacity::number;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ControlAdmissionConfig {
    pub enabled: bool,
    /// Sizing envelope only: HTTP containment does not gate other pool users.
    pub other_credits: usize,
    pub stop_credits: usize,
    pub delete_credits: usize,
    /// Reserved in the sizing model; not a claim of measured spare connections.
    pub spare_credits: usize,
    pub stop_per_minute: u32,
    pub delete_per_minute: u32,
    pub burst: u32,
    pub max_callers: usize,
}

impl Default for ControlAdmissionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            other_credits: 18,
            stop_credits: 2,
            delete_credits: 4,
            spare_credits: 6,
            stop_per_minute: 6,
            delete_per_minute: 3,
            burst: 1,
            max_callers: 1024,
        }
    }
}

impl ControlAdmissionConfig {
    pub(super) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Self, String> {
        let enabled = match lookup("CHRONO_AGENT_CONTROL_ADMISSION_ENABLED")?.as_deref() {
            None | Some("false") => false,
            Some("true") => true,
            _ => return Err("CHRONO_AGENT_CONTROL_ADMISSION_ENABLED must be true or false".into()),
        };
        let config = Self {
            enabled,
            other_credits: number(lookup, "CHRONO_AGENT_CONTROL_OTHER_CREDITS", 18, 1, 18)?,
            stop_credits: number(lookup, "CHRONO_AGENT_CONTROL_STOP_CREDITS", 2, 1, 2)?,
            delete_credits: number(lookup, "CHRONO_AGENT_CONTROL_DELETE_CREDITS", 4, 1, 4)?,
            spare_credits: number(lookup, "CHRONO_AGENT_CONTROL_SPARE_CREDITS", 6, 6, 30)?,
            stop_per_minute: number(lookup, "CHRONO_AGENT_CONTROL_STOP_PER_MINUTE", 6, 1, 6)?
                as u32,
            delete_per_minute: number(lookup, "CHRONO_AGENT_CONTROL_DELETE_PER_MINUTE", 3, 1, 3)?
                as u32,
            burst: number(lookup, "CHRONO_AGENT_CONTROL_BURST", 1, 1, 1)? as u32,
            max_callers: number(lookup, "CHRONO_AGENT_CONTROL_MAX_CALLERS", 1024, 1, 65536)?,
        };
        if config.other_credits + config.stop_credits + config.delete_credits + config.spare_credits
            > 30
        {
            return Err(
                "Agent control credit partition exceeds the verified pool budget of 30".into(),
            );
        }
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_off_preserves_the_isolated_pool_partition() {
        let config = ControlAdmissionConfig::from_lookup(&mut |_| Ok(None)).unwrap();
        assert_eq!(config, ControlAdmissionConfig::default());
        assert_eq!(
            config.other_credits
                + config.stop_credits
                + config.delete_credits
                + config.spare_credits,
            30
        );
        let enabled = ControlAdmissionConfig::from_lookup(&mut |key| {
            Ok((key == "CHRONO_AGENT_CONTROL_ADMISSION_ENABLED").then(|| "true".into()))
        })
        .unwrap();
        assert!(enabled.enabled);
    }

    #[test]
    fn malformed_and_unsafe_policy_values_are_rejected() {
        for (key, values) in [
            (
                "CHRONO_AGENT_CONTROL_ADMISSION_ENABLED",
                &["1", "TRUE", " true"] as &[_],
            ),
            (
                "CHRONO_AGENT_CONTROL_STOP_CREDITS",
                &["0", "3", "-1", "+1", " 1", "18446744073709551616"],
            ),
            ("CHRONO_AGENT_CONTROL_DELETE_CREDITS", &["0", "5"]),
            ("CHRONO_AGENT_CONTROL_OTHER_CREDITS", &["0", "19"]),
            ("CHRONO_AGENT_CONTROL_SPARE_CREDITS", &["5", "7"]),
            ("CHRONO_AGENT_CONTROL_STOP_PER_MINUTE", &["0", "7"]),
            ("CHRONO_AGENT_CONTROL_DELETE_PER_MINUTE", &["0", "4"]),
            ("CHRONO_AGENT_CONTROL_BURST", &["0", "2"]),
            ("CHRONO_AGENT_CONTROL_MAX_CALLERS", &["0", "65537"]),
        ] {
            for value in values {
                assert!(
                    ControlAdmissionConfig::from_lookup(&mut |name| Ok(
                        (name == key).then(|| (*value).into())
                    ))
                    .is_err(),
                    "{key}={value}"
                );
            }
        }
    }
}
