//! CMAEG management transport and its externally reachable callback origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GatewayConfig {
    pub base_url: String,
    pub public_url: String,
}
impl GatewayConfig {
    pub(super) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Option<Self>, String> {
        let base = lookup("CHRONO_CMAEG_BASE_URL")?.filter(|s| !s.is_empty());
        let public = lookup("CHRONO_CMAEG_PUBLIC_URL")?.filter(|s| !s.is_empty());
        match (base, public) {
            (None, None) => Ok(None),
            (Some(base_url), Some(public_url)) => {
                for (key, value) in [
                    ("CHRONO_CMAEG_BASE_URL", &base_url),
                    ("CHRONO_CMAEG_PUBLIC_URL", &public_url),
                ] {
                    super::validate_origin(key, value)?;
                }
                Ok(Some(Self {
                    base_url,
                    public_url,
                }))
            }
            _ => Err(
                "CHRONO_CMAEG_BASE_URL and CHRONO_CMAEG_PUBLIC_URL must be configured together"
                    .into(),
            ),
        }
    }
}
