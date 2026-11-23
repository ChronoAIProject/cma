//! Operator-published terms and privacy notice that owners must accept.
//!
//! All three settings are configured together or not at all. Leaving them
//! unset publishes no documents and records no consent.

use layer2_core::legal_consent::LegalTerms;

pub(super) const VERSION: &str = "CHRONO_LEGAL_TERMS_VERSION";
pub(super) const TERMS_URL: &str = "CHRONO_LEGAL_TERMS_URL";
pub(super) const PRIVACY_URL: &str = "CHRONO_LEGAL_PRIVACY_URL";

pub(super) fn from_lookup(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<Option<LegalTerms>, String> {
    let mut value = |key| lookup(key).map(|value| value.filter(|value| !value.is_empty()));
    match (value(VERSION)?, value(TERMS_URL)?, value(PRIVACY_URL)?) {
        (None, None, None) => Ok(None),
        (Some(version), Some(terms), Some(privacy)) => LegalTerms::new(&version, &terms, &privacy)
            .map(Some)
            .map_err(|error| format!("{VERSION}, {TERMS_URL} and {PRIVACY_URL}: {error}")),
        _ => Err(format!(
            "{VERSION}, {TERMS_URL} and {PRIVACY_URL} must be configured together"
        )),
    }
}
