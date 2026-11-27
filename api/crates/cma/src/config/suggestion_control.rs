//! Opt-in timing control for one suggestion answer on staging.
const SWITCH: &str = "CHRONO_STAGING_SUGGESTION_CONTROL";

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Mode {
    PauseFirst,
    LateReady,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    pub sandbox_id: String,
    pub agent_id: String,
}

pub(super) fn from_lookup(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<Option<Target>, String> {
    let Some(value) = lookup(SWITCH)? else {
        return Ok(None);
    };
    if lookup("CHRONO_ENVIRONMENT")?.as_deref() != Some("staging") {
        return Ok(None);
    }
    let invalid = || format!("{SWITCH} requires exactly one Sandbox ID and one Agent ID");
    let (sandbox, agent) = value.split_once(',').ok_or_else(invalid)?;
    if !id(sandbox, "sbx_") || !id(agent, "agt_") {
        return Err(invalid());
    }
    Ok(Some(Target {
        sandbox_id: sandbox.into(),
        agent_id: agent.into(),
    }))
}

fn id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|suffix| {
        suffix.len() == 32
            && suffix
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str =
        "sbx_00000000000000000000000000000001,agt_00000000000000000000000000000002";

    fn load(environment: Option<&str>, config: Option<&str>) -> Result<Option<Target>, String> {
        from_lookup(&mut |key| {
            Ok(match key {
                SWITCH => config,
                "CHRONO_ENVIRONMENT" => environment,
                _ => panic!("unexpected setting {key}"),
            }
            .map(str::to_owned))
        })
    }

    #[test]
    fn suggestion_control_defaults_off_and_requires_staging() {
        assert!(load(Some("staging"), None).unwrap().is_none());
        for environment in [None, Some("production"), Some("Staging")] {
            assert!(load(environment, Some(FIXTURE)).unwrap().is_none());
        }
        let target = load(Some("staging"), Some(FIXTURE)).unwrap().unwrap();
        assert!(target.sandbox_id.starts_with("sbx_"));
        assert!(target.agent_id.starts_with("agt_"));
    }

    #[test]
    fn suggestion_control_rejects_ambiguous_or_invalid_scope() {
        for value in [
            "".to_owned(),
            "*".to_owned(),
            format!("{FIXTURE},10"),
            FIXTURE.replace("agt_", "sbx_"),
        ] {
            assert!(load(Some("staging"), Some(&value)).is_err());
        }
    }
}
