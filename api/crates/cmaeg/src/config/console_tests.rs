use super::*;

fn config_with(pairs: &[(&'static str, &str)]) -> Result<ConsoleConfig, ConfigError> {
    let pairs: Vec<(&'static str, String)> = pairs
        .iter()
        .map(|(key, value)| (*key, (*value).to_owned()))
        .collect();
    ConsoleConfig::from_lookup(|key| {
        Ok(pairs
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value.clone()))
    })
}

fn detail(error: ConfigError) -> String {
    match error {
        ConfigError::InvalidConsole { detail } => detail,
        other => panic!("expected an invalid console configuration, got {other}"),
    }
}

#[test]
fn unset_keys_take_their_documented_defaults_and_leave_the_console_off() {
    let config = config_with(&[]).unwrap();
    assert!(!config.enabled);
    assert_eq!(config.client_id, "");
    assert!(config.client_secret.is_none());
    assert_eq!(config.login_rate_per_minute, 60);
    assert_eq!(config.session_ttl, Duration::from_millis(28_800_000));
    assert_eq!(config.session_idle, Duration::from_millis(1_800_000));
    assert_eq!(config.sessions_max, 1_000);
    assert_eq!(config.sessions_per_subject, 5);
    assert_eq!(config.missing_client_key(), Some(CMAEG_CONSOLE_CLIENT_ID));
    let defaults = ConsoleConfig::default();
    assert_eq!(defaults.session_idle, config.session_idle);
    assert_eq!(defaults.sessions_max, config.sessions_max);
}

#[test]
fn the_switch_is_off_unless_exactly_true() {
    assert!(
        config_with(&[(CMAEG_CONSOLE_ENABLED, "true")])
            .unwrap()
            .enabled
    );
    assert!(
        !config_with(&[(CMAEG_CONSOLE_ENABLED, "false")])
            .unwrap()
            .enabled
    );
    for value in ["", "True", "1", "yes", " true"] {
        assert_eq!(
            detail(config_with(&[(CMAEG_CONSOLE_ENABLED, value)]).unwrap_err()),
            "CMAEG_CONSOLE_ENABLED must be true or false",
            "{value:?}"
        );
    }
}

#[test]
fn bounds_are_inclusive_and_out_of_range_values_name_their_key() {
    let low = config_with(&[
        (CMAEG_CONSOLE_LOGIN_RATE_PER_MINUTE, "1"),
        (CMAEG_CONSOLE_SESSION_TTL_MS, "600000"),
        (CMAEG_CONSOLE_SESSION_IDLE_MS, "60000"),
        (CMAEG_CONSOLE_SESSIONS_MAX, "10"),
        (CMAEG_CONSOLE_SESSIONS_PER_SUBJECT, "1"),
    ])
    .unwrap();
    assert_eq!(low.login_rate_per_minute, 1);
    assert_eq!(low.session_ttl, Duration::from_secs(600));
    assert_eq!(low.session_idle, Duration::from_secs(60));
    assert_eq!(low.sessions_max, 10);
    assert_eq!(low.sessions_per_subject, 1);
    let high = config_with(&[
        (CMAEG_CONSOLE_LOGIN_RATE_PER_MINUTE, "6000"),
        (CMAEG_CONSOLE_SESSION_TTL_MS, "86400000"),
        (CMAEG_CONSOLE_SESSION_IDLE_MS, "86400000"),
        (CMAEG_CONSOLE_SESSIONS_MAX, "100000"),
        (CMAEG_CONSOLE_SESSIONS_PER_SUBJECT, "50"),
    ])
    .unwrap();
    assert_eq!(high.session_idle, high.session_ttl);
    assert_eq!(high.sessions_max, 100_000);
    for (key, value, range) in [
        (CMAEG_CONSOLE_LOGIN_RATE_PER_MINUTE, "0", "from 1 to 6000"),
        (
            CMAEG_CONSOLE_LOGIN_RATE_PER_MINUTE,
            "6001",
            "from 1 to 6000",
        ),
        (
            CMAEG_CONSOLE_SESSION_TTL_MS,
            "599999",
            "from 600000 to 86400000",
        ),
        (CMAEG_CONSOLE_SESSIONS_MAX, "9", "from 10 to 100000"),
        (CMAEG_CONSOLE_SESSIONS_PER_SUBJECT, "51", "from 1 to 50"),
    ] {
        assert_eq!(
            detail(config_with(&[(key, value)]).unwrap_err()),
            format!("{key} must be an unsigned decimal {range}"),
            "{key}={value}"
        );
    }
}

#[test]
fn the_idle_timeout_never_exceeds_the_absolute_lifetime() {
    let error = config_with(&[
        (CMAEG_CONSOLE_SESSION_TTL_MS, "600000"),
        (CMAEG_CONSOLE_SESSION_IDLE_MS, "600001"),
    ])
    .unwrap_err();
    assert_eq!(
        detail(error),
        "CMAEG_CONSOLE_SESSION_IDLE_MS must be an unsigned decimal from 60000 to 600000"
    );
    // An unset idle timeout follows a shorter lifetime down.
    let short = config_with(&[(CMAEG_CONSOLE_SESSION_TTL_MS, "600000")]).unwrap();
    assert_eq!(short.session_idle, Duration::from_secs(600));
}

#[test]
fn client_settings_fail_soft_when_missing_and_strictly_when_malformed() {
    let id_only = config_with(&[(CMAEG_CONSOLE_CLIENT_ID, "client-1")]).unwrap();
    assert_eq!(
        id_only.missing_client_key(),
        Some(CMAEG_CONSOLE_CLIENT_SECRET)
    );
    let empty_secret = config_with(&[
        (CMAEG_CONSOLE_CLIENT_ID, "client-1"),
        (CMAEG_CONSOLE_CLIENT_SECRET, ""),
    ])
    .unwrap();
    assert!(empty_secret.client_secret.is_none());
    let both = config_with(&[
        (CMAEG_CONSOLE_CLIENT_ID, "client-1"),
        (CMAEG_CONSOLE_CLIENT_SECRET, "fixture-console-secret"),
    ])
    .unwrap();
    assert_eq!(both.missing_client_key(), None);
    assert!(!format!("{both:?}").contains("fixture-console-secret"));
    let spaced = config_with(&[(CMAEG_CONSOLE_CLIENT_SECRET, "fixture console secret")])
        .unwrap_err()
        .to_string();
    assert!(spaced.contains(CMAEG_CONSOLE_CLIENT_SECRET));
    assert!(!spaced.contains("fixture console secret"));
    assert!(config_with(&[(CMAEG_CONSOLE_CLIENT_ID, &"c".repeat(257))]).is_err());
    assert!(config_with(&[(CMAEG_CONSOLE_CLIENT_ID, "client id")]).is_err());
}
