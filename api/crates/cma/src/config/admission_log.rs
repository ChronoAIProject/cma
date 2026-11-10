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
