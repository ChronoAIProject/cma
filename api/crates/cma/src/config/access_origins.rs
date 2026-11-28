//! A bounded, explicit allowlist for previously issued runtime tool authority.
pub(crate) fn retained(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<Vec<String>, String> {
    let key = "CHRONO_AGENT_ACCESS_RETAINED_ORIGINS";
    let raw = lookup("CHRONO_AGENT_ACCESS_RETAINED_ORIGINS")?.unwrap_or_default();
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let mut origins = Vec::new();
    for value in raw.split(',') {
        let origin = super::parse_canonical_https_origin(key, value.trim())?;
        if !origins.contains(&origin) {
            origins.push(origin);
        }
    }
    if origins.len() > 4 {
        return Err(format!("{key} supports at most four origins"));
    }
    Ok(origins)
}
