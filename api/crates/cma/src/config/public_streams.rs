//! Independent, default-off public observation admission.

pub(super) fn from_lookup(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<usize, String> {
    const KEY: &str = "CHRONO_PUBLIC_STREAM_LIMIT";
    let Some(value) = lookup(KEY)? else {
        return Ok(0);
    };
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("{KEY} must be an integer from 0 through 4096"));
    }
    value
        .parse::<usize>()
        .ok()
        .filter(|limit| *limit <= 4096)
        .ok_or_else(|| format!("{KEY} must be an integer from 0 through 4096"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_stream_admission_is_off_by_default_and_strictly_bounded() {
        assert_eq!(from_lookup(&mut |_| Ok(None)).unwrap(), 0);
        for value in ["0", "1", "64", "4096"] {
            assert_eq!(
                from_lookup(&mut |_| Ok(Some(value.into()))).unwrap(),
                value.parse::<usize>().unwrap()
            );
        }
        for value in [
            "",
            "-1",
            "+1",
            " 1",
            "1 ",
            "true",
            "1.0",
            "4097",
            "9999999999999999999999",
        ] {
            assert!(from_lookup(&mut |_| Ok(Some(value.into()))).is_err());
        }
        assert!(from_lookup(&mut |_| Err("lookup failed".into())).is_err());
    }
}
