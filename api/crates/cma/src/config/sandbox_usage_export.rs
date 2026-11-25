//! Capture is independent of both legacy funding and external delivery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UsageCaptureConfig {
    pub namespace: String,
    pub owner_issuer: String,
    pub completeness_enabled: bool,
}

impl UsageCaptureConfig {
    pub(super) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Option<Self>, String> {
        match lookup("CHRONO_SANDBOX_USAGE_CAPTURE_ENABLED")?.as_deref() {
            None | Some("false") => return Ok(None),
            Some("true") => {}
            _ => return Err("CHRONO_SANDBOX_USAGE_CAPTURE_ENABLED must be true or false".into()),
        }
        let value = |lookup: &mut _, key| {
            let value = super::required(lookup, key)?;
            if value.len() > 256 || value.trim() != value || value.chars().any(char::is_control) {
                return Err(format!("{key} must be a bounded nonempty identity"));
            }
            Ok(value)
        };
        let completeness_enabled =
            match lookup("CHRONO_SANDBOX_USAGE_COMPLETENESS_ENABLED")?.as_deref() {
                None | Some("false") => false,
                Some("true") => true,
                _ => {
                    return Err(
                        "CHRONO_SANDBOX_USAGE_COMPLETENESS_ENABLED must be true or false".into(),
                    );
                }
            };
        Ok(Some(Self {
            namespace: value(lookup, "CHRONO_SANDBOX_USAGE_PRODUCER_NAMESPACE")?,
            owner_issuer: value(lookup, "CHRONO_SANDBOX_USAGE_OWNER_ISSUER")?,
            completeness_enabled,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_capture_does_not_read_identity_or_delivery_configuration() {
        for flag in [None, Some("false")] {
            let value = UsageCaptureConfig::from_lookup(&mut |key| {
                assert_eq!(key, "CHRONO_SANDBOX_USAGE_CAPTURE_ENABLED");
                Ok(flag.map(str::to_owned))
            });
            assert_eq!(value.unwrap(), None);
        }
    }

    #[test]
    fn enabled_capture_requires_explicit_stable_identity() {
        for flag in ["", "1", "TRUE", " false"] {
            assert!(UsageCaptureConfig::from_lookup(&mut |_| Ok(Some(flag.into()))).is_err());
        }
        assert!(UsageCaptureConfig::from_lookup(&mut |key| {
            Ok((key == "CHRONO_SANDBOX_USAGE_CAPTURE_ENABLED").then(|| "true".into()))
        })
        .is_err());
        assert!(UsageCaptureConfig::from_lookup(&mut |key| Ok(Some(
            match key {
                "CHRONO_SANDBOX_USAGE_CAPTURE_ENABLED" => "true",
                "CHRONO_SANDBOX_USAGE_COMPLETENESS_ENABLED" => "false",
                "CHRONO_SANDBOX_USAGE_PRODUCER_NAMESPACE" => "fixture",
                "CHRONO_SANDBOX_USAGE_OWNER_ISSUER" => "https://identity.example",
                _ => panic!("unexpected configuration"),
            }
            .into()
        )))
        .unwrap()
        .is_some());
    }
}

pub(super) fn measured_enabled(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<bool, String> {
    match lookup("CHRONO_SANDBOX_USAGE_MEASURED_ENABLED")?.as_deref() {
        None | Some("false") => Ok(false),
        Some("true") => {
            if UsageCaptureConfig::from_lookup(lookup)?.is_none()
                || !super::MetricsConfig::from_lookup(lookup)?.enabled
            {
                return Err(
                    "measured usage requires allocated capture and verified metrics observation"
                        .into(),
                );
            }
            Ok(true)
        }
        _ => Err("CHRONO_SANDBOX_USAGE_MEASURED_ENABLED must be true or false".into()),
    }
}

#[cfg(test)]
mod measured_tests {
    use super::*;
    #[test]
    fn absent_or_false_measured_switch_reads_no_dependency_configuration() {
        for value in [None, Some("false")] {
            assert!(!measured_enabled(&mut |key| {
                assert_eq!(key, "CHRONO_SANDBOX_USAGE_MEASURED_ENABLED");
                Ok(value.map(str::to_owned))
            })
            .unwrap());
        }
    }
    #[test]
    fn measured_refuses_missing_lifecycle_capture_and_noncanonical_switches() {
        assert!(measured_enabled(&mut |key| Ok(
            (key == "CHRONO_SANDBOX_USAGE_MEASURED_ENABLED").then(|| "true".into())
        ))
        .is_err());
        for value in ["TRUE", "1", " true", ""] {
            assert!(measured_enabled(&mut |_| Ok(Some(value.into()))).is_err());
        }
    }
}
