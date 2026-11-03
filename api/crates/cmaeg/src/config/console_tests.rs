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
    assert!(config.nyxid_public_url.is_none());
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
fn oauth_origin_is_optional_and_rejects_non_public_or_credentialed_urls() {
    let config =
        config_with(&[(CMAEG_CONSOLE_NYXID_PUBLIC_URL, "https://API.NYXID.AI:443")]).unwrap();
    assert_eq!(
        config.nyxid_public_url.as_deref(),
        Some("https://api.nyxid.ai")
    );
    for value in [
        "",
        "http://api.nyxid.ai",
        "http://nyxid.chrono-bot.svc",
        "https://api.nyxid.ai/",
        "https://api.nyxid.ai/oauth",
        "https://user:secret@api.nyxid.ai",
        "https://api.nyxid.ai?scope=openid",
        "https://api.nyxid.ai#fragment",
    ] {
        let message = detail(config_with(&[(CMAEG_CONSOLE_NYXID_PUBLIC_URL, value)]).unwrap_err());
        assert!(message.contains(CMAEG_CONSOLE_NYXID_PUBLIC_URL));
        assert!(!message.contains("user:secret"));
    }
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

#[test]
fn memory_keys_take_their_documented_defaults() {
    let memory = config_with(&[]).unwrap().memory;
    assert_eq!(memory, ConsoleMemoryConfig::default());
    assert_eq!(memory.index_max_records, 100_000);
    assert_eq!(memory.index_resync, Duration::from_millis(600_000));
    assert_eq!(memory.summary_cache_max, 2_000);
    assert_eq!(memory.channel_ids_max, 20_000);
    assert_eq!(memory.activity_per_channel, 100);
    assert_eq!(memory.activity_max_bytes, 33_554_432);
    assert_eq!(memory.producers_per_channel, 32);
}

#[test]
fn memory_bounds_are_inclusive_and_out_of_range_values_name_their_key() {
    let low = config_with(&[
        (CMAEG_CONSOLE_INDEX_MAX_RECORDS, "1000"),
        (CMAEG_CONSOLE_INDEX_RESYNC_MS, "60000"),
        (CMAEG_CONSOLE_SUMMARY_CACHE_MAX, "100"),
        (CMAEG_CONSOLE_CHANNEL_IDS_MAX, "100"),
        (CMAEG_CONSOLE_ACTIVITY_PER_CHANNEL, "10"),
        (CMAEG_CONSOLE_ACTIVITY_MAX_BYTES, "1048576"),
        (CMAEG_CONSOLE_PRODUCERS_PER_CHANNEL, "1"),
    ])
    .unwrap()
    .memory;
    assert_eq!(low.index_max_records, 1_000);
    assert_eq!(low.index_resync, Duration::from_secs(60));
    assert_eq!(low.summary_cache_max, 100);
    assert_eq!(low.channel_ids_max, 100);
    assert_eq!(low.activity_per_channel, 10);
    assert_eq!(low.activity_max_bytes, 1_048_576);
    assert_eq!(low.producers_per_channel, 1);
    let high = config_with(&[
        (CMAEG_CONSOLE_INDEX_MAX_RECORDS, "1000000"),
        (CMAEG_CONSOLE_INDEX_RESYNC_MS, "86400000"),
        (CMAEG_CONSOLE_SUMMARY_CACHE_MAX, "100000"),
        (CMAEG_CONSOLE_CHANNEL_IDS_MAX, "1000000"),
        (CMAEG_CONSOLE_ACTIVITY_PER_CHANNEL, "1000"),
        (CMAEG_CONSOLE_ACTIVITY_MAX_BYTES, "268435456"),
        (CMAEG_CONSOLE_PRODUCERS_PER_CHANNEL, "256"),
    ])
    .unwrap()
    .memory;
    assert_eq!(high.index_max_records, 1_000_000);
    assert_eq!(high.activity_max_bytes, 268_435_456);
    assert_eq!(high.producers_per_channel, 256);
    for (key, value, range) in [
        (
            CMAEG_CONSOLE_INDEX_MAX_RECORDS,
            "999",
            "from 1000 to 1000000",
        ),
        (
            CMAEG_CONSOLE_INDEX_RESYNC_MS,
            "86400001",
            "from 60000 to 86400000",
        ),
        (CMAEG_CONSOLE_SUMMARY_CACHE_MAX, "99", "from 100 to 100000"),
        (
            CMAEG_CONSOLE_CHANNEL_IDS_MAX,
            "1000001",
            "from 100 to 1000000",
        ),
        (CMAEG_CONSOLE_ACTIVITY_PER_CHANNEL, "9", "from 10 to 1000"),
        (
            CMAEG_CONSOLE_ACTIVITY_MAX_BYTES,
            "268435457",
            "from 1048576 to 268435456",
        ),
        (CMAEG_CONSOLE_PRODUCERS_PER_CHANNEL, "0", "from 1 to 256"),
        (CMAEG_CONSOLE_PRODUCERS_PER_CHANNEL, "many", "from 1 to 256"),
    ] {
        assert_eq!(
            detail(config_with(&[(key, value)]).unwrap_err()),
            format!("{key} must be an unsigned decimal {range}"),
            "{key}={value}"
        );
    }
}
