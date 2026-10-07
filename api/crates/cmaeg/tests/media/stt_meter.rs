//! Speech-to-text metering, the three spend limits, usage reconciliation,
//! the channels-at-budget gauge and the spend configuration.
use super::budget::channel;
use cmaeg::{
    media::transcribe::meter::{
        direct_cap, direct_seconds, SpendRefusal, SttSpend, METERING_FLOOR_BYTES_PER_SECOND,
    },
    ConfigError, SttConfig,
};
use std::time::Duration;

/// One call may cost 600 s, a channel 600 s per hour and the pod `global`.
pub(super) fn tight(global: u32) -> SttConfig {
    SttConfig {
        channel_seconds_per_hour: 600,
        seconds_per_hour: global,
        ..SttConfig::default()
    }
}

/// The value of an unlabelled metric in Prometheus text.
pub(super) fn sample(text: &str, name: &str) -> Option<f64> {
    text.lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(' ')?.parse().ok())
}

#[test]
fn direct_uploads_meter_one_second_per_4000_bytes_rounded_up() {
    assert_eq!(METERING_FLOOR_BYTES_PER_SECOND, 4000);
    assert_eq!(direct_seconds(0), 1);
    assert_eq!(direct_seconds(1), 1);
    assert_eq!(direct_seconds(4000), 1);
    assert_eq!(direct_seconds(4001), 2);
    assert_eq!(direct_seconds(2_400_000), 600);
    assert_eq!(direct_seconds(u64::MAX), u32::MAX);
}

#[test]
fn the_direct_cap_is_the_smaller_of_the_byte_and_second_caps() {
    let defaults = SttConfig::default();
    assert_eq!(
        direct_cap(defaults.max_bytes, defaults.max_seconds),
        2_400_000
    );
    assert_eq!(direct_cap(1_048_576, defaults.max_seconds), 1_048_576);
    assert_eq!(direct_cap(24_000_000, 1_500), 6_000_000);
    assert_eq!(direct_cap(u64::MAX, u32::MAX), 4000 * u64::from(u32::MAX));
}

#[test]
fn every_refusal_has_a_stable_outcome() {
    assert_eq!(SpendRefusal::CallRate.outcome(), "stt_rate_limited");
    assert_eq!(
        SpendRefusal::ChannelBudget.outcome(),
        "stt_channel_budget_exhausted"
    );
    assert_eq!(SpendRefusal::GlobalBudget.outcome(), "stt_budget_exhausted");
}

#[test]
fn a_channel_budget_refusal_leaves_the_global_budget_untouched() {
    let spend = SttSpend::new(&tight(1_200));
    let (a, b) = (channel("stt-a"), channel("stt-b"));
    assert_eq!(spend.charge(&a, 600), Ok(()));
    assert_eq!(spend.charge(&a, 600), Err(SpendRefusal::ChannelBudget));
    assert_eq!(spend.charge(&a, 1), Err(SpendRefusal::ChannelBudget));
    // The global budget still holds the 600 s the refused charges did not take.
    assert_eq!(spend.charge(&b, 600), Ok(()));
    assert_eq!(
        spend.charge(&channel("stt-c"), 100),
        Err(SpendRefusal::GlobalBudget)
    );
}

#[test]
fn an_exhausted_global_budget_refuses_a_channel_within_its_own() {
    let spend = SttSpend::new(&tight(600));
    assert_eq!(spend.charge(&channel("stt-a"), 600), Ok(()));
    assert_eq!(
        spend.charge(&channel("stt-b"), 100),
        Err(SpendRefusal::GlobalBudget)
    );
    // A global refusal does not mark the channel as at its own budget.
    assert_eq!(spend.channels_at_budget(), 0);
}

#[test]
fn one_channel_at_its_budget_does_not_affect_another() {
    let spend = SttSpend::new(&tight(36_000));
    let (a, b) = (channel("stt-a"), channel("stt-b"));
    assert_eq!(spend.charge(&a, 600), Ok(()));
    assert_eq!(spend.charge(&a, 100), Err(SpendRefusal::ChannelBudget));
    assert_eq!(spend.charge(&b, 600), Ok(()));
    assert_eq!(spend.channels_at_budget(), 1);
}

#[test]
fn calls_past_the_channel_rate_are_refused() {
    let spend = SttSpend::new(&SttConfig {
        rate_per_minute: 2,
        ..SttConfig::default()
    });
    let (a, b) = (channel("stt-a"), channel("stt-b"));
    assert_eq!(spend.admit_call(&a), Ok(()));
    assert_eq!(spend.admit_call(&a), Ok(()));
    assert_eq!(spend.admit_call(&a), Err(SpendRefusal::CallRate));
    assert_eq!(spend.admit_call(&b), Ok(()));
}

#[test]
fn reported_usage_above_the_metered_seconds_is_charged_and_counted() {
    let spend = SttSpend::new(&tight(36_000));
    let a = channel("stt-a");
    assert_eq!(spend.charge(&a, 100), Ok(()));
    assert_eq!(spend.reconcile(&a, 100, Some(300)), 300);
    // 300 s of the channel's 600 s are spent, not 100 s.
    assert_eq!(spend.charge(&a, 301), Err(SpendRefusal::ChannelBudget));
    assert_eq!(spend.charge(&a, 300), Ok(()));
    // The channel is spent: the excess is refused, ignored and still counted.
    assert_eq!(spend.reconcile(&a, 100, Some(250)), 250);
}

#[test]
fn usage_at_or_below_the_metered_seconds_or_absent_counts_the_metered() {
    let spend = SttSpend::new(&tight(36_000));
    let a = channel("stt-a");
    assert_eq!(spend.charge(&a, 600), Ok(()));
    assert_eq!(spend.reconcile(&a, 600, Some(600)), 600);
    assert_eq!(spend.reconcile(&a, 600, Some(20)), 600);
    assert_eq!(spend.reconcile(&a, 600, None), 600);
}

#[tokio::test(start_paused = true)]
async fn the_at_budget_gauge_counts_distinct_channels_for_an_hour() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _recording = metrics::set_default_local_recorder(&recorder);
    let spend = SttSpend::new(&tight(360_000));
    let (a, b) = (channel("stt-a"), channel("stt-b"));
    for refused in [&a, &a, &b] {
        let _ = spend.charge(refused, 601);
    }
    assert_eq!(spend.channels_at_budget(), 2);
    let gauge = "cmaeg_stt_channels_at_budget";
    assert_eq!(sample(&handle.render(), gauge), Some(2.0));

    tokio::time::advance(Duration::from_secs(3_600)).await;
    assert_eq!(spend.channels_at_budget(), 0);
    // The next sweep, within 256 charges, publishes the expiry.
    let spender = channel("stt-d");
    for _ in 0..256 {
        assert_eq!(spend.charge(&spender, 1), Ok(()));
    }
    assert_eq!(sample(&handle.render(), gauge), Some(0.0));
    assert_eq!(spend.channels_at_budget(), 0);
}

fn config_with(pairs: &[(&'static str, &str)]) -> Result<SttConfig, ConfigError> {
    let pairs: Vec<(&'static str, String)> = pairs
        .iter()
        .map(|(key, value)| (*key, (*value).to_owned()))
        .collect();
    SttConfig::from_lookup(|key| {
        Ok(pairs
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value.clone()))
    })
}

fn detail(error: ConfigError) -> String {
    match error {
        ConfigError::InvalidMedia { detail } => detail,
        other => panic!("expected an invalid media configuration, got {other}"),
    }
}

const MAX_SECONDS: &str = "CMAEG_STT_MAX_SECONDS";
const MAX_BYTES: &str = "CMAEG_STT_MAX_BYTES";
const RATE: &str = "CMAEG_STT_RATE_PER_MINUTE";
const CHANNEL: &str = "CMAEG_STT_CHANNEL_SECONDS_PER_HOUR";
const GLOBAL: &str = "CMAEG_STT_SECONDS_PER_HOUR";

#[test]
fn unset_spend_keys_take_their_documented_defaults() {
    assert_eq!(
        config_with(&[]).unwrap(),
        SttConfig {
            max_seconds: 600,
            max_bytes: 20_971_520,
            rate_per_minute: 20,
            channel_seconds_per_hour: 3_600,
            seconds_per_hour: 36_000,
            ..SttConfig::default()
        }
    );
    assert_eq!(config_with(&[]).unwrap(), SttConfig::default());
}

#[test]
fn spend_keys_accept_their_bounds_inclusive() {
    let low = config_with(&[
        (MAX_SECONDS, "1"),
        (MAX_BYTES, "1048576"),
        (RATE, "1"),
        (CHANNEL, "600"),
        (GLOBAL, "600"),
    ])
    .unwrap();
    assert_eq!(
        low,
        SttConfig {
            max_seconds: 1,
            max_bytes: 1_048_576,
            rate_per_minute: 1,
            channel_seconds_per_hour: 600,
            seconds_per_hour: 600,
            ..SttConfig::default()
        }
    );
    let high = config_with(&[
        (MAX_SECONDS, "1500"),
        (MAX_BYTES, "24000000"),
        (RATE, "600"),
        (CHANNEL, "86400"),
        (GLOBAL, "360000"),
    ])
    .unwrap();
    assert_eq!(
        high,
        SttConfig {
            max_seconds: 1_500,
            max_bytes: 24_000_000,
            rate_per_minute: 600,
            channel_seconds_per_hour: 86_400,
            seconds_per_hour: 360_000,
            ..SttConfig::default()
        }
    );
}

#[test]
fn out_of_bounds_spend_keys_name_their_key_and_range() {
    for (key, value, range, extra) in [
        (MAX_SECONDS, "0", "from 1 to 1500", None),
        (MAX_SECONDS, "1501", "from 1 to 1500", None),
        (MAX_BYTES, "1048575", "from 1048576 to 24000000", None),
        (MAX_BYTES, "24000001", "from 1048576 to 24000000", None),
        (RATE, "0", "from 1 to 600", None),
        (RATE, "601", "from 1 to 600", None),
        (CHANNEL, "599", "from 600 to 86400", None),
        (
            CHANNEL,
            "86401",
            "from 600 to 86400",
            Some((GLOBAL, "360000")),
        ),
        (GLOBAL, "599", "from 600 to 360000", None),
        (GLOBAL, "360001", "from 600 to 360000", None),
    ] {
        let mut pairs = vec![(key, value)];
        pairs.extend(extra);
        assert_eq!(
            detail(config_with(&pairs).unwrap_err()),
            format!("{key} must be an unsigned decimal {range}"),
            "{key}={value}"
        );
    }
}

#[test]
fn non_numeric_spend_values_are_refused_by_key() {
    for key in [MAX_SECONDS, MAX_BYTES, RATE, CHANNEL, GLOBAL] {
        for value in [
            "", " 600", "600 ", "+600", "-1", "6e2", "0x258", "1_000", "ten",
        ] {
            let shown = detail(config_with(&[(key, value)]).unwrap_err());
            assert!(
                shown.starts_with(&format!("{key} must be an unsigned decimal from ")),
                "{key}={value:?}: {shown}"
            );
        }
    }
}

#[test]
fn a_call_must_fit_a_channel_hour_and_a_channel_hour_the_global_one() {
    let relation = |max: &str, channel: &str, global: &str| {
        format!(
            "CMAEG_STT_MAX_SECONDS ({max}) <= CMAEG_STT_CHANNEL_SECONDS_PER_HOUR ({channel}) \
             <= CMAEG_STT_SECONDS_PER_HOUR ({global}) must hold"
        )
    };
    assert_eq!(
        detail(config_with(&[(MAX_SECONDS, "601"), (CHANNEL, "600")]).unwrap_err()),
        relation("601", "600", "36000")
    );
    assert_eq!(
        detail(config_with(&[(CHANNEL, "36001")]).unwrap_err()),
        relation("600", "36001", "36000")
    );
    assert_eq!(
        detail(config_with(&[(GLOBAL, "3599")]).unwrap_err()),
        relation("600", "3600", "3599")
    );
    assert_eq!(
        detail(
            config_with(&[(MAX_SECONDS, "1500"), (CHANNEL, "1000"), (GLOBAL, "900")]).unwrap_err()
        ),
        relation("1500", "1000", "900")
    );
    // Equality on both sides holds.
    assert!(config_with(&[(MAX_SECONDS, "600"), (CHANNEL, "600"), (GLOBAL, "600")]).is_ok());
}

#[test]
fn a_lookup_failure_is_reported_unchanged() {
    let error = SttConfig::from_lookup(|key| {
        Err(ConfigError::InvalidMedia {
            detail: format!("{key} must be valid UTF-8"),
        })
    })
    .unwrap_err();
    assert_eq!(detail(error), "CMAEG_STT_MAX_SECONDS must be valid UTF-8");
}
