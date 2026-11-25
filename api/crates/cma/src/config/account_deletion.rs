//! Close deletion admission until the compatible access reader floor is deployed.

pub(super) fn enabled(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<bool, String> {
    match lookup("CHRONO_ACCOUNT_DELETION_ENABLED")?.as_deref() {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        _ => Err("CHRONO_ACCOUNT_DELETION_ENABLED must be true or false".into()),
    }
}

pub(super) fn retain_legal_consents(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<bool, String> {
    const KEY: &str = "CHRONO_ERASURE_RETAIN_LEGAL_CONSENTS";
    match lookup(KEY)?.as_deref() {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        _ => Err(format!("{KEY} must be true or false")),
    }
}

pub(super) fn transaction_drain(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<u64, String> {
    let invalid = || {
        "CHRONO_ACCOUNT_ERASURE_TRANSACTION_DRAIN_SECONDS must be an integer in 120..86400"
            .to_owned()
    };
    match lookup("CHRONO_ACCOUNT_ERASURE_TRANSACTION_DRAIN_SECONDS")? {
        None => Ok(120),
        Some(raw) => {
            let value = raw.parse::<u64>().map_err(|_| invalid())?;
            if !(120..=86400).contains(&value) || value.to_string() != raw {
                return Err(invalid());
            }
            Ok(value)
        }
    }
}
