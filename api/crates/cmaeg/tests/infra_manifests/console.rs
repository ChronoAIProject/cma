//! The console's memory rule: with the console enabled, its in-memory state
//! must fit in an eighth of the cmaeg container's memory limit.

use cmaeg::console::summary::SUMMARY_UNIT_BYTES;

use super::{configmap_value, container_memory, with_memory, CONFIGMAP, DEPLOYMENT};

/// The memory each bounded console structure may hold, as
/// `(key, bytes per unit, default)`. The activity budget is already in bytes.
/// A session holds two tokens, its metadata and the owner switcher's
/// organization list; an index head a record id, an owner and a version; a
/// channel id at most 512 B plus its key. The summary cache is weighed in
/// the gateway's own 2 KiB units, a summary taking one per 2 KiB it holds,
/// so its term is exact however large a channel's routing policy is.
const CONSOLE_TERMS: [(&str, u64, u64); 5] = [
    ("CMAEG_CONSOLE_ACTIVITY_MAX_BYTES", 1, 33_554_432),
    ("CMAEG_CONSOLE_INDEX_MAX_RECORDS", 300, 100_000),
    ("CMAEG_CONSOLE_CHANNEL_IDS_MAX", 600, 20_000),
    ("CMAEG_CONSOLE_SUMMARY_CACHE_MAX", SUMMARY_UNIT_BYTES, 2_000),
    ("CMAEG_CONSOLE_SESSIONS_MAX", 4 * 1024, 1_000),
];

/// With the console enabled, its in-memory state must fit in an eighth of the
/// cmaeg container's memory limit:
/// `ACTIVITY_MAX_BYTES + INDEX_MAX_RECORDS × 300 B + CHANNEL_IDS_MAX × 600 B
/// + SUMMARY_CACHE_MAX × 2 KiB + SESSIONS_MAX × 4 KiB`.
fn console_memory_rule(configmap: &str, deployment: &str) -> Result<(), String> {
    if configmap_value(configmap, "CMAEG_CONSOLE_ENABLED") != Some("true") {
        return Ok(());
    }
    let limit = container_memory(deployment, "cmaeg", "limits")?;
    let mut budget = 0u64;
    for (key, unit, default) in CONSOLE_TERMS {
        let count = match configmap_value(configmap, key) {
            None => default,
            Some(value) => value
                .parse::<u64>()
                .map_err(|_| format!("{key} {value:?} is not an unsigned decimal"))?,
        };
        budget = budget.saturating_add(count.saturating_mul(unit));
    }
    if budget > limit / 8 {
        let keys = CONSOLE_TERMS.map(|(key, _, _)| key).join(", ");
        return Err(format!(
            "CMAEG_CONSOLE_ENABLED=true requires the console memory of {keys} ({budget} bytes) to fit in an eighth of the cmaeg memory limit ({limit} bytes)"
        ));
    }
    Ok(())
}

fn console_on() -> String {
    let on = CONFIGMAP.replace(
        "CMAEG_CONSOLE_ENABLED: \"false\"",
        "CMAEG_CONSOLE_ENABLED: \"true\"",
    );
    assert_ne!(on, CONFIGMAP, "the shipped console flag line changed shape");
    on
}

#[test]
fn console_enabled_requires_its_memory_within_an_eighth_of_the_limit() {
    assert_eq!(
        configmap_value(CONFIGMAP, "CMAEG_CONSOLE_ENABLED"),
        Some("false")
    );
    let on = console_on();
    assert_eq!(console_memory_rule(&on, DEPLOYMENT), Ok(()));
    let oversized = on.replace(
        "CMAEG_CONSOLE_SESSIONS_MAX: \"1000\"",
        "CMAEG_CONSOLE_SESSIONS_MAX: \"100000\"",
    );
    assert_ne!(oversized, on, "the shipped session cap line changed shape");
    let error = console_memory_rule(&oversized, DEPLOYMENT).unwrap_err();
    assert!(error.contains("CMAEG_CONSOLE_SESSIONS_MAX"), "{error}");
    // A larger container makes room for the same cap, and the flag off
    // accepts any cap.
    let large = with_memory(DEPLOYMENT, "4Gi", "4Gi");
    assert_eq!(console_memory_rule(&oversized, &large), Ok(()));
    let off = oversized.replace(
        "CMAEG_CONSOLE_ENABLED: \"true\"",
        "CMAEG_CONSOLE_ENABLED: \"false\"",
    );
    assert_eq!(console_memory_rule(&off, DEPLOYMENT), Ok(()));
}

#[test]
fn the_shipped_memory_keys_fit_at_1gi_and_1536mi() {
    let on = console_on();
    for (key, _, default) in CONSOLE_TERMS {
        assert_eq!(
            configmap_value(&on, key),
            Some(default.to_string().as_str()),
            "{key} ships its documented default"
        );
    }
    assert_eq!(console_memory_rule(&on, DEPLOYMENT), Ok(()));
    let media_sized = with_memory(DEPLOYMENT, "1536Mi", "1536Mi");
    assert_eq!(console_memory_rule(&on, &media_sized), Ok(()));
}

#[test]
fn an_oversized_activity_budget_is_rejected_naming_the_keys() {
    let oversized = console_on().replace(
        "CMAEG_CONSOLE_ACTIVITY_MAX_BYTES: \"33554432\"",
        "CMAEG_CONSOLE_ACTIVITY_MAX_BYTES: \"268435456\"",
    );
    let error = console_memory_rule(&oversized, DEPLOYMENT).unwrap_err();
    for (key, _, _) in CONSOLE_TERMS {
        assert!(error.contains(key), "{error}");
    }
    let unreadable = console_on().replace(
        "CMAEG_CONSOLE_INDEX_MAX_RECORDS: \"100000\"",
        "CMAEG_CONSOLE_INDEX_MAX_RECORDS: \"lots\"",
    );
    let error = console_memory_rule(&unreadable, DEPLOYMENT).unwrap_err();
    assert!(error.contains("CMAEG_CONSOLE_INDEX_MAX_RECORDS"), "{error}");
}
