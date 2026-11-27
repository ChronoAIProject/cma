//! Exact Sandbox selectors for operator-only staging qualification.

pub(super) fn sandbox_from_lookup(
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
    const SETTING: &str = "CHRONO_STAGING_PARK_RUNTIME_UPDATE_FOR_SANDBOX";

    fn load(environment: Option<&str>, switch: Option<&str>) -> Result<Option<String>, String> {
        sandbox_from_lookup(
            &mut |key| {
                Ok(match key {
                    SETTING => switch,
                    "CHRONO_ENVIRONMENT" => environment,
                    _ => panic!("unexpected key {key}"),
                }
                .map(str::to_owned))
            },
            SETTING,
        )
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
            load(Some("staging"), Some(FIXTURE)).unwrap().as_deref(),
            Some(FIXTURE)
        );
    }

    #[test]
    fn sandbox_selector_rejects_empty_wildcard_and_multiple_targets() {
        for value in ["", "*", "sbx_*", &format!("{FIXTURE},{FIXTURE}")] {
            assert!(load(Some("staging"), Some(value)).is_err());
        }
    }
}
