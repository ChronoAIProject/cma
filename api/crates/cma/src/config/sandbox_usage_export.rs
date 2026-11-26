//! Capture is independent of both legacy funding and external delivery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UsageCaptureConfig {
    pub namespace: String,
    pub owner_issuer: String,
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
        Ok(Some(Self {
            namespace: value(lookup, "CHRONO_SANDBOX_USAGE_PRODUCER_NAMESPACE")?,
            owner_issuer: value(lookup, "CHRONO_SANDBOX_USAGE_OWNER_ISSUER")?,
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
