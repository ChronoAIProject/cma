//! Exact Sandbox selectors for operator-only staging qualification.

use layer2_core::SandboxId;

const DROP_OPEN_REPLY: &str = "CHRONO_STAGING_DROP_ACP_OPEN_REPLY_FOR_SANDBOX";
const PARK_RUNTIME_UPDATE: &str = "CHRONO_STAGING_PARK_RUNTIME_UPDATE_FOR_SANDBOX";

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct StagingSandboxControls {
    pub open_reply_loss: Option<String>,
    pub park_runtime_update: Option<SandboxId>,
}

impl StagingSandboxControls {
    pub(super) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Self, String> {
        Ok(Self {
            open_reply_loss: sandbox_from_lookup(lookup, DROP_OPEN_REPLY)?,
            park_runtime_update: sandbox_from_lookup(lookup, PARK_RUNTIME_UPDATE)?.map(SandboxId),
        })
    }
}

fn sandbox_from_lookup(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    setting: &str,
) -> Result<Option<String>, String> {
    let Some(sandbox_id) = lookup(setting)? else {
        return Ok(None);
    };
    if lookup("CHRONO_ENVIRONMENT")?.as_deref() != Some("staging") {
        return Ok(None);
    }
    if !sandbox_id.strip_prefix("sbx_").is_some_and(|id| {
        id.len() == 32
            && id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    }) {
        return Err(format!("{setting} must specify one exact Sandbox ID"));
    }
    Ok(Some(sandbox_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "sbx_00000000000000000000000000000001";
    fn load(environment: Option<&str>, switch: Option<&str>) -> Result<Option<SandboxId>, String> {
        StagingSandboxControls::from_lookup(&mut |key| {
            Ok(match key {
                PARK_RUNTIME_UPDATE => switch,
                DROP_OPEN_REPLY => None,
                "CHRONO_ENVIRONMENT" => environment,
                _ => panic!("unexpected key {key}"),
            }
            .map(str::to_owned))
        })
        .map(|controls| {
            assert!(controls.open_reply_loss.is_none());
            controls.park_runtime_update
        })
    }

    #[test]
    fn sandbox_selector_defaults_off_and_requires_staging() {
        assert_eq!(load(Some("staging"), None).unwrap(), None);
        for environment in [
            None,
            Some("production"),
            Some("development"),
            Some("Staging"),
        ] {
            assert_eq!(load(environment, Some(FIXTURE)).unwrap(), None);
        }
        assert_eq!(
            load(Some("staging"), Some(FIXTURE)).unwrap(),
            Some(SandboxId(FIXTURE.into()))
        );
    }

    #[test]
    fn sandbox_selector_rejects_empty_wildcard_and_multiple_targets() {
        for value in ["", "*", "sbx_*", &format!("{FIXTURE},{FIXTURE}")] {
            assert!(load(Some("staging"), Some(value)).is_err());
        }
    }

    #[test]
    fn staging_controls_select_their_own_sandboxes() {
        let other = "sbx_00000000000000000000000000000002";
        let controls = StagingSandboxControls::from_lookup(&mut |key| {
            Ok(Some(
                match key {
                    DROP_OPEN_REPLY => other,
                    PARK_RUNTIME_UPDATE => FIXTURE,
                    "CHRONO_ENVIRONMENT" => "staging",
                    _ => panic!("unexpected key {key}"),
                }
                .into(),
            ))
        })
        .unwrap();
        assert_eq!(controls.open_reply_loss.as_deref(), Some(other));
        assert_eq!(
            controls.park_runtime_update,
            Some(SandboxId(FIXTURE.into()))
        );
    }
}
