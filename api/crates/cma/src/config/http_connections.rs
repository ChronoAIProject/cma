//! Optional bounded age for accepted HTTP connections.

use std::time::Duration;

const KEY: &str = "CHRONO_HTTP_CONNECTION_MAX_AGE_SECONDS";
const MIN_SECONDS: u64 = 30;
const MAX_SECONDS: u64 = 86_400;

/// Unset keeps unbounded connections. A set value retires each connection for
/// new requests after that age so long-lived clients rebalance across replicas.
pub(super) fn max_age(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<Option<Duration>, String> {
    let Some(raw) = lookup(KEY)? else {
        return Ok(None);
    };
    let invalid = || format!("{KEY} must be an integer in {MIN_SECONDS}..={MAX_SECONDS}");
    let seconds = raw.parse::<u64>().map_err(|_| invalid())?;
    if !(MIN_SECONDS..=MAX_SECONDS).contains(&seconds) || seconds.to_string() != raw {
        return Err(invalid());
    }
    Ok(Some(Duration::from_secs(seconds)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(value: Option<&str>) -> Result<Option<Duration>, String> {
        max_age(&mut |key| Ok((key == KEY).then(|| value.map(str::to_owned)).flatten()))
    }

    #[test]
    fn connection_age_is_unbounded_unless_explicitly_configured() {
        assert_eq!(parse(None), Ok(None));
        assert_eq!(parse(Some("30")), Ok(Some(Duration::from_secs(30))));
        assert_eq!(parse(Some("86400")), Ok(Some(Duration::from_secs(86_400))));
        for value in ["", "0", "29", "86401", "300s", " 300", "+300", "0300", "-1"] {
            assert!(parse(Some(value)).is_err(), "{value:?}");
        }
    }
}
