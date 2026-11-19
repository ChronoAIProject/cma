const KEY: &str = "CHRONO_LIFECYCLE_FAILURE_METADATA_ENABLED";

pub(super) fn enabled(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<bool, String> {
    match lookup(KEY)?.as_deref() {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        _ => Err(format!("{KEY} must be true or false")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_metadata_writer_requires_explicit_activation() {
        for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
            assert_eq!(enabled(&mut |_| Ok(value.map(str::to_owned))), Ok(expected));
        }
        for value in ["", "1", "TRUE", " true", "false "] {
            assert!(enabled(&mut |_| Ok(Some(value.into()))).is_err());
        }
        assert!(enabled(&mut |_| Err("lookup failed".into())).is_err());
    }
}
