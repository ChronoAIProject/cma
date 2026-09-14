use serde::{Deserialize, Serialize};

use crate::ids::PlacementId;

use super::TriggerError;

pub const MAX_LIVE_PLACEMENTS: usize = 32;
const MAX_SOURCE_URL_BYTES: usize = 512;
const MAX_LABEL_BYTES: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    pub id: PlacementId,
    pub source_url: String,
    #[serde(default)]
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<String>,
}

impl Placement {
    pub fn new(source_url: String, label: String) -> Result<Self, TriggerError> {
        let update = PlacementUpdate { source_url, label }.normalize()?;
        Ok(Self {
            id: PlacementId::generate(),
            source_url: update.source_url,
            label: update.label,
            retired_at: None,
        })
    }

    pub fn validate(&self) -> Result<(), TriggerError> {
        if !self.id.as_str().starts_with("plc_") || self.id.as_str().len() == 4 {
            return Err(TriggerError::new("invalid_placement_id", "placements.id"));
        }
        let normalized = PlacementUpdate {
            source_url: self.source_url.clone(),
            label: self.label.clone(),
        }
        .normalize()?;
        if normalized.source_url != self.source_url {
            return Err(TriggerError::new(
                "invalid_placement_url",
                "placements.source_url",
            ));
        }
        if self
            .retired_at
            .as_ref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(TriggerError::new(
                "invalid_timestamp",
                "placements.retired_at",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementUpdate {
    pub source_url: String,
    #[serde(default)]
    pub label: String,
}

impl PlacementUpdate {
    pub(crate) fn normalize(self) -> Result<Self, TriggerError> {
        if self.source_url.len() > MAX_SOURCE_URL_BYTES {
            return Err(TriggerError::new(
                "placement_url_too_long",
                "placements.source_url",
            ));
        }
        if self.label.len() > MAX_LABEL_BYTES {
            return Err(TriggerError::new(
                "placement_label_too_long",
                "placements.label",
            ));
        }
        let mut parsed = url::Url::parse(&self.source_url)
            .map_err(|_| TriggerError::new("invalid_placement_url", "placements.source_url"))?;
        if parsed.scheme() != "https"
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err(TriggerError::new(
                "invalid_placement_url",
                "placements.source_url",
            ));
        }
        parsed.set_fragment(None);
        let source_url = parsed.to_string();
        if source_url.len() > MAX_SOURCE_URL_BYTES {
            return Err(TriggerError::new(
                "placement_url_too_long",
                "placements.source_url",
            ));
        }
        Ok(Self {
            source_url,
            label: self.label,
        })
    }
}
