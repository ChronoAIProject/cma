//! A deployment ceiling is opt-in; no guessed cluster capacity is enabled.

pub(super) fn from_lookup(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<u16, String> {
    const KEY: &str = "CHRONO_SANDBOX_GLOBAL_LIMIT";
    let value = lookup(KEY)?.unwrap_or_else(|| "0".into());
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("{KEY} must be an integer from 0 to 4096"));
    }
    value
        .parse::<u16>()
        .ok()
        .filter(|limit| *limit <= 4096)
        .ok_or_else(|| format!("{KEY} must be an integer from 0 to 4096"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_admission_is_off_until_an_explicit_bounded_limit_is_set() {
        assert_eq!(from_lookup(&mut |_| Ok(None)).unwrap(), 0);
        for (value, expected) in [("0", 0), ("1", 1), ("30", 30), ("4096", 4096)] {
            assert_eq!(
                from_lookup(&mut |_| Ok(Some(value.into()))).unwrap(),
                expected
            );
        }
        for value in ["", "-1", "+1", "1.0", "true", " 30", "4097", "65536"] {
            assert!(from_lookup(&mut |_| Ok(Some(value.into()))).is_err());
        }
        assert!(from_lookup(&mut |_| Err("lookup failed".into())).is_err());
    }
}
