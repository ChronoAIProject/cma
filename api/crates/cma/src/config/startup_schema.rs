//! Startup enforcement never authorizes schema mutation or retention changes.

pub(crate) const KEY: &str = "CHRONO_AM_STARTUP_SCHEMA_ENFORCEMENT_ENABLED";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum StartupSchemaMode {
    #[default]
    Observe,
    Enforce,
}

impl StartupSchemaMode {
    pub(crate) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Self, String> {
        match lookup(KEY)?.as_deref() {
            None | Some("false") => Ok(Self::Observe),
            Some("true") => Ok(Self::Enforce),
            _ => Err(format!("{KEY} must be true or false")),
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::Enforce => "enforce",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_configuration_preserves_the_parsed_startup_mode() {
        let key_path =
            std::env::temp_dir().join(format!("cma-schema-config-{}", uuid::Uuid::new_v4()));
        std::fs::write(&key_path, "fixture-only").unwrap();
        for value in [None, Some("false"), Some("true"), Some("untrusted-input")] {
            let result = crate::config::Config::from_lookup(|key| {
                let value = match key {
                    KEY => value.map(str::to_owned),
                    "CHRONO_AUTH_MODE" => Some("trusted".into()),
                    "CHRONO_MONGODB_URI" => Some("mongodb://localhost/fixture".into()),
                    "CHRONO_AUTH_ENCRYPTION_KEY" => {
                        Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".into())
                    }
                    "CHRONO_SANDBOX_BASE_URL" => Some("http://sandbox.example".into()),
                    "CHRONO_SANDBOX_API_KEY_FILE" => Some(key_path.to_str().unwrap().into()),
                    "CHRONO_SANDBOX_IMAGE" => Some(format!(
                        "registry.example/cma-runtime:v1@sha256:{}",
                        "a".repeat(64)
                    )),
                    "CHRONO_SANDBOX_NAMESPACE" => Some("fixture".into()),
                    "CHRONO_UPSTREAM_BASE_URL" => {
                        Some("https://models.example/api/v1/proxy/s/model".into())
                    }
                    "CHRONO_UPSTREAM_WIRE_API" => Some("responses".into()),
                    "CHRONO_PLATFORM_BASE_URL" => Some("https://platform.example".into()),
                    "CHRONO_DEPLOYMENT_ID" => Some("fixture".into()),
                    _ => None,
                };
                Ok(value)
            });
            if value == Some("untrusted-input") {
                assert_eq!(result.unwrap_err(), format!("{KEY} must be true or false"));
            } else {
                let expected = if value == Some("true") {
                    StartupSchemaMode::Enforce
                } else {
                    StartupSchemaMode::Observe
                };
                assert_eq!(result.unwrap().startup_schema_mode, expected);
            }
        }
        std::fs::remove_file(key_path).unwrap();
    }

    #[test]
    fn enforcement_is_default_off_and_only_accepts_exact_booleans() {
        for (value, expected) in [
            (None, StartupSchemaMode::Observe),
            (Some("false"), StartupSchemaMode::Observe),
            (Some("true"), StartupSchemaMode::Enforce),
        ] {
            let mut calls = 0;
            let actual = StartupSchemaMode::from_lookup(&mut |key| {
                assert_eq!(key, KEY);
                calls += 1;
                Ok(value.map(str::to_owned))
            });
            assert_eq!(actual, Ok(expected));
            assert_eq!(calls, 1);
        }
        for value in [
            "",
            "0",
            "1",
            "TRUE",
            "False",
            " true",
            "false ",
            "untrusted-input",
        ] {
            assert_eq!(
                StartupSchemaMode::from_lookup(&mut |_| Ok(Some(value.into()))),
                Err(format!("{KEY} must be true or false")),
            );
        }
        assert_eq!(
            StartupSchemaMode::from_lookup(&mut |_| Err("lookup failed".into())),
            Err("lookup failed".into()),
        );
    }
}
