use layer3_adapter::runtime_bootstrap::StagingStartupFailure;
use std::time::SystemTime;

const SWITCH: &str = "CHRONO_STAGING_RUNTIME_STARTUP_FAILURE";

pub(super) fn from_lookup(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    now: SystemTime,
) -> Result<Option<StagingStartupFailure>, String> {
    let Some(value) = lookup(SWITCH)? else {
        return Ok(None);
    };
    let environment = lookup("CHRONO_ENVIRONMENT")?;
    if environment.as_deref() != Some("staging") {
        return Ok(None);
    }
    StagingStartupFailure::from_setting(environment.as_deref(), &value, now)
        .map(Some)
        .ok_or_else(|| {
            format!("{SWITCH} requires an exact Sandbox ID and a Unix expiry within 45 minutes")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    const FIXTURE: &str = "sbx_00000000000000000000000000000001";

    fn fixture(
        environment: Option<&str>,
        setting: Option<&str>,
    ) -> Result<Option<StagingStartupFailure>, String> {
        from_lookup(
            &mut |key| {
                let value = match key {
                    SWITCH => setting,
                    "CHRONO_ENVIRONMENT" => environment,
                    _ => panic!("unexpected key {key}"),
                };
                Ok(value.map(str::to_owned))
            },
            UNIX_EPOCH + Duration::from_secs(100),
        )
    }

    #[test]
    fn startup_failure_fixture_defaults_off_and_requires_exact_staging() {
        let valid = format!("{FIXTURE}:200");
        assert!(fixture(Some("staging"), None).unwrap().is_none());
        for environment in [
            None,
            Some("production"),
            Some("development"),
            Some("Staging"),
        ] {
            assert!(fixture(environment, Some(&valid)).unwrap().is_none());
        }
        let fault = fixture(Some("staging"), Some(&valid)).unwrap().unwrap();
        assert!(fault.valid_for(FIXTURE));
        assert!(fault.armed_at(UNIX_EPOCH + Duration::from_secs(199)));
        assert!(!fault.armed_at(UNIX_EPOCH + Duration::from_secs(200)));
        assert!(!fixture(Some("staging"), Some(&format!("{FIXTURE}:90")))
            .unwrap()
            .unwrap()
            .armed_at(UNIX_EPOCH + Duration::from_secs(100)));
    }

    #[test]
    fn startup_failure_fixture_rejects_nonexact_and_unbounded_settings() {
        for invalid in ["sbx_*:200", "staging:true", &format!("{FIXTURE}:2801")] {
            assert!(fixture(Some("staging"), Some(invalid)).is_err());
        }
    }
}
