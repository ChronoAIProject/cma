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
