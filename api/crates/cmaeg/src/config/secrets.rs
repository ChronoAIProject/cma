use super::ConfigError;
use std::{env, fs};

/// Accept Kubernetes Secret environment injection, retaining optional file
/// configuration for existing operators. Refuse ambiguous sources at startup.
pub(crate) fn read(name: &'static str, file_name: &'static str) -> Result<String, ConfigError> {
    let direct = env::var_os(name);
    let file = env::var_os(file_name);
    let content = match (direct, file) {
        (Some(_), Some(_)) => {
            return Err(ConfigError::InvalidManagement {
                detail: format!("set only one of {name} and {file_name}"),
            });
        }
        (Some(value), None) => value
            .into_string()
            .map_err(|_| ConfigError::InvalidManagement {
                detail: format!("{name} must contain UTF-8 text"),
            })?,
        (None, Some(path)) => {
            fs::read_to_string(path).map_err(|_| ConfigError::InvalidManagement {
                detail: format!("cannot read {file_name}"),
            })?
        }
        (None, None) => {
            return Err(ConfigError::InvalidManagement {
                detail: format!("{name} or {file_name} must be set"),
            });
        }
    };
    let value = content.trim().to_owned();
    if value.is_empty() || value.lines().count() != 1 {
        return Err(ConfigError::InvalidManagement {
            detail: format!("{name} must contain one non-empty line"),
        });
    }
    Ok(value)
}
