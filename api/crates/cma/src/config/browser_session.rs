//! Independent opt-in for the indexed browser/subject join.
const KEY: &str = "CHRONO_AUTH_BROWSER_SESSION_JOIN_ENABLED";

pub(super) fn from_lookup(
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
    fn joined_browser_reads_require_an_explicit_strict_boolean() {
        for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
            assert_eq!(
                from_lookup(&mut |key| {
                    assert_eq!(key, KEY);
                    Ok(value.map(str::to_owned))
                }),
                Ok(expected)
            );
        }
        for value in ["", "TRUE", "1", "false ", " true"] {
            assert!(from_lookup(&mut |_| Ok(Some(value.into()))).is_err());
        }
        assert_eq!(
            from_lookup(&mut |_| Err("invalid encoding".into())),
            Err("invalid encoding".into())
        );
    }
}
