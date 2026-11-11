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
