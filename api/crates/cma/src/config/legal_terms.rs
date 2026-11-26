//! Operator-published terms and privacy notice that owners must accept.
//!
//! One setting carries the version and both document URLs, separated by
//! single spaces, so publishing terms costs one deployment environment entry.
//! Leaving it unset publishes no documents and records no consent.

use layer2_core::legal_consent::LegalTerms;

pub(super) const KEY: &str = "CHRONO_LEGAL_TERMS";

pub(super) fn from_lookup(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<Option<LegalTerms>, String> {
    let Some(value) = lookup(KEY)?.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let [version, terms, privacy] = value.split(' ').collect::<Vec<_>>()[..] else {
        return Err(format!(
            "{KEY} must be \"<version> <terms URL> <privacy URL>\" separated by single spaces"
        ));
    };
    LegalTerms::new(version, terms, privacy)
        .map(Some)
        .map_err(|error| format!("{KEY}: {error}"))
}
