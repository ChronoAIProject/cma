use layer2_core::profiles::ThinkingEffort;

use super::value_or;

const DEFAULT_SANDBOX_CPU: &str = "1";
const DEFAULT_SANDBOX_MEMORY: &str = "1Gi";
const DEFAULT_AGENT_MODEL: &str = "gpt-6";
const DEFAULT_AGENT_EFFORT: &str = "high";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DefaultProfileConfig {
    pub cpu_millis: u32,
    pub memory_mib: u32,
    pub agent_model: String,
    pub agent_effort: ThinkingEffort,
}

impl DefaultProfileConfig {
    pub(super) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Self, String> {
        let cpu = value_or(lookup, "CHRONO_SANDBOX_CPU", DEFAULT_SANDBOX_CPU)?;
        let memory = value_or(lookup, "CHRONO_SANDBOX_MEMORY", DEFAULT_SANDBOX_MEMORY)?;
        let agent_model = value_or(lookup, "CHRONO_DEFAULT_AGENT_MODEL", DEFAULT_AGENT_MODEL)?;
        if agent_model.trim().is_empty() {
            return Err("CHRONO_DEFAULT_AGENT_MODEL must not be blank".into());
        }
        let effort = value_or(lookup, "CHRONO_DEFAULT_AGENT_EFFORT", DEFAULT_AGENT_EFFORT)?;
        Ok(Self {
            cpu_millis: cpu_millis(&cpu, "CHRONO_SANDBOX_CPU")?,
            memory_mib: memory_mib(&memory, "CHRONO_SANDBOX_MEMORY")?,
            agent_model,
            agent_effort: thinking_effort(&effort)?,
        })
    }
}

/// New inline environments are independent of retained deployment-owned Profiles.
pub(super) fn agent_manager_capacity(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<layer3_adapter::SandboxCapacity, String> {
    const CPU: &str = "CHRONO_AGENT_MANAGER_SANDBOX_CPU";
    const MEMORY: &str = "CHRONO_AGENT_MANAGER_SANDBOX_MEMORY";
    Ok(layer3_adapter::SandboxCapacity {
        cpu_millis: cpu_millis(&value_or(lookup, CPU, "2")?, CPU)?,
        memory_mib: memory_mib(&value_or(lookup, MEMORY, "4Gi")?, MEMORY)?,
    })
}

fn positive(raw: &str, key: &str) -> Result<u32, String> {
    raw.parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("{key} must be a positive Kubernetes quantity"))
}

fn cpu_millis(raw: &str, key: &str) -> Result<u32, String> {
    if let Some(millis) = raw.strip_suffix('m') {
        positive(millis, key)
    } else {
        positive(raw, key)?
            .checked_mul(1_000)
            .ok_or_else(|| format!("{key} exceeds the supported CPU range"))
    }
}

fn memory_mib(raw: &str, key: &str) -> Result<u32, String> {
    if let Some(mib) = raw.strip_suffix("Mi") {
        positive(mib, key)
    } else if let Some(gib) = raw.strip_suffix("Gi") {
        positive(gib, key)?
            .checked_mul(1_024)
            .ok_or_else(|| format!("{key} exceeds the supported memory range"))
    } else {
        Err(format!("{key} must be a positive whole Mi or Gi quantity"))
    }
}

fn thinking_effort(raw: &str) -> Result<ThinkingEffort, String> {
    match raw {
        "minimal" => Ok(ThinkingEffort::Minimal),
        "low" => Ok(ThinkingEffort::Low),
        "medium" => Ok(ThinkingEffort::Medium),
        "high" => Ok(ThinkingEffort::High),
        "x_high" => Ok(ThinkingEffort::XHigh),
        _ => Err(concat!(
            "CHRONO_DEFAULT_AGENT_EFFORT must be one of ",
            "minimal, low, medium, high, x_high"
        )
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn configured(values: &[(&str, &str)]) -> Result<DefaultProfileConfig, String> {
        let values: HashMap<_, _> = values.iter().copied().collect();
        DefaultProfileConfig::from_lookup(&mut |key| Ok(values.get(key).map(ToString::to_string)))
    }

    #[test]
    fn defaults_and_supported_quantities_are_exact() {
        assert_eq!(
            configured(&[]).unwrap(),
            DefaultProfileConfig {
                cpu_millis: 1_000,
                memory_mib: 1_024,
                agent_model: "gpt-6".into(),
                agent_effort: ThinkingEffort::High,
            }
        );
        let custom = configured(&[
            ("CHRONO_SANDBOX_CPU", "750m"),
            ("CHRONO_SANDBOX_MEMORY", "2048Mi"),
            ("CHRONO_DEFAULT_AGENT_MODEL", "model-a"),
            ("CHRONO_DEFAULT_AGENT_EFFORT", "x_high"),
        ])
        .unwrap();
        assert_eq!(custom.cpu_millis, 750);
        assert_eq!(custom.memory_mib, 2_048);
        assert_eq!(custom.agent_effort, ThinkingEffort::XHigh);
    }

    #[test]
    fn malformed_or_lossy_quantities_and_agent_defaults_are_refused() {
        for (key, value) in [
            ("CHRONO_SANDBOX_CPU", "0"),
            ("CHRONO_SANDBOX_CPU", "0.5"),
            ("CHRONO_SANDBOX_MEMORY", "1.5Gi"),
            ("CHRONO_SANDBOX_MEMORY", "1024"),
            ("CHRONO_DEFAULT_AGENT_MODEL", "  "),
            ("CHRONO_DEFAULT_AGENT_EFFORT", "maximum"),
        ] {
            assert!(
                configured(&[(key, value)]).is_err(),
                "accepted {key}={value}"
            );
        }
    }

    #[test]
    fn inline_capacity_is_independent_of_retained_system_profile_configuration() {
        let capacity = |values: &[(&str, &str)]| {
            let values: HashMap<_, _> = values.iter().copied().collect();
            agent_manager_capacity(&mut |key| Ok(values.get(key).map(ToString::to_string)))
        };
        let defaults = capacity(&[
            ("CHRONO_SANDBOX_CPU", "750m"),
            ("CHRONO_SANDBOX_MEMORY", "512Mi"),
        ])
        .unwrap();
        assert_eq!(defaults.cpu_millis, 2_000);
        assert_eq!(defaults.memory_mib, 4_096);
        let custom = capacity(&[
            ("CHRONO_AGENT_MANAGER_SANDBOX_CPU", "3000m"),
            ("CHRONO_AGENT_MANAGER_SANDBOX_MEMORY", "8192Mi"),
        ])
        .unwrap();
        assert_eq!(custom.cpu_millis, 3_000);
        assert_eq!(custom.memory_mib, 8_192);
        for (key, value) in [
            ("CHRONO_AGENT_MANAGER_SANDBOX_CPU", "0"),
            ("CHRONO_AGENT_MANAGER_SANDBOX_CPU", "0.5"),
            ("CHRONO_AGENT_MANAGER_SANDBOX_MEMORY", "1.5Gi"),
            ("CHRONO_AGENT_MANAGER_SANDBOX_MEMORY", "1024"),
        ] {
            assert!(capacity(&[(key, value)]).unwrap_err().contains(key));
        }
    }
}
