//! Canonical record keys shared by owner read endpoints.
use cmaeg_core::{ids::RecordId, ValidationError};

pub(super) fn canonical_record_id(id: String) -> Result<RecordId, ValidationError> {
    if !uuid::Uuid::parse_str(&id)
        .is_ok_and(|parsed| parsed.get_version_num() == 4 && parsed.to_string() == id)
    {
        return Err(ValidationError::new(
            "/record_id",
            "must be a canonical UUID v4",
        ));
    }
    RecordId::new(id)
}
