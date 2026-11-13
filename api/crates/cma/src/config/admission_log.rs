//! Exact Agent cohort; an empty list keeps admission on full validation.
pub(super) fn from_lookup(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<Vec<layer2_core::AgentId>, String> {
    let value = lookup("CHRONO_AGENT_LOG_ADMISSION_AGENTS")?.unwrap_or_default();
    if value.is_empty() {
        return Ok(Vec::new());
    }
    let mut ids = std::collections::BTreeSet::new();
    for id in value.split(',') {
        if !id.starts_with("agt_")
            || id.len() > 128
            || id.len() <= 4
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            || !ids.insert(layer2_core::AgentId(id.into()))
            || ids.len() > 16
        {
            return Err(
                "CHRONO_AGENT_LOG_ADMISSION_AGENTS must contain at most 16 unique Agent ids".into(),
            );
        }
    }
    Ok(ids.into_iter().collect())
}

/// Percentage stages are mutually exclusive with the explicit startup cohort.
pub(super) fn fleet_percent(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    explicit: &[layer2_core::AgentId],
) -> Result<u8, String> {
    let value = lookup("CHRONO_AGENT_LOG_FLEET_PERCENT")?.unwrap_or_else(|| "0".into());
    let percent = value
        .parse::<u8>()
        .ok()
        .filter(|n| *n <= 100 && n.to_string() == value)
        .ok_or("CHRONO_AGENT_LOG_FLEET_PERCENT must be an integer from 0 to 100")?;
    if percent > 0 && !explicit.is_empty() {
        return Err("fleet percentage and explicit Agent cohort are mutually exclusive".into());
    }
    Ok(percent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cohort_defaults_off_and_rejects_ambiguous_input() {
        assert!(from_lookup(&mut |_| Ok(None)).unwrap().is_empty());
        assert_eq!(
            from_lookup(&mut |_| Ok(Some("agt_one,agt_two".into())))
                .unwrap()
                .len(),
            2
        );
        for value in [
            "all",
            "agt_one,agt_one",
            "agt_",
            "agt_one, agt_two",
            "agt_one,",
        ] {
            assert!(from_lookup(&mut |_| Ok(Some(value.into()))).is_err());
        }
    }
}

#[cfg(test)]
mod fleet_tests {
    use super::*;

    #[test]
    fn release_stage_patches_preserve_independent_writer_and_recovery_controls() {
        const STAGES: [(&str, u8); 4] = [
            (
                include_str!("../../../../../.github/agent-log-stages/off.json"),
                0,
            ),
            (
                include_str!("../../../../../.github/agent-log-stages/10.json"),
                10,
            ),
            (
                include_str!("../../../../../.github/agent-log-stages/50.json"),
                50,
            ),
            (
                include_str!("../../../../../.github/agent-log-stages/100.json"),
                100,
            ),
        ];
        for (source, expected) in STAGES {
            let patch: serde_json::Value = serde_json::from_str(source).unwrap();
            assert_eq!(patch.as_object().unwrap().len(), 1);
            let data = patch["data"].as_object().unwrap();
            assert_eq!(data.len(), 2, "stages must change only legacy selectors");
            for (adoption, recovery) in [(false, false), (true, false), (false, true), (true, true)]
            {
                // A fleet rollback cannot close independent process dispatch
                // before its separately approved gate closure/uncertainty drain.
                let mut settings = std::collections::BTreeMap::from([
                    ("CHRONO_AGENT_LOG_ADMISSION_AGENTS", "agt_old".to_owned()),
                    ("CHRONO_AGENT_LOG_FLEET_PERCENT", "50".to_owned()),
                    ("UNRELATED_SETTING", "unchanged".to_owned()),
                    (
                        "CHRONO_AGENT_LOG_SEGMENT_ADOPTION_ENABLED",
                        adoption.to_string(),
                    ),
                    (
                        "CHRONO_AGENT_CAPACITY_RECOVERY_ENABLED",
                        recovery.to_string(),
                    ),
                ]);
                for (key, value) in data {
                    assert!(matches!(
                        key.as_str(),
                        "CHRONO_AGENT_LOG_ADMISSION_AGENTS" | "CHRONO_AGENT_LOG_FLEET_PERCENT"
                    ));
                    settings.insert(key.as_str(), value.as_str().unwrap().to_owned());
                }
                let mut lookup = |key: &str| Ok(settings.get(key).cloned());
                let explicit = from_lookup(&mut lookup).unwrap();
                assert!(explicit.is_empty());
                assert_eq!(settings["UNRELATED_SETTING"], "unchanged");
                assert_eq!(fleet_percent(&mut lookup, &explicit).unwrap(), expected);
                let flags = super::super::features::FeatureFlags::from_lookup(&mut lookup).unwrap();
                assert_eq!(flags.segment_adoption_enabled, adoption);
                assert_eq!(flags.capacity_recovery_enabled, recovery);
                assert!(!flags.bounded_response_reads);
                assert!(!flags.bounded_transcript_reads);
                assert!(!flags.bounded_usage_reads);
            }
        }
    }

    #[test]
    fn fleet_stages_default_off_and_reject_ambiguous_or_mixed_modes() {
        assert_eq!(fleet_percent(&mut |_| Ok(None), &[]).unwrap(), 0);
        for value in ["0", "10", "50", "100"] {
            assert_eq!(
                fleet_percent(&mut |_| Ok(Some(value.into())), &[])
                    .unwrap()
                    .to_string(),
                value
            );
        }
        for value in ["", "all", "01", " 10", "10%", "101", "-1", "true"] {
            assert!(fleet_percent(&mut |_| Ok(Some(value.into())), &[]).is_err());
        }
        let explicit = [layer2_core::AgentId("agt_one".into())];
        assert!(fleet_percent(&mut |_| Ok(Some("10".into())), &explicit).is_err());
        assert_eq!(fleet_percent(&mut |_| Ok(None), &explicit).unwrap(), 0);
    }
}
