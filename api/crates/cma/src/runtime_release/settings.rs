use std::path::PathBuf;

use base64::{engine::general_purpose::STANDARD, Engine};

#[derive(Clone, Default)]
pub(crate) enum Settings {
    #[default]
    Disabled,
    Managed {
        trusted_public_key: [u8; 32],
        grant_path: PathBuf,
        challenge_path: PathBuf,
        binary_image: String,
    },
}

impl Settings {
    pub(crate) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Self, String> {
        let mode = lookup("CHRONO_RUNTIME_RELEASE_MODE")?.unwrap_or_else(|| "disabled".into());
        if mode == "disabled" {
            return Ok(Self::Disabled);
        }
        if mode != "managed" {
            return Err("CHRONO_RUNTIME_RELEASE_MODE must be disabled or managed".into());
        }
        let mut required = |name: &str| {
            lookup(name)?
                .filter(|value| !value.is_empty())
                .ok_or_else(|| format!("{name} is required for managed runtime release authority"))
        };
        let key = required("CHRONO_RUNTIME_RELEASE_PUBLIC_KEY")?;
        let trusted_public_key: [u8; 32] = STANDARD
            .decode(key)
            .map_err(|_| "runtime release public key must be base64 Ed25519")?
            .try_into()
            .map_err(|_| "runtime release public key must be 32 bytes")?;
        let grant_path = path(required("CHRONO_RUNTIME_RELEASE_GRANT_PATH")?)?;
        let challenge_path = path(required("CHRONO_RUNTIME_RELEASE_CHALLENGE_PATH")?)?;
        let binary_image = required("CHRONO_RUNTIME_RELEASE_BINARY_IMAGE")?;
        if grant_path == challenge_path || !super::bootstrap::image(&binary_image) {
            return Err("runtime release paths and binary identity are invalid".into());
        }
        Ok(Self::Managed {
            trusted_public_key,
            grant_path,
            challenge_path,
            binary_image,
        })
    }
}

fn path(value: String) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    if !path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return Err("runtime release receipt path must be absolute without traversal".into());
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_disabled_and_unknown_mode_refuses() {
        assert!(matches!(
            Settings::from_lookup(&mut |_| Ok(None)).unwrap(),
            Settings::Disabled
        ));
        assert!(Settings::from_lookup(&mut |name| Ok(
            (name == "CHRONO_RUNTIME_RELEASE_MODE").then(|| "true".into())
        ))
        .is_err());
    }

    #[test]
    fn managed_requires_explicit_trust_and_private_receipt_paths() {
        assert!(Settings::from_lookup(&mut |name| Ok(
            (name == "CHRONO_RUNTIME_RELEASE_MODE").then(|| "managed".into())
        ))
        .is_err());
        assert!(path("relative/receipt".into()).is_err());
        assert!(path("/tmp/../receipt".into()).is_err());
        assert!(path("/tmp/owned/receipt".into()).is_ok());
    }
}
