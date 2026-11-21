use crate::records::RecordError;

pub const TRACKING: &str = "CMAEG_WORKER_CREDENTIAL_TRACKING_ENABLED";
pub const OWNER_KEYS: &str = "CMAEG_OWNER_KEYS_ENABLED";
pub const SERVICE_ID: &str = "CMAEG_OWNER_KEY_SERVICE_ID";
pub const CUTOVER_ID: &str = "CMAEG_OWNER_KEY_CUTOVER_ID";

#[derive(Clone)]
pub struct Settings {
    pub tracking: bool,
    pub owner_keys: bool,
    pub service_id: Option<String>,
    pub cutover_id: Option<String>,
}

impl std::fmt::Debug for Settings {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Settings")
            .field("tracking", &self.tracking)
            .field("owner_keys", &self.owner_keys)
            .field("service_id_present", &self.service_id.is_some())
            .field("cutover_id_present", &self.cutover_id.is_some())
            .finish()
    }
}

impl Settings {
    pub fn from_env() -> Result<Self, RecordError> {
        Self::parse(|name| {
            std::env::var(name).map(Some).or_else(|error| match error {
                std::env::VarError::NotPresent => Ok(None),
                _ => Err(RecordError::Rejected(
                    "owner key configuration must be UTF-8".into(),
                )),
            })
        })
    }

    pub(crate) fn parse(
        mut get: impl FnMut(&str) -> Result<Option<String>, RecordError>,
    ) -> Result<Self, RecordError> {
        let flag = |value: Option<String>| match value.as_deref() {
            None | Some("false") => Ok(false),
            Some("true") => Ok(true),
            _ => Err(RecordError::Rejected(
                "owner key flags must be true or false".into(),
            )),
        };
        let tracking = flag(get(TRACKING)?)?;
        let owner_keys = flag(get(OWNER_KEYS)?)?;
        let service_id = get(SERVICE_ID)?.filter(|s| !s.is_empty());
        let cutover_id = get(CUTOVER_ID)?.filter(|s| !s.is_empty());
        let canonical_uuid = |value: &str| {
            uuid::Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
        };
        if owner_keys
            && (!tracking
                || !service_id.as_deref().is_some_and(canonical_uuid)
                || !cutover_id.as_deref().is_some_and(canonical_uuid))
        {
            return Err(RecordError::Rejected(
                "owner keys require tracking, service UUID and durable cutover attestation ID"
                    .into(),
            ));
        }
        Ok(Self {
            tracking,
            owner_keys,
            service_id,
            cutover_id,
        })
    }
}

#[cfg(test)]
#[path = "owner_keys_tests.rs"]
mod tests;
