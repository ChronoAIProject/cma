//! Default-off normal-path identity reuse has an absolute, bounded lifetime.
const KEY: &str = "CHRONO_AUTH_VERIFIED_REUSE_SECONDS";

pub(super) fn from_lookup(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<u64, String> {
    match lookup(KEY)?.as_deref() {
        None | Some("0") => Ok(0),
        Some(value @ ("1" | "2" | "3" | "4" | "5")) => Ok(value.parse().expect("one digit")),
        _ => Err(format!("{KEY} must be an integer from 0 to 5")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reuse_is_off_by_default_and_rejects_ambiguous_or_unbounded_configuration() {
        assert_eq!(from_lookup(&mut |_| Ok(None)), Ok(0));
        for value in 0..=5 {
            assert_eq!(from_lookup(&mut |_| Ok(Some(value.to_string()))), Ok(value));
        }
        for value in ["", "6", "60", "-1", "1.5", "01", "true", " 1", "1 "] {
            assert!(from_lookup(&mut |_| Ok(Some(value.into()))).is_err());
        }
        assert_eq!(
            from_lookup(&mut |_| Err("encoding".into())),
            Err("encoding".into())
        );
    }
}
