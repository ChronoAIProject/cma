//! Request telemetry rollout is independent of durable audit recording.
const KEY: &str = "CHRONO_ACTIVITY_ASYNC_ENABLED";

pub(super) fn enabled(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<bool, String> {
    boolean(KEY, lookup)
}

pub(super) fn sampling_enabled(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<bool, String> {
    boolean("CHRONO_ACTIVITY_OPERATIONAL_READ_SAMPLING_ENABLED", lookup)
}

pub(super) fn ingress_audit_enabled(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<bool, String> {
    boolean("CHRONO_ACTIVITY_INGRESS_AUDIT_ENABLED", lookup)
}

pub(super) fn gateway_audit_subject(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<Option<layer2_core::SubjectId>, String> {
    let Some(subject) = lookup("CHRONO_ACTIVITY_GATEWAY_AUDIT_SUBJECT")? else {
        return Ok(None);
    };
    if subject.is_empty()
        || subject.len() > 256
        || subject.trim() != subject
        || subject.chars().any(char::is_control)
    {
        return Err(
            "CHRONO_ACTIVITY_GATEWAY_AUDIT_SUBJECT requires one exact service subject".into(),
        );
    }
    Ok(Some(layer2_core::SubjectId::new(subject)))
}

fn boolean(
    key: &str,
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<bool, String> {
    match lookup(key)?.as_deref() {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        _ => Err(format!("{key} must be true or false")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_async_defaults_off_and_accepts_only_exact_booleans() {
        for (raw, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
            assert_eq!(enabled(&mut |_| Ok(raw.map(str::to_owned))), Ok(expected));
            assert_eq!(
                sampling_enabled(&mut |_| Ok(raw.map(str::to_owned))),
                Ok(expected)
            );
        }
        for raw in ["", "1", "TRUE", "False", " true", "false "] {
            assert!(enabled(&mut |_| Ok(Some(raw.to_owned()))).is_err());
            assert!(sampling_enabled(&mut |_| Ok(Some(raw.to_owned()))).is_err());
        }
        assert_eq!(
            enabled(&mut |_| Err("lookup failed".into())),
            Err("lookup failed".into())
        );
    }
}
