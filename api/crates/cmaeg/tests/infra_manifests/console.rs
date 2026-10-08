//! The console's memory rule: with the console enabled, its in-memory state
//! must fit in an eighth of the cmaeg container's memory limit.

use super::{configmap_value, container_memory, with_memory, CONFIGMAP, DEPLOYMENT};

/// What one console session may hold: two tokens, its metadata and the
/// owner switcher's organization list.
const CONSOLE_SESSION_BYTES: u64 = 4 * 1024;
const CONSOLE_SESSIONS_DEFAULT: u64 = 1_000;

/// With the console enabled, its in-memory state must fit in an eighth of the
/// cmaeg container's memory limit. Sessions are its only state so far; the
/// console memory structures add their own terms to the same sum.
fn console_memory_rule(configmap: &str, deployment: &str) -> Result<(), String> {
    if configmap_value(configmap, "CMAEG_CONSOLE_ENABLED") != Some("true") {
        return Ok(());
    }
    let limit = container_memory(deployment, "cmaeg", "limits")?;
    let sessions = match configmap_value(configmap, "CMAEG_CONSOLE_SESSIONS_MAX") {
        None => CONSOLE_SESSIONS_DEFAULT,
        Some(value) => value.parse::<u64>().map_err(|_| {
            format!("CMAEG_CONSOLE_SESSIONS_MAX {value:?} is not an unsigned decimal")
        })?,
    };
    let budget = sessions.saturating_mul(CONSOLE_SESSION_BYTES);
    if budget > limit / 8 {
        return Err(format!(
            "CMAEG_CONSOLE_ENABLED=true requires CMAEG_CONSOLE_SESSIONS_MAX × 4 KiB ({budget} bytes) to fit in an eighth of the cmaeg memory limit ({limit} bytes)"
        ));
    }
    Ok(())
}

#[test]
fn console_enabled_requires_its_memory_within_an_eighth_of_the_limit() {
    assert_eq!(
        configmap_value(CONFIGMAP, "CMAEG_CONSOLE_ENABLED"),
        Some("false")
    );
    let on = CONFIGMAP.replace(
        "CMAEG_CONSOLE_ENABLED: \"false\"",
        "CMAEG_CONSOLE_ENABLED: \"true\"",
    );
    assert_ne!(on, CONFIGMAP, "the shipped console flag line changed shape");
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
